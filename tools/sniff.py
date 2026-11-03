"""Identify container type of each Painter data-file extension (one sample per extension)."""
import os
import sys

ROOT = r"C:\Program Files\Adobe\Adobe Substance 3D Painter\resources"
EXTS = ["spsm", "spmsk", "sppr", "spexp", "spt", "abrimg", "sbsar", "spi1d", "spi3d", "pkfx", "mdl", "abr"]

MAGIC = [
    (b"\x89HDF\r\n\x1a\n", "HDF5"),
    (b"7z\xbc\xaf\x27\x1c", "7z"),
    (b"PK\x03\x04", "ZIP"),
    (b"\x89PNG", "PNG"),
    (b"v/1\x01", "EXR"),
    (b"\x1f\x8b", "GZIP"),
    (b"{", "JSON?"),
    (b"<?xml", "XML"),
    (b"8BIM", "8BIM"),
    (b"8BPS", "PSD"),
]


def kind(head):
    for m, n in MAGIC:
        if head.startswith(m):
            return n
    return "?"


samples = {}
for dp, _, fns in os.walk(ROOT):
    for fn in fns:
        e = fn.rsplit(".", 1)[-1].lower() if "." in fn else ""
        if e in EXTS and e not in samples:
            samples[e] = os.path.join(dp, fn)

for e in EXTS:
    p = samples.get(e)
    if not p:
        print(f"{e:8} none")
        continue
    with open(p, "rb") as f:
        head = f.read(16)
    hexs = " ".join(f"{b:02X}" for b in head)
    asc = "".join(chr(b) if 32 <= b < 127 else "." for b in head)
    print(f"{e:8} {os.path.getsize(p):>10}  {kind(head):6} {hexs}  |{asc}|  {os.path.basename(p)}")
