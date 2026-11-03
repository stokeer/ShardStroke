"""List every class seen in an Alg member with its properties and one example value.

usage: classes.py CONTAINER MEMBER [class-substring]
"""
import sys

import h5py

import alg


def short(v):
    if isinstance(v, dict):
        return f"<{v.get('_class', 'dict')}>"
    if isinstance(v, list):
        return f"[{len(v)}]"
    s = str(v)
    return s if len(s) < 40 else s[:37] + "..."


def main():
    path, member = sys.argv[1], sys.argv[2]
    pat = sys.argv[3] if len(sys.argv) > 3 else ""
    root = alg.decode(h5py.File(path, "r")[member][()].tobytes())
    seen = {}

    def walk(o):
        if isinstance(o, dict):
            c = o.get("_class")
            if c and c not in seen:
                seen[c] = o
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(root)
    for c, o in seen.items():
        if pat and pat not in c:
            continue
        print(c)
        for k, v in o.items():
            if k != "_class":
                print(f"    {k:32} {short(v)}")


if __name__ == "__main__":
    main()
