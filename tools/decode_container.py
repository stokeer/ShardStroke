"""Decode every Alg member of a Painter HDF5 container (variants A and B).

usage: decode_container.py FILE [--max-mb 8] [--json OUTDIR]
"""
import json
import os
import sys

import h5py

from alg import AlgError, decode, is_alg


def main():
    a = sys.argv[1:]
    path = a[0]
    max_mb = float(a[a.index("--max-mb") + 1]) if "--max-mb" in a else 8
    outdir = a[a.index("--json") + 1] if "--json" in a else None
    ok = bad = other = skipped = 0
    with h5py.File(path, "r") as f:
        names = []
        f.visititems(lambda n, o: names.append(n) if isinstance(o, h5py.Dataset) else None)
        for n in names:
            ds = f[n]
            if ds.dtype.kind != "u" or ds.dtype.itemsize != 1:
                continue
            if ds.size > max_mb * 1e6:
                skipped += 1
                continue
            b = ds[()].tobytes()
            if not is_alg(b):
                other += 1
                continue
            try:
                root = decode(b)
                ok += 1
                st = "OK"
                if outdir:
                    os.makedirs(outdir, exist_ok=True)
                    with open(os.path.join(outdir, n.replace("/", "__") + ".json"), "w", encoding="utf-8") as fh:
                        json.dump(root, fh, indent=1)
            except AlgError as e:
                bad += 1
                st = f"FAIL {str(e)[:60]}"
            print(f"{st:48} {len(b):>9}  {n}")
    print(f"\nalg-ok={ok} alg-bad={bad} non-alg={other} skipped-large={skipped}")


if __name__ == "__main__":
    main()
