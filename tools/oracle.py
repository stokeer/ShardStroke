#!/usr/bin/env python3
"""Differential-testing oracle: render a `.sbsar` with Substance's own command-line renderer.

`sbsrender` ships with the user's licensed Substance 3D Designer. It is used here only as a black box: render a
graph with chosen parameters and compare the picture with ShardStroke's evaluation. No executable is analysed or
redistributed (README, "Rules").

    python tools/oracle.py GRAPH.sbsar OUT.png [--size 11] [--set name@value ...] [--output NAME]

`--size` is the log2 of the side (11 = 2048). Importable: `render(sbsar, out_png, size=11, values={...})`.
"""
import glob
import os
import subprocess
import sys
import tempfile

CANDIDATES = [
    r"C:\Program Files\Adobe\Adobe Substance 3D Designer\sbsrender.exe",
    r"C:\Program Files\Adobe\Adobe Substance 3D Painter\sbsrender.exe",
]


def sbsrender():
    env = os.environ.get("SBSRENDER")
    if env and os.path.exists(env):
        return env
    for c in CANDIDATES:
        if os.path.exists(c):
            return c
    raise SystemExit("sbsrender not found (set SBSRENDER)")


def render(sbsar, out_png, size=11, values=None, output=None, bit_depth="8", seed=None):
    """Render `sbsar` to `out_png`. `values`: {"balance": 0.3, "$randomseed": 5}. Returns the PNG path."""
    values = dict(values or {})
    values.setdefault("$outputsize", f"{size},{size}")
    if seed is not None:
        values["$randomseed"] = seed
    out_dir = tempfile.mkdtemp(prefix="oracle_")
    cmd = [sbsrender(), "--quiet", "render", "--input", sbsar, "--output-path", out_dir, "--output-name", "out",
           "--output-format", "png", "--output-bit-depth", bit_depth, "--no-report"]
    if output:
        cmd += ["--input-graph-output", output]
    for k, v in values.items():
        cmd += ["--set-value", f"{k}@{v}"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    files = glob.glob(os.path.join(out_dir, "*.png"))
    if r.returncode != 0 or not files:
        raise RuntimeError(f"sbsrender failed ({r.returncode}): {r.stdout[-300:]} {r.stderr[-300:]}")
    os.replace(files[0], out_png)
    return out_png


def main():
    a = sys.argv[1:]
    if len(a) < 2:
        print(__doc__)
        return 1
    sbsar, out = a[0], a[1]
    size, values, output = 11, {}, None
    i = 2
    while i < len(a):
        if a[i] == "--size":
            size = int(a[i + 1]); i += 2
        elif a[i] == "--set":
            k, v = a[i + 1].split("@", 1); values[k] = v; i += 2
        elif a[i] == "--output":
            output = a[i + 1]; i += 2
        else:
            i += 1
    print(render(sbsar, out, size, values, output))
    return 0


if __name__ == "__main__":
    sys.exit(main())
