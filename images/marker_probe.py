#!/usr/bin/env python3
"""Prepare one read-only, physical marker alias in the pinned RAM recovery DTB.

No overlay merge is used: new overlay children precede existing partitions and
would renumber MTD devices. This fixed transformation appends its sole child.
It performs no device operation and grants no NAND write capability.
"""
import argparse
import hashlib
import json
import pathlib
import struct

from fdt import Fdt

BASE_SHA256 = '0132f7fa312542c374c7e5dfec2a1a12893b9cbe490d22e165fd7a89de51762e'
PARTITIONS = '/soc/nand-controller@1c03000/nand@0/partitions'
PROBE_NAME = 'partition@38800000'
PHYSICAL_OFFSET = 947912704
ERASE_BYTES = 4194304


def _end_of_partitions(data, header):
    pos = header[2]
    end = pos + header[9]
    stack = []
    while pos < end:
        begin = pos
        token, = struct.unpack_from('>I', data, pos)
        pos += 4
        if token == 1:
            nul = data.index(b'\0', pos, end)
            stack.append(data[pos:nul].decode())
            pos = (nul + 4) & ~3
        elif token == 2:
            if '/' + '/'.join(stack[1:]) == PARTITIONS:
                return begin
            stack.pop()
        elif token == 3:
            length, = struct.unpack_from('>I', data, pos)
            pos = (pos + 8 + length + 3) & ~3
        elif token == 4:
            continue
        elif token == 9:
            break
        else:
            raise ValueError('unexpected FDT token')
    raise ValueError('missing fixed partitions node')


def _walk(node):
    yield node
    for child in node.children.values():
        yield from _walk(child)


def validate_preservation(original, derived):
    """Require exactly one new read-only child; every existing node stays intact."""
    before = Fdt(original)
    after = Fdt(derived)
    old = {node.path(): node for node in _walk(before.root)}
    new = {node.path(): node for node in _walk(after.root)}
    probe_path = PARTITIONS + '/' + PROBE_NAME
    if set(new) - set(old) != {probe_path} or set(old) - set(new):
        raise ValueError('unexpected device tree topology change')
    for path, node in old.items():
        expected_children = list(node.children)
        if path == PARTITIONS:
            expected_children.append(PROBE_NAME)
        if new[path].properties != node.properties or list(new[path].children) != expected_children:
            raise ValueError('existing device tree or partition order changed')
    expected = {
        'label': b'BBM.probe\0',
        'reg': struct.pack('>4I', 0, PHYSICAL_OFFSET, 0, ERASE_BYTES),
        'read-only': b'',
    }
    if new[probe_path].properties != expected or new[probe_path].children:
        raise ValueError('probe must be the exact read-only physical block')


def append_probe(data):
    """Reject any unpinned input, then append the fixed physical alias."""
    if hashlib.sha256(data).hexdigest() != BASE_SHA256:
        raise ValueError('unpinned base device tree')
    tree = Fdt(data)
    header = list(struct.unpack('>10I', data[:40]))
    if header[1] != len(data) or header[2] != 56 or header[4] != 40 or any(data[40:56]):
        raise ValueError('unsupported pinned DTB reservation/header layout')
    if header[2] + header[9] != header[3] or header[3] + header[8] != len(data):
        raise ValueError('unsupported pinned DTB block layout')
    parent = tree.node(PARTITIONS)
    if parent is None or len(parent.children) != 5 or PROBE_NAME in parent.children:
        raise ValueError('unexpected original partition layout')
    if parent.properties.get('#address-cells') != struct.pack('>I', 2) or parent.properties.get('#size-cells') != struct.pack('>I', 2):
        raise ValueError('unsupported partition cells')
    strings = bytearray(data[header[3]:])

    def prop(name, value):
        encoded = name.encode() + b'\0'
        offset = strings.find(encoded)
        if offset < 0:
            offset = len(strings)
            strings.extend(encoded)
        raw = struct.pack('>3I', 3, len(value), offset) + value
        return raw + bytes(-len(raw) % 4)

    node = struct.pack('>I', 1) + PROBE_NAME.encode() + b'\0'
    node += bytes(-len(node) % 4)
    node += prop('label', b'BBM.probe\0')
    node += prop('reg', struct.pack('>4I', 0, PHYSICAL_OFFSET, 0, ERASE_BYTES))
    node += prop('read-only', b'') + struct.pack('>I', 2)
    insert = _end_of_partitions(data, header)
    structure = data[header[2]:insert] + node + data[insert:header[3]]
    header[3] += len(node)
    header[9] += len(node)
    header[8] = len(strings)
    header[1] = header[3] + len(strings)
    result = struct.pack('>10I', *header) + data[40:56] + structure + strings
    validate_preservation(data, result)
    return result


def build(base, output):
    base = pathlib.Path(base)
    output = pathlib.Path(output)
    for path in (base, *base.parents, output, *output.parents):
        if path.is_symlink():
            raise ValueError('symlink input or output path')
    if not base.is_file() or base.stat().st_size > 32768:
        raise ValueError('base DTB bound or type')
    data = base.read_bytes()
    derived = append_probe(data)
    metadata = {
        'base_sha256': BASE_SHA256,
        'derived_sha256': hashlib.sha256(derived).hexdigest(),
        'derived_bytes': len(derived),
        'physical_probe_offset': PHYSICAL_OFFSET,
        'physical_probe_bytes': ERASE_BYTES,
        'probe_label': 'BBM.probe',
        'read_only': True,
        'slc_mode': False,
        'existing_nodes_properties_and_partition_order_unchanged': True,
        'scope': 'Pinned diagnostic RAM DTB preparation only; no physical load/read/write performed',
    }
    output.mkdir(mode=0o700)
    for name, content in [('marker-probe.dtb', derived), ('metadata.json', (json.dumps(metadata, indent=2) + '\n').encode())]:
        with (output / name).open('xb') as stream:
            (output / name).chmod(0o600)
            stream.write(content)
    return metadata


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    print(json.dumps(build(args.base, args.output), indent=2))
