"""Extract opaque uint8 blobs from a Painter HDF5 file and classify them.

usage: blobs.py FILE OUTDIR
"""
import math
import os
import sys
import zlib
from collections import Counter

import h5py


def entropy(b):
    if not b:
        return 0.0
    c = Counter(b)
    n = len(b)
    return -sum(v / n * math.log2(v / n) for v in c.values())


def guess(b):
    if b.startswith(b"7z\xbc\xaf"):
        return "7z"
    if b.startswith(b"PK"):
        return "zip"
    if b[:2] in (b"\x78\x9c", b"\x78\x01", b"\x78\xda", b"\x78\x5e"):
        try:
            zlib.decompress(b)
            return "zlib(ok)"
        except Exception:  # noqa: BLE001
            return "zlib?(fail)"
    if b.startswith(b"{") or b.startswith(b"["):
        return "json?"
    if b.startswith(b"<?xml") or b.startswith(b"<"):
        return "xml?"
    if b.startswith(b"\x89PNG"):
        return "png"
    printable = sum(1 for x in b[:512] if 32 <= x < 127 or x in (9, 10, 13))
    if b[:512] and printable / len(b[:512]) > 0.95:
        return "text"
    return "bin"


def main():
    path, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    rows = []
    with h5py.File(path, "r") as f:
        def visit(name, o):
            if isinstance(o, h5py.Dataset) and o.dtype.kind == "u" and o.dtype.itemsize == 1:
                b = o[()].tobytes()
                safe = name.replace("/", "__")
                with open(os.path.join(out, safe), "wb") as fh:
                    fh.write(b)
                rows.append((name, len(b), guess(b), entropy(b[:65536]), b[:24]))
        f.visititems(visit)
    for name, n, g, e, head in rows:
        h = " ".join(f"{x:02X}" for x in head)
        a = "".join(chr(x) if 32 <= x < 127 else "." for x in head)
        print(f"{n:>9}  {g:11} H={e:4.2f}  {name}\n           {h} |{a}|")


if __name__ == "__main__":
    main()
