"""Decode a project made with painter_probe/enum_probe.py and print enum value tables.

usage: probe_decode.py PROJECT.spp
"""
import sys

import h5py

import alg


def mask_bits(v):
    if isinstance(v, str) and v.startswith("raw:"):
        v = int.from_bytes(bytes.fromhex(v[4:]), "little")
    return [i for i in range(128) if (v >> i) & 1] if isinstance(v, int) else []


def walk_layers(items, out):
    for it in items:
        out.append(it)
        sub = it.get("subStack")
        if sub:
            walk_layers(sub.get("items", []), out)


def main():
    doc = alg.decode(h5py.File(sys.argv[1], "r")["paint/document.bin"][()].tobytes())
    layers = []
    for m in doc["materials"]:
        for s in m["stacks"]:
            walk_layers((s.get("stack") or {}).get("items", []), layers)
    bm, pm, ch = {}, {}, {}
    for l in layers:
        label = l.get("label", "")
        if label.startswith("BM:"):
            e = (l.get("perChannelBlending") or [{}])[0]
            bm[label[3:]] = e.get("blendingMode")
        elif label.startswith("PM:"):
            for a in (l.get("actions") or {}).get("items", []):
                if a.get("_class") == "DataActionFill":
                    pm[label[3:]] = a.get("projection")
        elif label.startswith("CH:"):
            for e in l.get("perChannelBlending") or []:
                bits = mask_bits(e.get("channelTypes"))
                if bits:
                    ch[label[3:]] = bits
    print("BLENDING (name -> file value)")
    for k, v in sorted(bm.items(), key=lambda x: (x[1] is None, x[1])):
        print(f"  {v!s:>4}  {k}")
    print("PROJECTION")
    for k, v in sorted(pm.items(), key=lambda x: (x[1] is None, x[1])):
        print(f"  {v!s:>4}  {k}")
    print("CHANNEL TYPE (name -> bit index in channelTypes)")
    for k, v in sorted(ch.items(), key=lambda x: x[1]):
        print(f"  {v}  {k}")


if __name__ == "__main__":
    main()
