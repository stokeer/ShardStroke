"""Import a Painter smart material / smart mask / preset into the ShardStroke layer model (JSON).

ShardStroke model (v0):
  Document: {"kind": "smart_material"|"smart_mask"|"preset"|"project", "name", "source",
             "layers": [Layer]}
  Layer:    {"type": "layer"|"group", "name", "visible", "blend": [{"mode","channels","opacity"}],
             "content": [Effect], "mask": {"enabled","background","effects":[Effect]} | None,
             "children": [Layer]}
  Effect:   {"type": fill|paint|eraser|smudge|group|filter|levels|anchor|decal|<other>,
             "visible", "channels": [..], ...type specific..., "children": [Effect] (group)}
  Source:   {"type": uniform|bitmap|procedural|reference|<other>, "channels", "opacity", ...}

usage: import_spsm.py FILE [OUT.json]       import one file
       import_spsm.py --report DIR          summarize every .spsm/.spmsk/.sppr under DIR
"""
import json
import os
import sys
from collections import Counter

import h5py

import alg
from to_json import clean

KIND = {".spsm": "smart_material", ".spmsk": "smart_mask", ".sppr": "preset", ".spt": "template", ".spp": "project"}
EFFECT = {
    "DataActionFill": "fill", "DataActionPaint": "paint", "DataActionEraser": "eraser",
    "DataActionSmudge": "smudge", "DataActionGroup": "group", "DataActionAnchor": "anchor",
    "DataActionFilterProcedural": "filter", "DataActionFilterLevels": "levels",
    "DataActionDecal": "decal", "DataActionClone": "clone",
}
SOURCE = {
    "DataSourceUniform": "uniform", "DataSourceBitmap": "bitmap",
    "DataSourceProcedural": "procedural", "DataSourceReference": "reference",
}
SKIP = {"_class", "uid", "uuid", "selection", "tags", "changeFlags"}


def visible(o):
    return bool(o.get("enabled", 255))


def blends(o):
    out = []
    for e in o.get("perChannelBlending") or []:
        out.append({"mode": e.get("blendingMode"), "channels": e.get("channelTypes"),
                    "opacity": e.get("opacity")})
    return out


def tweaks(lst):
    d = {}
    for t in lst or []:
        d[t.get("identifier")] = t.get("value")
    return d


def procedural(p):
    if not p:
        return None
    return {
        "resource": p.get("urlToSbsRes"),
        "outputs": [{"id": o.get("identifier"), "alpha": o.get("identifierAlpha"),
                     "channels": o.get("channelTypes"), "colorspace": o.get("colorSpace")}
                    for o in p.get("outputs") or []],
        "tweaks": tweaks(p.get("tweaks")),
        "inputs": [{"id": i.get("identifier"), "channel_type": i.get("channelType"),
                    "colorspace": i.get("colorSpace")} for i in p.get("inputs") or []],
        "input_sources": [source(s.get("source")) for s in p.get("sourcesInputs") or [] if s.get("source")],
        "output_alpha": p.get("outputIdentifierAlpha"),
    }


def source(s):
    if not s:
        return None
    t = SOURCE.get(s.get("_class"), s.get("_class"))
    out = {"type": t, "channels": s.get("channelTypes"), "opacity": s.get("opacity")}
    if t == "uniform":
        out["color"] = s.get("color")
    elif t == "bitmap":
        b = s.get("bitmap") or {}
        out["resource"] = b.get("urlToBitmapRes")
        out["colorspace"] = b.get("colorSpace")
    elif t == "procedural":
        out["procedural"] = procedural(s.get("procedural"))
    else:
        out["raw"] = {k: v for k, v in s.items() if k not in SKIP}
    return out


def effect(a):
    c = a.get("_class")
    t = EFFECT.get(c, c)
    out = {"type": t, "visible": visible(a), "channels": a.get("channelTypes"),
           "label": a.get("label") or None}
    if t == "fill":
        out.update(
            projection=a.get("projection"), filtering=a.get("filtering"),
            uv={"scale": a.get("uvScale"), "position": a.get("uvPosition"),
                "rotation": a.get("uvRotation"), "wrap": a.get("uvSamplingWrap")},
            sources=[source(s) for s in a.get("sources") or []],
            blend=blends(a))
    elif t == "filter":
        out.update(procedural=procedural(a.get("procedural")),
                   inputs=[source(s.get("source")) for s in a.get("inputs") or [] if isinstance(s, dict) and s.get("source")])
    elif t == "group":
        out["children"] = [effect(x) for x in ((a.get("subStack") or {}).get("items") or [])]
        out["blend"] = blends(a)
    elif t in ("paint", "eraser", "smudge"):
        out["strokes"] = len(a.get("strokes") or [])
        out["raw"] = {k: v for k, v in a.items() if k not in SKIP and k != "strokes"}
    else:
        out["raw"] = {k: v for k, v in a.items() if k not in SKIP}
    return out


def layer(o):
    c = o.get("_class")
    sub = (o.get("subStack") or {}).get("items") or []
    mask_items = ((o.get("maskActions") or {}).get("items")) or []
    mask = None
    if o.get("maskActions") is not None or o.get("maskEnabled") is not None:
        mask = {"enabled": bool(o.get("maskEnabled", 0)), "initial": o.get("maskInitial"),
                "effects": [effect(x) for x in mask_items]}
    return {
        "type": "group" if c == "DataLayerGroup" else "layer",
        "name": o.get("label"),
        "visible": visible(o),
        "blend": blends(o),
        "content": [effect(x) for x in ((o.get("actions") or {}).get("items") or [])],
        "mask": mask,
        "children": [layer(x) for x in sub],
    }


def import_file(path):
    ext = os.path.splitext(path)[1].lower()
    with h5py.File(path, "r") as f:
        member = None
        for cand in ("preset.bin", "paint/document.bin", "preset_2.bin", "preset_1.bin"):
            if cand in f:
                try:
                    root = clean(alg.decode(f[cand][()].tobytes()))
                    member = cand
                    break
                except alg.AlgError:
                    continue
        if member is None:
            raise alg.AlgError("no decodable preset member")
    doc = {"kind": KIND.get(ext, ext), "name": os.path.splitext(os.path.basename(path))[0],
           "source": os.path.basename(path), "member": member, "layers": []}
    if root.get("_class") == "DataLayersPreset":
        doc["layers"] = [layer(root["layerGroup"])] if root.get("layerGroup") else []
        doc["preset_uuid"] = root.get("uuid")
    elif root.get("_class") == "DataDocument":
        for m in root.get("materials") or []:
            for s in m.get("stacks") or []:
                doc["layers"] += [layer(x) for x in (s.get("stack") or {}).get("items") or []]
    else:
        doc["raw_root_class"] = root.get("_class")
        doc["raw"] = root
    return doc


def walk_stats(layers, st):
    for l in layers:
        st["layers"] += 1
        st["groups"] += l["type"] == "group"
        for e in l["content"] + ((l.get("mask") or {}).get("effects") or []):
            effect_stats(e, st)
        walk_stats(l["children"], st)


def effect_stats(e, st):
    st["effects"][e["type"]] += 1
    for s in e.get("sources") or []:
        if s:
            st["sources"][s["type"]] += 1
            if s["type"] == "procedural" and s["procedural"] and s["procedural"]["resource"]:
                st["procedurals"][s["procedural"]["resource"].split("?")[0].split("/")[-1]] += 1
            if s["type"] == "bitmap":
                st["bitmaps"] += 1
    p = e.get("procedural")
    if p and p.get("resource"):
        st["procedurals"][p["resource"].split("?")[0].split("/")[-1]] += 1
    for c in e.get("children") or []:
        effect_stats(c, st)


def report(root):
    files = []
    for dp, _, fns in os.walk(root):
        files += [os.path.join(dp, x) for x in fns if x.lower().endswith((".spsm", ".spmsk", ".sppr"))]
    st = {"layers": 0, "groups": 0, "effects": Counter(), "sources": Counter(), "procedurals": Counter(),
          "bitmaps": 0}
    ok = bad = 0
    fails = []
    for fp in files:
        try:
            walk_stats(import_file(fp)["layers"], st)
            ok += 1
        except Exception as e:  # noqa: BLE001
            bad += 1
            fails.append((os.path.basename(fp), str(e)[:80]))
    print(f"files={len(files)} ok={ok} fail={bad}")
    print(f"layers={st['layers']} groups={st['groups']} bitmaps={st['bitmaps']}")
    print("effects:", dict(st["effects"].most_common()))
    print("sources:", dict(st["sources"].most_common()))
    print(f"procedurals referenced: {len(st['procedurals'])} distinct")
    for n, c in st["procedurals"].most_common(25):
        print(f"   {c:5} {n}")
    for f, m in fails[:10]:
        print("FAIL", f, m)


def main():
    a = sys.argv[1:]
    if a and a[0] == "--report":
        report(a[1])
        return
    doc = import_file(a[0])
    s = json.dumps(doc, indent=1, ensure_ascii=False)
    if len(a) > 1:
        open(a[1], "w", encoding="utf-8").write(s)
        print("wrote", a[1])
    else:
        print(s[:6000])


if __name__ == "__main__":
    main()
