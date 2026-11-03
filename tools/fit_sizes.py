"""Brute-force byte sizes of unknown Alg-B value types using end-offset consistency.

usage: fit_sizes.py DIR [...]   -> prints best size per type
"""
import os
import struct
import sys

import h5py

from alg_b import B, Fail

MAGIC = 0x1B7C2FDD
EXTS = (".spsm", ".spmsk", ".sppr", ".spt")
CAND = [1, 2, 4, 8, 12, 16, 24, 32, 36, 48, 64, 128]


def load(paths, limit=120):
    out = []
    for a in paths:
        files = []
        if os.path.isdir(a):
            for dp, _, fns in os.walk(a):
                files += [os.path.join(dp, x) for x in fns if x.lower().endswith(EXTS)]
        else:
            files.append(a)
        for fp in files:
            try:
                with h5py.File(fp, "r") as f:
                    names = []
                    f.visititems(lambda n, o: names.append(n) if isinstance(o, h5py.Dataset) else None)
                    for n in names:
                        ds = f[n]
                        if ds.dtype.kind == "u" and 12 <= ds.size <= 4e6:
                            b = ds[()].tobytes()
                            if struct.unpack_from("<I", b)[0] == MAGIC and struct.unpack_from("<I", b, 4)[0] == 0:
                                out.append(b)
            except OSError:
                pass
            if len(out) >= limit:
                return out
    return out


def score(blobs):
    ok = parsed = 0
    unknown = {}
    for b in blobs:
        d = B(b)
        try:
            d.run()
            if d.p == len(b) and not d.warn:
                ok += 1
        except Fail:
            for t in d.unknown:
                unknown[t] = unknown.get(t, 0) + 1
        parsed += d.p
    return ok, parsed, unknown


def main():
    blobs = load(sys.argv[1:])
    print("blobs:", len(blobs))
    B.FIXED = {}
    for _ in range(12):
        ok, parsed, unknown = score(blobs)
        print(f"state: ok={ok}/{len(blobs)} parsed={parsed} unknown={ {hex(k): v for k, v in unknown.items()} } fixed={ {hex(k): v for k, v in B.FIXED.items()} }")
        if not unknown:
            break
        t = max(unknown, key=unknown.get)
        best = None
        for s in CAND:
            B.FIXED[t] = s
            o2, p2, _ = score(blobs)
            if best is None or (p2, o2) > best[0]:
                best = ((p2, o2), s)
        B.FIXED[t] = best[1]
        print(f"  -> type {t:#04x}: best size {best[1]} (parsed={best[0][0]} ok={best[0][1]})")


if __name__ == "__main__":
    main()
