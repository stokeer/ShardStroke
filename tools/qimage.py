"""Decode Painter `.image` resources: a Qt QDataStream (big-endian) QVariantMap.

  u32 count, then count x { QString key (u32 byte-len + UTF-16BE), QVariant value }
  QVariant: u32 type, u8 isNull, payload (Qt type ids: 1 Bool u8, 2 Int i32, 3 UInt u32,
            4 LongLong i64, 5 ULongLong u64, 6 Double f64, 10 String, 12 ByteArray (u32 len + bytes))

usage: qimage.py FILE.spp [N]   -> prints header keys of the first N image resources
"""
import struct
import sys


class QReader:
    def __init__(self, b):
        self.b, self.p = b, 0

    def u32(self):
        v = struct.unpack_from(">I", self.b, self.p)[0]
        self.p += 4
        return v

    def qstring(self):
        n = self.u32()
        if n == 0xFFFFFFFF:
            return None
        s = self.b[self.p:self.p + n].decode("utf-16-be")
        self.p += n
        return s

    def qvariant(self):
        t = self.u32()
        null = self.b[self.p]
        self.p += 1
        b = self.b
        if t == 1:
            v = b[self.p] != 0
            self.p += 1
        elif t == 2:
            v = struct.unpack_from(">i", b, self.p)[0]
            self.p += 4
        elif t == 3:
            v = struct.unpack_from(">I", b, self.p)[0]
            self.p += 4
        elif t == 4:
            v = struct.unpack_from(">q", b, self.p)[0]
            self.p += 8
        elif t == 5:
            v = struct.unpack_from(">Q", b, self.p)[0]
            self.p += 8
        elif t == 6:
            v = struct.unpack_from(">d", b, self.p)[0]
            self.p += 8
        elif t == 10:
            v = self.qstring()
        elif t == 12:
            n = self.u32()
            v = (self.p, n)           # (offset, length) of the byte array, not copied
            self.p += 0 if n == 0xFFFFFFFF else n
        else:
            raise ValueError(f"unsupported QVariant type {t} at {self.p - 5}")
        return t, v


def read_map(b):
    r = QReader(b)
    n = r.u32()
    out = {}
    for _ in range(n):
        k = r.qstring()
        out[k] = r.qvariant()
    return out, r.p


def main():
    import h5py
    f = h5py.File(sys.argv[1], "r")
    res = f["resources"]
    names = [n for n in res if n.endswith(".image")]
    for n in names[: int(sys.argv[2]) if len(sys.argv) > 2 else 3]:
        b = res[n][()].tobytes()
        m, end = read_map(b)
        print(f"--- {n[:12]} size={len(b)} header_end={end}")
        for k, (t, v) in m.items():
            print(f"   {k:20} type={t:2} {v}")


if __name__ == "__main__":
    main()
