#!/usr/bin/env python3
"""Set (or check) the PE header checksum of Windows executables.

Neither lld-link (cargo-xwin) nor makensis fills in OptionalHeader.CheckSum,
so our unsigned Windows binaries ship with a checksum of 0. Windows only
enforces the field for drivers, but a zero checksum is one more anomaly that
antivirus heuristics hold against an unsigned file. Code signing tools
recompute it; until the Windows certificate exists, this script does.

Usage: pe-checksum.py [--check] FILE...
  --check   report the state and exit 1 on any mismatch, without writing.
"""
import struct
import sys


def checksum_offset(data: bytes) -> int:
    if data[:2] != b"MZ":
        raise ValueError("not a DOS/PE executable")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("PE signature not found")
    return pe + 24 + 64  # optional header + CheckSum field


def compute(data: bytes, skip: int) -> int:
    """The algorithm Windows uses (documented for imagehlp's CheckSumMappedFile)."""
    total = 0
    limit = len(data) - len(data) % 4
    for i in range(0, limit, 4):
        if i == skip:
            continue
        total += struct.unpack_from("<I", data, i)[0]
        total = (total & 0xFFFFFFFF) + (total >> 32)
    if limit != len(data):
        tail = data[limit:] + b"\0" * (4 - len(data) + limit)
        total += struct.unpack("<I", tail)[0]
        total = (total & 0xFFFFFFFF) + (total >> 32)
    total = (total & 0xFFFF) + (total >> 16)
    total += total >> 16
    total &= 0xFFFF
    return (total + len(data)) & 0xFFFFFFFF


def main(argv):
    check = "--check" in argv
    files = [a for a in argv if a != "--check"]
    if not files:
        print(__doc__, file=sys.stderr)
        return 2
    status = 0
    for path in files:
        with open(path, "rb") as f:
            data = bytearray(f.read())
        off = checksum_offset(data)
        current = struct.unpack_from("<I", data, off)[0]
        wanted = compute(data, off)
        if current == wanted:
            print(f"{path}: checksum 0x{current:08x} OK")
            continue
        if check:
            print(f"{path}: checksum 0x{current:08x}, expected 0x{wanted:08x}")
            status = 1
            continue
        struct.pack_into("<I", data, off, wanted)
        with open(path, "wb") as f:
            f.write(data)
        print(f"{path}: checksum 0x{current:08x} -> 0x{wanted:08x}")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
