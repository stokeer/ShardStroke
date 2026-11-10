"""Unified Alg decoder: picks variant A (ver=1) or B (ver=0) from the header.

    from alg import decode
    obj = decode(bytes)            # nested dicts/lists; objects carry "_class"

CLI: alg.py FILE|container.h5 [member]
"""
import json
import struct
import sys

import alg_b
import alg_decode

MAGIC = 0x1B7C2FDD


class AlgError(Exception):
    pass


def is_alg(b):
    return len(b) >= 12 and struct.unpack_from("<I", b)[0] == MAGIC


def decode(b):
    if not is_alg(b):
        raise AlgError("not an Alg blob")
    ver = struct.unpack_from("<I", b, 4)[0]
    try:
        if ver == 1:
            d = alg_decode.Decoder(b)
            root = d.run()
        elif ver == 0:
            d = alg_b.B(b)
            root = d.run()
        else:
            raise AlgError(f"unknown version {ver}")
    except (alg_decode.Fail, alg_b.Fail) as e:
        raise AlgError(str(e)) from e
    if d.p != len(b):
        raise AlgError(f"trailing bytes: parsed {d.p}/{len(b)}")
    return root


def main():
    path = sys.argv[1]
    if path.lower().endswith((".spsm", ".spmsk", ".sppr", ".spt", ".spp")):
        import h5py
        with h5py.File(path, "r") as f:
            b = f[sys.argv[2]][()].tobytes()
    else:
        b = open(path, "rb").read()
    print(json.dumps(decode(b), indent=1)[:8000])


if __name__ == "__main__":
    main()
