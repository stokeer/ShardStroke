"""Export Alg members of a Painter container as clean JSON (enums resolved, raw blobs decoded).

usage: to_json.py CONTAINER OUTDIR [--member NAME ...]
"""
import json
import os
import struct
import sys

import h5py

import alg
from enums import BLENDING_MODE, PROJECTION_MODE, mask_to_channels

CHANNEL_MASK_KEYS = {"channelTypes"}
VEC_FLOATS = {"dddEulerAngles", "dddPosition", "dddSize", "uvPosition", "uvScale", "color"}


def _floats(hexs):
    b = bytes.fromhex(hexs)
    return list(struct.unpack(f"<{len(b) // 4}f", b))


def clean(o, key=None, cls=None):
    if isinstance(o, dict):
        c = o.get("_class")
        out = {"_class": c} if c else {}
        for k, v in o.items():
            if k == "_class":
                continue
            out[k] = clean(v, k, c)
        return out
    if isinstance(o, list):
        return [clean(v, key, cls) for v in o]
    if isinstance(o, str) and o.startswith("raw:"):
        raw = o[4:]
        if key in CHANNEL_MASK_KEYS:
            return mask_to_channels(o)
        if key in VEC_FLOATS or (cls and key in ("color", "backgroundColor")):
            return _floats(raw)
        if key == "value" and cls in ("DataTweakInt2",):
            return list(struct.unpack("<2i", bytes.fromhex(raw)))
        if key == "value" and cls in ("DataTweakInt",):
            return struct.unpack("<i", bytes.fromhex(raw))[0]
        return o
    if key in CHANNEL_MASK_KEYS and isinstance(o, int):
        return mask_to_channels(o)
    if key == "blendingMode" and isinstance(o, int):
        return BLENDING_MODE.get(o, o)
    if key == "projection" and isinstance(o, int) and cls == "DataActionFill":
        return PROJECTION_MODE.get(o, o)
    if isinstance(o, dict) and "uint" in o:
        return o["uint"]
    return o


def main():
    a = sys.argv[1:]
    path, outdir = a[0], a[1]
    only = [a[i + 1] for i, x in enumerate(a) if x == "--member"]
    os.makedirs(outdir, exist_ok=True)
    n = 0
    with h5py.File(path, "r") as f:
        names = []
        f.visititems(lambda nm, o: names.append(nm) if isinstance(o, h5py.Dataset) else None)
        for nm in names:
            if only and nm not in only:
                continue
            ds = f[nm]
            if ds.dtype.kind != "u" or ds.dtype.itemsize != 1 or ds.size < 12:
                continue
            b = ds[()].tobytes()
            if not alg.is_alg(b):
                continue
            try:
                obj = clean(alg.decode(b))
            except alg.AlgError as e:
                print("skip", nm, e)
                continue
            fp = os.path.join(outdir, nm.replace("/", "__") + ".json")
            with open(fp, "w", encoding="utf-8") as fh:
                json.dump(obj, fh, indent=1, ensure_ascii=False)
            n += 1
            print("wrote", fp)
    print(n, "members exported")


if __name__ == "__main__":
    main()
