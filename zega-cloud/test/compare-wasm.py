"""Compare WASM code exactly and enumerate every changed panic-location byte.

Usage: python3 zega-cloud/test/compare-wasm.py MAIN.wasm BRANCH.wasm MAIN_REV
Fails on size growth, any non-data change, or anything except source-line moves.
"""
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys


def leb(data, offset):
    value = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
        shift += 7


def sections(data):
    assert data[:8] == b'\0asm\x01\0\0\0', 'expected WASM v1'
    result = []
    offset = 8
    while offset < len(data):
        section = data[offset]
        size, start = leb(data, offset + 1)
        result.append((section, start, data[start:start + size]))
        offset = start + size
    return result


def segments(payload):
    count, offset = leb(payload, 0)
    result = []
    for _ in range(count):
        mode, offset = leb(payload, offset)
        assert mode == 0 and payload[offset] == 0x41, 'expected active memory-zero segment'
        address, offset = leb(payload, offset + 1)
        assert payload[offset] == 0x0b, 'expected end of constant expression'
        size, offset = leb(payload, offset + 1)
        result.append((address, offset, payload[offset:offset + size]))
        offset += size
    assert offset == len(payload)
    return result


def compare(before, after, main_rev):
    a, b = Path(before).read_bytes(), Path(after).read_bytes()
    assert len(a) == len(b), f'expected equal sizes: {len(a)} vs {len(b)}'
    left, right = sections(a), sections(b)
    assert len(left) == len(right)
    report = {'mainRevision': main_rev, 'main': {'bytes': len(a), 'sha256': hashlib.sha256(a).hexdigest()},
              'branch': {'bytes': len(b), 'sha256': hashlib.sha256(b).hexdigest()},
              'byteIdentical': a == b, 'sizeGrowthBytes': len(b) - len(a),
              'identicalSections': [], 'differences': []}
    for (kind, start, raw), (other_kind, other_start, other_raw) in zip(left, right):
        assert (kind, start, len(raw)) == (other_kind, other_start, len(other_raw))
        if raw == other_raw:
            report['identicalSections'].append(kind)
            continue
        assert kind == 11, f'non-data section changed: {kind}'
        old_segments, new_segments = segments(raw), segments(other_raw)
        assert len(old_segments) == len(new_segments)
        def source(ptr, size):
            for addr, _, data in old_segments:
                if addr <= ptr and ptr + size <= addr + len(data):
                    return data[ptr - addr:ptr - addr + size].decode('utf8')
            raise ValueError('source pointer outside data segments')
        for (addr, off, data), (new_addr, new_off, new_data) in zip(old_segments, new_segments):
            assert (addr, off, len(data)) == (new_addr, new_off, len(new_data))
            changed = {i for i, (x, y) in enumerate(zip(data, new_data)) if x != y}
            while changed:
                first = min(changed)
                location = first - ((addr + first) % 4) - 8
                ptr, size, line, column = struct.unpack_from('<IIII', data, location)
                new_ptr, new_size, new_line, new_column = struct.unpack_from('<IIII', new_data, location)
                assert (ptr, size, column) == (new_ptr, new_size, new_column), 'change beyond line number'
                name = source(ptr, size)
                assert name.startswith('/zega/') and name.endswith('.rs'), name
                path = name.removeprefix('/zega/')
                old_source = subprocess.check_output(['git', 'show', f'{main_rev}:{path}'], text=True).splitlines()[line - 1]
                new_source = Path(path).read_text().splitlines()[new_line - 1]
                assert old_source == new_source, f'changed source at {name}:{new_line}'
                offsets = sorted(i for i in changed if location + 8 <= i < location + 12)
                assert offsets, 'expected changed line-number bytes'
                report['differences'].append({'file': path, 'mainLine': line, 'branchLine': new_line,
                    'column': column, 'source': old_source.strip(), 'bytes': [
                        {'offset': start + off + i, 'main': data[i], 'branch': new_data[i]} for i in offsets]})
                changed.difference_update(offsets)
    report['differingBytes'] = sum(len(d['bytes']) for d in report['differences'])
    assert report['differingBytes'] == sum(x != y for x, y in zip(a, b)), 'unexplained bytes'
    return report


if __name__ == '__main__':
    print(json.dumps(compare(*sys.argv[1:]), indent=2))
