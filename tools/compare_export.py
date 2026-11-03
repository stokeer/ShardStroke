"""Compare composed Base color views (SHARD_VIEW_DIR dump) with Painter's exported *_BCR.tga.

usage: compare_export.py VIEW_DIR [EXPORT_DIR ...]
"""
import glob
import os
import sys

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
DEFAULT = os.environ.get("SHARD_EXPORT_DIR", ".")


def truth_files(dirs):
    out = {}
    for d in dirs:
        for f in glob.glob(os.path.join(d, "*_BCR*.tga")):
            name = os.path.basename(f).split("_BCR")[0]
            # prefer the most recently written export of a set
            if name not in out or os.path.getmtime(f) > os.path.getmtime(out[name]):
                out[name] = f
    return out


def main():
    view_dir = sys.argv[1]
    dirs = sys.argv[2:] or [os.path.join(DEFAULT, "ExportedTexture"), os.path.join(DEFAULT, "Interior")]
    truth = truth_files(dirs)
    print(f"{'set':26} {'truth RGB':>16} {'mine RGB':>16} {'MAE':>6} {'corr':>6}")
    for f in sorted(glob.glob(os.path.join(view_dir, "*.png"))):
        name = os.path.splitext(os.path.basename(f))[0]
        if name not in truth:
            continue
        t = np.asarray(Image.open(truth[name]).convert("RGB").resize((512, 512), Image.BILINEAR)).astype(float)
        m = np.asarray(Image.open(f).convert("RGB").resize((512, 512), Image.BILINEAR)).astype(float)
        mae = np.abs(t - m).mean()
        tl, ml = t.mean(2).ravel(), m.mean(2).ravel()
        corr = np.corrcoef(tl, ml)[0, 1] if tl.std() > 0 and ml.std() > 0 else float("nan")
        tm = t.reshape(-1, 3).mean(0).round().astype(int)
        mm = m.reshape(-1, 3).mean(0).round().astype(int)
        print(f"{name:26} {str(tuple(tm)):>16} {str(tuple(mm)):>16} {mae:6.1f} {corr:6.2f}")


main()
