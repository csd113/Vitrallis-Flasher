#!/usr/bin/env python3
"""Minimal flattened-device-tree reader used for artifact compatibility checks.

Only the structure needed by the image builder and physical-manifest validator
is implemented: node/property traversal, string properties, `__symbols__`
label extraction and overlay `__fixups__` external references. Parsing is
bounded and fails closed on malformed input; unknown tokens are rejected.
"""
import struct

FDT_MAGIC = 0xD00DFEED
FDT_BEGIN_NODE = 1
FDT_END_NODE = 2
FDT_PROP = 3
FDT_NOP = 4
FDT_END = 9
MAX_BLOB_BYTES = 8 * 1024 * 1024
MAX_NODES = 65536
MAX_PROPERTIES = 262144


class FdtError(ValueError):
    """The blob is not a supported, well-formed flattened device tree."""


class Node:
    __slots__ = ("name", "parent", "children", "properties")

    def __init__(self, name, parent):
        self.name = name
        self.parent = parent
        self.children = {}
        self.properties = {}

    def path(self):
        if self.parent is None:
            return "/"
        parts = []
        node = self
        while node.parent is not None:
            parts.append(node.name)
            node = node.parent
        return "/" + "/".join(reversed(parts))


class Fdt:
    """Parsed device tree with bounded traversal helpers."""

    def __init__(self, data: bytes):
        if not isinstance(data, (bytes, bytearray)) or len(data) < 40:
            raise FdtError("device tree blob is too small")
        if len(data) > MAX_BLOB_BYTES:
            raise FdtError("device tree blob exceeds the size bound")
        magic, totalsize, off_struct, off_strings, _, _, version, _, size_strings, size_struct = struct.unpack(
            ">10I", bytes(data[:40])
        )
        if magic != FDT_MAGIC:
            raise FdtError("missing device tree magic")
        if version < 16 or version > 17:
            raise FdtError("unsupported device tree version")
        if totalsize < 40 or totalsize > len(data):
            raise FdtError("device tree totalsize is inconsistent")
        if off_struct + size_struct > totalsize or off_strings + size_strings > totalsize:
            raise FdtError("device tree block offsets are out of range")
        self.data = bytes(data[:totalsize])
        self.version = version
        self._string_block = self.data[off_strings:off_strings + size_strings]
        self.root = None
        self._nodes = 0
        self._properties = 0
        self._parse(off_struct, size_struct)

    def _string(self, offset: int) -> str:
        if not 0 <= offset < len(self._string_block):
            raise FdtError("property name offset is out of range")
        end = self._string_block.find(b"\x00", offset)
        if end < 0:
            raise FdtError("unterminated property name")
        return self._string_block[offset:end].decode("utf-8", "strict")

    def _parse(self, offset: int, size: int) -> None:
        end = offset + size
        pos = offset
        stack = []
        root = None
        while pos < end:
            (token,) = struct.unpack_from(">I", self.data, pos)
            pos += 4
            if token == FDT_BEGIN_NODE:
                nul = self.data.find(b"\x00", pos, end)
                if nul < 0:
                    raise FdtError("unterminated node name")
                name = self.data[pos:nul].decode("utf-8", "strict")
                node = Node(name, stack[-1] if stack else None)
                if stack:
                    if name in stack[-1].children:
                        raise FdtError("duplicate device tree node")
                    stack[-1].children[name] = node
                elif root is None:
                    if name != "":
                        raise FdtError("device tree root is not the first node")
                    root = node
                else:
                    raise FdtError("multiple device tree roots")
                self._nodes += 1
                if self._nodes > MAX_NODES:
                    raise FdtError("too many device tree nodes")
                stack.append(node)
                pos = nul + 1
                pos = (pos + 3) & ~3
            elif token == FDT_END_NODE:
                if not stack:
                    raise FdtError("unbalanced device tree node")
                stack.pop()
            elif token == FDT_PROP:
                if not stack:
                    raise FdtError("property outside a node")
                length, name_offset = struct.unpack_from(">II", self.data, pos)
                pos += 8
                if pos + length > end:
                    raise FdtError("property data is out of range")
                name = self._string(name_offset)
                if name in stack[-1].properties:
                    raise FdtError("duplicate device tree property")
                stack[-1].properties[name] = self.data[pos:pos + length]
                self._properties += 1
                if self._properties > MAX_PROPERTIES:
                    raise FdtError("too many device tree properties")
                pos += length
                pos = (pos + 3) & ~3
            elif token == FDT_NOP:
                continue
            elif token == FDT_END:
                if stack:
                    raise FdtError("unterminated device tree node")
                self.root = root
                return
            else:
                raise FdtError("unknown device tree token")
        raise FdtError("device tree structure block has no end token")

    def node(self, path: str):
        if path == "/":
            return self.root
        if not path.startswith("/") or ".." in path.split("/"):
            raise FdtError("invalid device tree path")
        node = self.root
        for part in path.strip("/").split("/"):
            node = node.children.get(part)
            if node is None:
                return None
        return node

    def property(self, path: str, name: str):
        node = self.node(path)
        if node is None:
            return None
        return node.properties.get(name)

    def string(self, path: str, name: str):
        value = self.property(path, name)
        if value is None:
            return None
        end = value.find(b"\x00")
        if end < 0:
            return None
        try:
            return value[:end].decode("utf-8", "strict")
        except UnicodeDecodeError:
            return None

    def symbols(self) -> dict:
        """Return the `__symbols__` label map; empty when the node is absent."""
        node = self.node("/__symbols__")
        if node is None:
            return {}
        result = {}
        for label, raw in node.properties.items():
            end = raw.find(b"\x00")
            if end < 0:
                raise FdtError("malformed __symbols__ entry")
            result[label] = raw[:end].decode("utf-8", "strict")
        return result

    def external_fixups(self) -> dict:
        """Return overlay `__fixups__` label references as label -> [site, ...]."""
        node = self.node("/__fixups__")
        if node is None:
            return {}
        result = {}
        for label, raw in node.properties.items():
            sites = []
            for site in raw.split(b"\x00"):
                if not site:
                    continue
                text = site.decode("utf-8", "strict")
                if text.count(":") < 2:
                    raise FdtError("malformed overlay fixup site")
                sites.append(text)
            if not sites:
                raise FdtError("overlay fixup has no sites")
            result[label] = sites
        return result
