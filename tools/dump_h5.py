"""Dump HDF5 tree: groups, datasets (shape/dtype), attrs, small scalar/string values.

usage: dump_h5.py FILE [max_children_per_group=40] [max_depth=12]
"""
import sys

import h5py
import numpy as np


def short(v, n=120):
    if isinstance(v, bytes):
        try:
            v = v.decode("utf-8")
        except UnicodeDecodeError:
            return f"<{len(v)} bytes>"
    if isinstance(v, np.ndarray):
        if v.size > 8:
            return f"<array {v.shape} {v.dtype}>"
        v = v.tolist()
    s = str(v)
    return s if len(s) <= n else s[:n] + "..."


def attrs(o, ind):
    for k, v in o.attrs.items():
        print(f"{ind}  @{k} = {short(v)}")


def walk(g, ind, depth, maxc, maxd):
    items = list(g.items())
    for i, (name, o) in enumerate(items):
        if i >= maxc:
            print(f"{ind}... (+{len(items) - maxc} more)")
            break
        if isinstance(o, h5py.Group):
            print(f"{ind}[G] {name}/  ({len(o)} children)")
            attrs(o, ind)
            if depth < maxd:
                walk(o, ind + "  ", depth + 1, maxc, maxd)
        else:
            extra = ""
            try:
                if o.shape == () or (o.size <= 4 and o.dtype.kind in "iufbSO"):
                    extra = " = " + short(o[()])
            except Exception as e:  # noqa: BLE001
                extra = f" <read error {e}>"
            print(f"{ind}[D] {name}  shape={o.shape} dtype={o.dtype}{extra}")
            attrs(o, ind)


def main():
    path = sys.argv[1]
    maxc = int(sys.argv[2]) if len(sys.argv) > 2 else 40
    maxd = int(sys.argv[3]) if len(sys.argv) > 3 else 12
    with h5py.File(path, "r") as f:
        print(f"FILE {path}")
        print(f"root attrs: {len(f.attrs)}")
        attrs(f, "")
        walk(f, "", 0, maxc, maxd)


if __name__ == "__main__":
    main()
