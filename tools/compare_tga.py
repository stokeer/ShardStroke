"""Compare ShardStroke exports (BCR/NMO .tga) with Painter's exports, channel by channel.

usage: compare_tga.py MINE_DIR TRUTH_DIR [set ...]
"""
import os
import sys

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
N = 512
LABELS = {"BCR": ["R", "G", "B", "rough"], "NMO": ["nrmX", "nrmY", "metal", "AO"]}


def load(path):
    return np.asarray(Image.open(path).convert("RGBA").resize((N, N), Image.BILINEAR)).astype(float)


def main():
    mine_dir, truth_dir = sys.argv[1], sys.argv[2]
    sets = sys.argv[3:]
    if not sets:
        sets = sorted({f.rsplit("_", 1)[0] for f in os.listdir(truth_dir) if f.endswith(".tga")})
    print(f"{'set':22} {'map':4} {'chan':6} {'truth':>7} {'mine':>7} {'MAE':>6} {'corr':>6}")
    for s in sets:
        for kind in ("BCR", "NMO"):
            a, b = os.path.join(mine_dir, f"{s}_{kind}.tga"), os.path.join(truth_dir, f"{s}_{kind}.tga")
            if not (os.path.exists(a) and os.path.exists(b)):
                continue
            m, t = load(a), load(b)
            for c, lab in enumerate(LABELS[kind]):
                mc, tc = m[:, :, c].ravel(), t[:, :, c].ravel()
                # compare only where the Painter export has content (A > 0 or any channel > 0)
                mask = (t.sum(2).ravel() > 0)
                if mask.sum() < 100:
                    continue
                mae = np.abs(mc[mask] - tc[mask]).mean()
                corr = np.corrcoef(mc[mask], tc[mask])[0, 1] if mc[mask].std() > 0 and tc[mask].std() > 0 else float("nan")
                print(f"{s:22} {kind:4} {lab:6} {tc[mask].mean():7.1f} {mc[mask].mean():7.1f} {mae:6.1f} {corr:6.2f}")


main()
