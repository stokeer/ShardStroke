"""Print the layer stack of a Painter project / template (document.bin) with enum names.

usage: stack.py CONTAINER [member=paint/document.bin]
"""
import sys

import h5py

import alg
from enums import BLENDING_MODE, PROJECTION_MODE, mask_to_channels


def bl(o):
    out = []
    for e in o.get("perChannelBlending") or []:
        chs = ",".join(mask_to_channels(e.get("channelTypes"))) or "?"
        mode = BLENDING_MODE.get(e.get("blendingMode"), f"#{e.get('blendingMode')}")
        out.append(f"{mode}@{chs} op={e.get('opacity'):.2f}")
    return "; ".join(out)


def layer(o, depth):
    pad = "  " * depth
    c = o.get("_class", "?")
    on = "" if o.get("enabled") else " (hidden)"
    print(f"{pad}- {c.replace('Data', '')} '{o.get('label', '')}'{on} {bl(o)}")
    for k in ("actions", "maskActions"):
        a = o.get(k)
        if a:
            for it in a.get("items", []):
                effect(it, depth + 2, "content" if k == "actions" else "mask")
    sub = o.get("subStack")
    if sub:
        for it in sub.get("items", []):
            layer(it, depth + 1)


def effect(o, depth, kind):
    pad = "  " * depth
    c = o.get("_class", "?").replace("DataAction", "")
    extra = ""
    if c == "Fill":
        srcs = [s.get("_class", "?").replace("DataSource", "") for s in o.get("sources", [])]
        extra = (f"proj={PROJECTION_MODE.get(o.get('projection'), o.get('projection'))} "
                 f"src={srcs} ch={','.join(mask_to_channels(o.get('channelTypes')))[:60]}")
    elif c in ("Paint", "Eraser", "Smudge"):
        extra = f"strokes={len(o.get('strokes') or [])}"
    off = "" if o.get("enabled") else " (off)"
    print(f"{pad}[{kind}] {c}{off} {extra}")
    if c == "Group":
        for it in (o.get("items") or []):
            effect(it, depth + 1, kind)


def main():
    path = sys.argv[1]
    member = sys.argv[2] if len(sys.argv) > 2 else "paint/document.bin"
    doc = alg.decode(h5py.File(path, "r")[member][()].tobytes())
    for m in doc.get("materials", []):
        print(f"MATERIAL {m.get('sceneMaterialName')}")
        for s in m.get("stacks", []):
            print(f"  STACK shader={s.get('shaderModel')} channels="
                  f"{[c.get('type') for c in s.get('channels', [])][:10]}...")
            for it in (s.get("stack") or {}).get("items", []):
                layer(it, 2)


if __name__ == "__main__":
    main()
