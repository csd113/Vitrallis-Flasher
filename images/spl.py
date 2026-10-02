#!/usr/bin/env python3
"""Deterministic per-part NAND SPL image generation.

Upstream `lib-nand.sh` fills one erase block with four BROM copies of
`sunxi-spl.bin` and pads the unused pages with bytes read from `/dev/urandom`.
Both the page padding passed back into the tool and the tool-internal
"randomize unused space" branch of `sunxi-nand-image-builder` therefore change
on every run. This module replaces only those entropy inputs with a documented,
content-addressed stream, keeping the unmodified pinned tool and its exact
NAND layout semantics.

Deterministic stream definition (checked in, no secret state):

    seed    = SHA-256(b"Vitrallis Flasher deterministic NAND SPL padding v1")
    block_i = SHA-256(seed || 0x00 || label || big-endian-uint64(i))

Setting `spl.py`'s `stream()` output as the tool's `/dev/urandom` replaces raw
kernel entropy with a fixed, reproducible stream; the byte count the tool reads
is unchanged because the tool still performs the same reads in the same order.
The 256 KiB tool source is `[sunxi-spl.bin (16 KiB) + pad-group (48 KiB)] x 4`,
which reproduces upstream's four 64-page BROM groups in one tool invocation.

Every generated image is structurally validated page by page: exact geometry,
OOB geometry implied by the file size, BBM bytes, and a descramble of the
ECC-protected region checked against the exact source chunk (the BROM
scrambler is reproduced in Python). Nothing here writes to NAND.
"""
import hashlib

PAGE_SIZE = 16384
USABLE_PAGE_SIZE = 1024
ECC_STEP_SIZE = 1024
ERASEBLOCK_SIZE = 4194304
PAGES_PER_ERASEBLOCK = ERASEBLOCK_SIZE // PAGE_SIZE  # 256
BROM_COPY_PAGES = 64
BROM_COPY_COUNT = PAGES_PER_ERASEBLOCK // BROM_COPY_PAGES  # 4
SPL_SIZE = 16384
SPL_PAGES = SPL_SIZE // USABLE_PAGE_SIZE  # 16
PAD_PAGES = BROM_COPY_PAGES - SPL_PAGES  # 48
PAD_BYTES = PAD_PAGES * USABLE_PAGE_SIZE  # 49152
SOURCE_BYTES = SPL_SIZE + PAD_BYTES  # per-copy source, 65536
TOTAL_SOURCE_BYTES = SOURCE_BYTES * BROM_COPY_COUNT  # 262144
ECC_STRENGTH = 64
ECC_BYTES = ((ECC_STRENGTH * 14) + 7) // 8  # 112, even
ECC_REGION = ECC_STEP_SIZE + 4 + ECC_BYTES  # 1140 scrambled bytes per page
OOB_SIZES = {"hynix": 1664, "toshiba": 1280}
SEED = hashlib.sha256(b"Vitrallis Flasher deterministic NAND SPL padding v1").digest()
PAD_LABELS = tuple(f"source-pad-{index}".encode() for index in range(BROM_COPY_COUNT))
ENTROPY_LABEL = b"tool-unused-space"


class SplError(ValueError):
    """A source or generated SPL image violates the documented contract."""


def stream(label: bytes, length: int) -> bytes:
    """Return `length` deterministic bytes derived from `label` and `SEED`."""
    if not isinstance(label, bytes) or not label or len(label) > 64:
        raise SplError("stream label must be 1..64 bytes")
    if type(length) is not int or not 0 <= length <= 64 * 1024 * 1024:
        raise SplError("stream length out of range")
    out = bytearray()
    counter = 0
    while len(out) < length:
        out += hashlib.sha256(SEED + b"\x00" + label + counter.to_bytes(8, "big")).digest()
        counter += 1
    return bytes(out[:length])


def validate_source_spl(data: bytes) -> None:
    """Reject anything that is not the expected 16 KiB sun5i SPL binary."""
    if not isinstance(data, bytes) or len(data) != SPL_SIZE:
        raise SplError("sunxi-spl.bin must be exactly 16384 bytes")
    if data[4:12] != b"eGON.BT0":
        raise SplError("sunxi-spl.bin lacks the eGON.BT0 magic")
    if int.from_bytes(data[16:20], "little") != SPL_SIZE:
        raise SplError("eGON header length disagrees with the file size")
    if data[20:24] != b"SPL\x02":
        raise SplError("sunxi-spl.bin does not declare the SPL v2 signature")


def build_source(sunxi_spl: bytes) -> bytes:
    """Build the deterministic 256 KiB tool input with four BROM groups."""
    validate_source_spl(sunxi_spl)
    chunks = []
    for index in range(BROM_COPY_COUNT):
        chunks.append(sunxi_spl)
        chunks.append(stream(PAD_LABELS[index], PAD_BYTES))
    source = b"".join(chunks)
    if len(source) != TOTAL_SOURCE_BYTES:
        raise SplError("internal source assembly error")
    return source


def entropy_bytes(oob_size: int) -> int:
    """Exact number of `/dev/urandom` bytes the tool consumes for this geometry."""
    oob = oob_for_variant(oob_size)
    tail = PAGE_SIZE + oob - ECC_REGION
    return tail * PAGES_PER_ERASEBLOCK


def entropy_stream(variant) -> bytes:
    """Deterministic replacement for the tool's `/dev/urandom` input.

    The stream must be at least `entropy_bytes(variant)` long; generating
    exactly that many bytes keeps the consumed prefix fixed and testable.
    """
    token = variant_token(variant)
    return stream(ENTROPY_LABEL + b"-" + token.encode(), entropy_bytes(token))


def image_size(oob_size: int) -> int:
    """Exact size of the one-erase-block data+OOB image for this geometry."""
    return PAGES_PER_ERASEBLOCK * (PAGE_SIZE + oob_for_variant(oob_size))


def oob_for_variant(variant) -> int:
    """Return the OOB byte count for a variant name or integer OOB size."""
    if isinstance(variant, str):
        try:
            return OOB_SIZES[variant]
        except KeyError:
            raise SplError("unknown NAND variant") from None
    if type(variant) is int and variant in OOB_SIZES.values():
        return variant
    raise SplError("unknown NAND variant")


def lfsr_step(state: int, count: int) -> int:
    """Exact port of the tool's `lfsr_step` for the BROM boot0 seed."""
    state &= 0x7FFF
    while count > 0:
        state = ((state >> 1) | ((((state ^ (state >> 1)) & 1) << 14))) & 0x7FFF
        count -= 1
    return state


def scrambler_stream(length: int) -> bytes:
    """BROM boot0 keystream (seed 0x4a80, advanced 15 steps before use)."""
    if type(length) is not int or not 0 <= length <= 1024 * 1024:
        raise SplError("scrambler length out of range")
    out = bytearray()
    state = lfsr_step(0x4A80, 15)
    for _ in range(length):
        out.append(state & 0xFF)
        state = lfsr_step(state, 8)
    return bytes(out)


def descramble(data: bytes) -> bytes:
    """Invert the tool's boot0 scramble over `ECC_REGION` bytes."""
    if not isinstance(data, bytes) or len(data) != ECC_REGION:
        raise SplError("descramble expects the ECC-protected region")
    return bytes(a ^ b for a, b in zip(data, scrambler_stream(len(data))))


def validate_image(image: bytes, oob_size: int, sunxi_spl: bytes) -> None:
    """Structurally validate one generated erase-block SPL image.

    Checks the exact size, per-page BBM bytes and the descrambled data region
    of every page against the exact input chunk it must carry. ECC bytes
    themselves are tool-generated and are not recomputed here.
    """
    oob = oob_for_variant(oob_size)
    validate_source_spl(sunxi_spl)
    if not isinstance(image, bytes) or len(image) != image_size(oob):
        raise SplError("generated image has the wrong size for its OOB geometry")
    for page in range(PAGES_PER_ERASEBLOCK):
        offset = page * (PAGE_SIZE + oob)
        region = descramble(image[offset:offset + ECC_REGION])
        within = page % BROM_COPY_PAGES
        if within < SPL_PAGES:
            expected = sunxi_spl[within * USABLE_PAGE_SIZE:(within + 1) * USABLE_PAGE_SIZE]
        else:
            group = page // BROM_COPY_PAGES
            expected = stream(PAD_LABELS[group], PAD_BYTES)[
                (within - SPL_PAGES) * USABLE_PAGE_SIZE:(within - SPL_PAGES + 1) * USABLE_PAGE_SIZE
            ]
        if region[:USABLE_PAGE_SIZE] != expected:
            raise SplError(f"page {page} does not carry the expected source chunk")
        if image[offset + PAGE_SIZE:offset + PAGE_SIZE + 2] != b"\xff\xff":
            raise SplError(f"page {page} BBM bytes are not erased")


def variant_token(variant) -> str:
    """Canonical variant name for file naming and manifests."""
    oob = oob_for_variant(variant)
    for name, value in OOB_SIZES.items():
        if value == oob:
            return name
    raise SplError("unknown NAND variant")
