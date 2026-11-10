"""Parse Painter's `paint/scene.assbin` (Assimp binary-dump variant) into meshes.

Observed layout (Painter 12.1):
  header   : 44-byte id "ASSIMP.binary-dump.<date>\n", u32 major, u32 minor, u32 rev, u16 flags,
             u16 shortened, u16 compressed, u16 pad, 256 filename, 128 options, 64 reserved  (=512 bytes)
  scene    : nodes tree (chunk id 8: name, 4x4 matrix, metadata e.g. ALG_PRE_NORMALIZATION_*) ...
  mesh     : u32 0x1289 marker, u32 0, string name (u32 len + bytes),
             u32 primitiveTypes (4 = triangles), u32 ?, u32 flags, u32 numVertices,
             arrays of numVertices x vec3<f32> in this order (by flag bits):
               0x001 positions, 0x002 normals / 0x004 tangents+bitangents: stored as
               positions, tangent?, bitangent?, normal (verified by unit-length test), then 8 bytes,
               0x100 texcoords vec3 (u,v,0)
             then u32 numFaces, u32 1, numFaces x [u32 3, i0, i1, i2, u32 flag]

usage: assbin.py FILE.spp|scene.assbin [outdir]   -> prints meshes, writes OBJ per mesh
"""
import math
import os
import struct
import sys

MARK = 0x1289


class Mesh:
    def __init__(self, name, pos, uv, normals, faces):
        self.name, self.pos, self.uv, self.normals, self.faces = name, pos, uv, normals, faces


def read_scene(b):
    return b[0x200:] if b.startswith(b"ASSIMP.binary-dump") else b


def vec3s(b, o, n):
    return [struct.unpack_from("<3f", b, o + 12 * i) for i in range(n)]


def length(v):
    return math.sqrt(sum(x * x for x in v))


def parse_meshes(b):
    meshes = []
    i = 0
    n_total = len(b)
    while i + 16 < n_total:
        # mesh record: [u32 chunk_length][u32 0][u32 name_len][name][u32 prim=4]...
        if struct.unpack_from("<I", b, i + 4)[0] != 0:
            i += 4
            continue
        p = i + 8
        try:
            nlen = struct.unpack_from("<I", b, p)[0]
            if nlen == 0 or nlen > 256:
                i += 4
                continue
            raw = b[p + 4:p + 4 + nlen]
            if not all(32 <= c < 127 for c in raw):
                i += 4
                continue
            name = raw.decode("ascii")
            p += 4 + nlen
            prim, _unk, flags, nv = struct.unpack_from("<4I", b, p)
            p += 16
            if prim != 4 or nv == 0 or nv > 5_000_000 or not (flags & 1):
                i += 4
                continue
            # positions directly after nv; every following array is preceded by a u32 count (== nv)
            arrays = [vec3s(b, p, nv)]
            p += 12 * nv
            narr = (1 if flags & 2 else 0) + (2 if flags & 4 else 0)
            ok = True
            for _ in range(narr):
                if struct.unpack_from("<I", b, p)[0] != nv:
                    ok = False
                    break
                p += 4
                arrays.append(vec3s(b, p, nv))
                p += 12 * nv
            if not ok:
                i += 4
                continue
            # order: normals, tangents, bitangents
            normals = arrays[1] if (flags & 2) and len(arrays) > 1 else None
            uv = None
            if flags & 0x100:
                # u32 numComponents, u32 count, then vec3 (u, v, 0)
                if struct.unpack_from("<I", b, p + 4)[0] != nv:
                    i += 4
                    continue
                p += 8
                uv = vec3s(b, p, nv)
                p += 12 * nv
            # locate numFaces (u32) followed by 1 and a run of [3,i0,i1,i2,f]
            q = p
            found = None
            for q in range(p, min(p + 64, len(b) - 8), 4):
                nf, one = struct.unpack_from("<II", b, q)
                if one == 1 and 0 < nf < 10_000_000:
                    n3 = struct.unpack_from("<I", b, q + 8)[0]
                    if n3 == 3:
                        found = (q, nf)
                        break
            if not found:
                i += 4
                continue
            q, nf = found
            faces = []
            r = q + 8
            for _ in range(nf):
                n, a, c, d, f = struct.unpack_from("<5I", b, r)
                if n != 3 or max(a, c, d) >= nv:
                    break
                faces.append((a, c, d))
                r += 20
            if len(faces) != nf:
                i += 4
                continue
            mm = Mesh(name, arrays[0], uv, normals, faces)
            mm.extra = arrays[1:]
            meshes.append(mm)
            i = r
        except struct.error:
            break
    return meshes


def to_obj(m, path):
    with open(path, "w") as f:
        f.write(f"o {m.name}\n")
        for v in m.pos:
            f.write("v %.6f %.6f %.6f\n" % v)
        if m.uv:
            for v in m.uv:
                f.write("vt %.6f %.6f\n" % (v[0], v[1]))
        if m.normals:
            for v in m.normals:
                f.write("vn %.6f %.6f %.6f\n" % v)
        for a, b, c in m.faces:
            def t(i):
                i += 1
                s = str(i)
                if m.uv and m.normals:
                    return f"{i}/{i}/{i}"
                if m.uv:
                    return f"{i}/{i}"
                return s
            f.write(f"f {t(a)} {t(b)} {t(c)}\n")


def main():
    path = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else None
    if path.lower().endswith(".spp"):
        import h5py
        b = h5py.File(path, "r")["paint/scene.assbin"][()].tobytes()
    else:
        b = open(path, "rb").read()
    ms = parse_meshes(read_scene(b))
    print(f"{len(ms)} meshes")
    for m in ms:
        lo = [min(v[k] for v in m.pos) for k in range(3)]
        hi = [max(v[k] for v in m.pos) for k in range(3)]
        print(f"  {m.name:20} v={len(m.pos):6} f={len(m.faces):6} uv={'y' if m.uv else 'n'} "
              f"n={'y' if m.normals else 'n'} bbox={[round(x,3) for x in lo]}..{[round(x,3) for x in hi]}")
        if out:
            os.makedirs(out, exist_ok=True)
            to_obj(m, os.path.join(out, m.name + ".obj"))


if __name__ == "__main__":
    main()
