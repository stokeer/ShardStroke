"""Print the class/property skeleton of an Alg member (first list element only).

usage: skeleton.py CONTAINER MEMBER [max_depth=6]
"""
import sys

import h5py

import alg


def sk(o, depth, maxd, name=""):
    pad = "  " * depth
    if isinstance(o, dict):
        keys = [k for k in o if k != "_class"]
        scal = {k: o[k] for k in keys if not isinstance(o[k], (dict, list))}
        s = ", ".join(f"{k}={str(v)[:24]}" for k, v in list(scal.items())[:8])
        print(f"{pad}{name} <{o.get('_class')}> {s}")
        if depth < maxd:
            for k in keys:
                v = o[k]
                if isinstance(v, (dict, list)) and v:
                    sk(v, depth + 1, maxd, k)
    elif isinstance(o, list):
        print(f"{pad}{name} [list x{len(o)}]")
        if o and depth < maxd and isinstance(o[0], (dict, list)):
            sk(o[0], depth + 1, maxd, "[0]")


def main():
    path, member = sys.argv[1], sys.argv[2]
    maxd = int(sys.argv[3]) if len(sys.argv) > 3 else 6
    b = h5py.File(path, "r")[member][()].tobytes()
    sk(alg.decode(b), 0, maxd)


if __name__ == "__main__":
    main()
