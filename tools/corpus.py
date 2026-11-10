"""Run the B decoder on every Alg member (ver=0) of many containers; list failures.

usage: corpus.py DIR_OR_FILE [...]   (walks for .spsm/.spmsk/.sppr/.spt)
"""
import os
import struct
import sys
from collections import Counter

import h5py

from alg_b import B, Fail

MAGIC = 0x1B7C2FDD
EXTS = (".spsm", ".spmsk", ".sppr", ".spt")


def members(path, max_mb=6):
    with h5py.File(path, "r") as f:
        names = []
        f.visititems(lambda n, o: names.append(n) if isinstance(o, h5py.Dataset) else None)
        for n in names:
            ds = f[n]
            if ds.dtype.kind == "u" and ds.size <= max_mb * 1e6 and ds.size >= 12:
                head = ds[:12].tobytes()
                if struct.unpack_from("<I", head)[0] == MAGIC:
                    yield n, ds[()].tobytes()


def main():
    files = []
    for a in sys.argv[1:]:
        if os.path.isdir(a):
            for dp, _, fns in os.walk(a):
                files += [os.path.join(dp, x) for x in fns if x.lower().endswith(EXTS)]
        else:
            files.append(a)
    ok = bad = a_var = 0
    reasons = Counter()
    examples = {}
    for fp in files:
        try:
            for n, b in members(fp):
                if struct.unpack_from("<I", b, 4)[0] != 0:
                    a_var += 1
                    continue
                d = B(b)
                try:
                    d.run()
                    if d.p == len(b) and not d.warn:
                        ok += 1
                        continue
                    r = d.warn[0] if d.warn else f"short {d.p}/{len(b)}"
                except Fail as e:
                    r = str(e)
                bad += 1
                key = r.split("@")[0].split(" end ")[0][:70]
                reasons[key] += 1
                examples.setdefault(key, f"{os.path.basename(fp)}:{n}  [{r[:90]}]")
        except Exception as e:  # noqa: BLE001
            print("ERR", fp, e)
    print(f"files={len(files)} B-ok={ok} B-bad={bad} variantA-skipped={a_var}")
    for k, c in reasons.most_common(15):
        print(f"  {c:4}  {k}\n        e.g. {examples[k]}")


if __name__ == "__main__":
    main()
