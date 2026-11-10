"""Fit byte sizes of unknown variant-A (ver=1) value types over a corpus.

usage: fit_a.py PATH [...]
"""
import os
import struct
import sys

import h5py

from alg_decode import Decoder, Fail

MAGIC = 0x1B7C2FDD
EXTS = (".spsm", ".spmsk", ".sppr", ".spt", ".spp")
CAND = [1, 2, 4, 8, 12, 16, 24, 32, 36, 48, 64]


def load(paths, limit=200, max_bytes=4_000_000):
    out = []
    for a in paths:
        files = []
        if os.path.isdir(a):
            for dp, _, fns in os.walk(a):
                files += [os.path.join(dp, x) for x in fns if x.lower().endswith(EXTS)]
        else:
            files.append(a)
        for fp in files:
            try:
                with h5py.File(fp, "r") as f:
                    names = []
                    f.visititems(lambda n, o: names.append(n) if isinstance(o, h5py.Dataset) else None)
                    for n in names:
                        ds = f[n]
                        if ds.dtype.kind == "u" and 12 <= ds.size <= max_bytes:
                            b = ds[()].tobytes()
                            if struct.unpack_from("<I", b)[0] == MAGIC and struct.unpack_from("<I", b, 4)[0] == 1:
                                out.append((f"{os.path.basename(fp)}:{n}", b))
            except OSError:
                pass
            if len(out) >= limit:
                return out
    return out


def score(blobs):
    ok = parsed = 0
    unknown = {}
    for _, b in blobs:
        d = Decoder(b)
        try:
            d.run()
            if d.p == len(b) and not d.warn:
                ok += 1
        except Fail:
            for t, pn in d.unknown.items():
                unknown.setdefault(t, [0, pn])[0] += 1
        parsed += d.p
    return ok, parsed, unknown


def main():
    blobs = load(sys.argv[1:])
    print("blobs:", len(blobs))
    Decoder.FIXED = {}
    for _ in range(14):
        ok, parsed, unknown = score(blobs)
        print(f"state ok={ok}/{len(blobs)} parsed={parsed} unknown={ {hex(k): v for k, v in unknown.items()} } fixed={ {hex(k): v for k, v in Decoder.FIXED.items()} }")
        if not unknown:
            break
        t = max(unknown, key=lambda k: unknown[k][0])
        best = None
        for s in CAND:
            Decoder.FIXED[t] = s
            o2, p2, _ = score(blobs)
            if best is None or (o2, p2) > best[0]:
                best = ((o2, p2), s)
        Decoder.FIXED[t] = best[1]
        print(f"  -> type {t:#04x} (e.g. {unknown[t][1]}): size {best[1]} (ok={best[0][0]} parsed={best[0][1]})")


if __name__ == "__main__":
    main()
