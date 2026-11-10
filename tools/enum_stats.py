"""Histogram of integer-valued properties (candidate enums) over a corpus.

usage: enum_stats.py PATH [...]
"""
import os
import sys
from collections import Counter, defaultdict

import h5py

from alg import AlgError, decode, is_alg

EXTS = (".spsm", ".spmsk", ".sppr", ".spt", ".spp")


def walk(o, acc):
    if isinstance(o, dict):
        c = o.get("_class", "?")
        for k, v in o.items():
            if k == "_class":
                continue
            if isinstance(v, bool):
                continue
            if isinstance(v, int):
                acc[f"{c}.{k}"][v] += 1
            walk(v, acc)
    elif isinstance(o, list):
        for v in o:
            walk(v, acc)


def main():
    files = []
    for a in sys.argv[1:]:
        if os.path.isdir(a):
            for dp, _, fns in os.walk(a):
                files += [os.path.join(dp, x) for x in fns if x.lower().endswith(EXTS)]
        else:
            files.append(a)
    acc = defaultdict(Counter)
    n = 0
    for fp in files:
        try:
            with h5py.File(fp, "r") as f:
                names = []
                f.visititems(lambda nm, o: names.append(nm) if isinstance(o, h5py.Dataset) else None)
                for nm in names:
                    ds = f[nm]
                    if ds.dtype.kind != "u" or ds.size > 30e6:
                        continue
                    b = ds[()].tobytes()
                    if not is_alg(b):
                        continue
                    try:
                        walk(decode(b), acc)
                        n += 1
                    except AlgError:
                        pass
        except OSError:
            pass
    print("members decoded:", n)
    skip = ("uid", "uuid", "count")
    for k in sorted(acc):
        if k.endswith(skip):
            continue
        c = acc[k]
        if len(c) <= 24:
            print(f"{k:55} {dict(sorted(c.items()))}")


if __name__ == "__main__":
    main()
