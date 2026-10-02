#!/usr/bin/env python3
"""Legacy uImage parsing shared by the builder and the manifest validator."""
import struct
import zlib

UIMAGE_MAGIC = 0x27051956
UIMAGE_TYPE_SCRIPT = 6
UIMAGE_TYPE_KERNEL = 2
UIMAGE_TYPE_RAMDISK = 3
UIMAGE_HEADER_BYTES = 64


class UImageError(ValueError):
    """The bytes are not a well-formed uImage."""


def script_text(payload: bytes) -> str:
    """Decode a uImage script payload, allowing the length-prefixed form.

    flash-kernel's generated boot.scr carries an 8-byte prefix (big-endian
    payload length minus 8, followed by four zero bytes) before the UTF-8
    script text. The prefix is stripped only when it is internally consistent;
    the remainder must still be strict UTF-8.
    """
    body = payload
    if len(payload) >= 8 and payload[4:8] == b"\x00\x00\x00\x00" and int.from_bytes(payload[:4], "big") == len(payload) - 8:
        body = payload[8:]
    return body.decode("utf-8", "strict")


def parse(data: bytes):
    """Parse a uImage and verify both header and data CRCs.

    Returns `(header, payload)` where header is a dict. Raises `UImageError`
    for truncation, wrong magic, or either CRC mismatch.
    """
    if not isinstance(data, (bytes, bytearray)) or len(data) < UIMAGE_HEADER_BYTES:
        raise UImageError("uImage is too small")
    (
        magic,
        header_crc,
        timestamp,
        size,
        load,
        entry,
        data_crc,
        os_id,
        arch,
        image_type,
        compression,
    ) = struct.unpack(">IIIIIIIBBBB", bytes(data[:32]))
    if magic != UIMAGE_MAGIC:
        raise UImageError("missing uImage magic")
    if UIMAGE_HEADER_BYTES + size > len(data):
        raise UImageError("uImage payload is truncated")
    header = bytearray(data[:UIMAGE_HEADER_BYTES])
    header[4:8] = b"\x00\x00\x00\x00"
    if zlib.crc32(header) & 0xFFFFFFFF != header_crc:
        raise UImageError("uImage header CRC mismatch")
    payload = bytes(data[UIMAGE_HEADER_BYTES:UIMAGE_HEADER_BYTES + size])
    if zlib.crc32(payload) & 0xFFFFFFFF != data_crc:
        raise UImageError("uImage data CRC mismatch")
    return {
        "timestamp": timestamp,
        "size": size,
        "load": load,
        "entry": entry,
        "os": os_id,
        "arch": arch,
        "type": image_type,
        "compression": compression,
    }, payload


def build(header: dict, payload: bytes) -> bytes:
    """Build a deterministic uImage from a header dict and payload.

    Used by tests, fixture tooling and the reviewed RAM-only recovery builder.
    """
    name = header.get("name", "test")[:31].encode()
    fields = struct.pack(
        ">IIIIIIIBBBB",
        UIMAGE_MAGIC,
        0,
        int(header.get("timestamp", 0)),
        len(payload),
        int(header.get("load", 0)),
        int(header.get("entry", 0)),
        0,
        int(header.get("os", 5)),
        int(header.get("arch", 2)),
        int(header.get("type", UIMAGE_TYPE_SCRIPT)),
        int(header.get("compression", 0)),
    )
    header_bytes = bytearray(fields + name + b"\x00" * (32 - len(name)))
    header_bytes[24:28] = struct.pack(">I", zlib.crc32(payload) & 0xFFFFFFFF)
    header_bytes[4:8] = b"\x00\x00\x00\x00"
    header_bytes[4:8] = struct.pack(">I", zlib.crc32(header_bytes) & 0xFFFFFFFF)
    return bytes(header_bytes) + payload
