"""Alg reflective-serialization decoder.

Model (reverse-engineered from data files):
  header  : magic u32 | ver | v2 | v3 | nclasses | filesize              (u32 LE)
  class   : marker 0xFFFFFFFF -> name, nprops, props[(name,type)]   (defined INLINE at first use,
            gets next class index); otherwise marker is the class index.
  object  : fields in class property order, by type:
              0x09 i32 | 0x0A bool(u8) | 0x0C uid(u64) | 0x10 bytes(u32 len + data)
              0x11 list : u32 count, [class marker once if count>0], elements (monomorphic)
              0x13 list : u32 count, per element: u32 end_offset, class marker, element (polymorphic)
  QVariant (inside 0x10 bytes) : u32 BE type | u8 isNull | payload BE

usage: alg_decode.py FILE [-q]
"""
import json
import struct
import sys

MAGIC = 0x1B7C2FDD
NEWCLASS = 0xFFFFFFFF


class Fail(Exception):
    pass


class Decoder:
    # type -> byte size (variant A numbering, fitted over starter_assets by end-offset consistency;
    # 401/401 members decode). 0x02 vec2f, 0x03 vec3f, 0x04 vec4f, 0x05 i32, 0x06 int2,
    # 0x0B u32 (BE flags), 0x0D mat3f, 0x0E mat4f, 0x0F ref (u64), 0x15 u64, 0x16 bitset128
    FIXED = {0x02: 8, 0x03: 12, 0x04: 16, 0x05: 4, 0x06: 8, 0x0B: 4, 0x0D: 36, 0x0E: 64,
             0x0F: 8, 0x15: 8, 0x16: 16}

    def __init__(self, b):
        self.b = b
        self.p = 0
        self.unknown = {}
        self.warn = []
        self.by_idx = []       # class index -> (name, props)

    def u32(self):
        if self.p + 4 > len(self.b):
            raise Fail(f"eof u32 @{self.p:#x}")
        v = struct.unpack_from("<I", self.b, self.p)[0]
        self.p += 4
        return v

    def i32(self):
        if self.p + 4 > len(self.b):
            raise Fail(f"eof i32 @{self.p:#x}")
        v = struct.unpack_from("<i", self.b, self.p)[0]
        self.p += 4
        return v

    def raw(self, n):
        if self.p + n > len(self.b):
            raise Fail(f"eof raw {n} @{self.p:#x}")
        v = self.b[self.p:self.p + n]
        self.p += n
        return v

    def bytes_(self):
        n = self.u32()
        if n > 1 << 24:
            raise Fail(f"bad len {n} @{self.p - 4:#x}")
        return self.raw(n)

    def class_marker(self):
        m = self.u32()
        if m == NEWCLASS:
            name = self.bytes_().decode("ascii", "replace")
            n = self.u32()
            if n > 256:
                raise Fail(f"nprops {n}")
            props = [(self.bytes_().decode("ascii", "replace"), self.u32()) for _ in range(n)]
            self.by_idx.append((name, props))
            return len(self.by_idx) - 1
        if m >= len(self.by_idx):
            raise Fail(f"unknown class idx {m} @{self.p - 4:#x}")
        return m

    def object(self, idx):
        name, props = self.by_idx[idx]
        obj = {"_class": name}
        for pn, pt in props:
            obj[pn] = self.value(pn, pt)
        return obj

    def value(self, pn, pt):
        if pt == 0x09:
            return self.i32()
        if pt == 0x0A:
            return self.raw(1)[0]
        if pt == 0x0C:
            return struct.unpack("<Q", self.raw(8))[0]
        if pt == 0x10:
            return decode_bytes(self.bytes_())
        if pt == 0x01:
            return struct.unpack("<f", self.raw(4))[0]
        if pt in (0x12, 0x14):                # nested object (0x14: embedded / non-nullable)
            end = self.u32()                  # absolute end offset of the nested object; 0 = null
            if end == 0:
                return None
            o = self.object(self.class_marker())
            if end != self.p:
                self.warn.append(f"{pn} end offset {end:#x} != pos {self.p:#x}")
            return o
        if pt == 0x11:
            n = self.u32()
            if n > 1 << 20:
                raise Fail(f"list too long {n} @{self.p - 4:#x}")
            if n == 0:
                return []
            idx = self.class_marker()
            return [self.object(idx) for _ in range(n)]
        if pt == 0x13:
            n = self.u32()
            if n > 1 << 20:
                raise Fail(f"list too long {n} @{self.p - 4:#x}")
            out = []
            for i in range(n):
                end = self.u32()
                idx = self.class_marker()
                o = self.object(idx)
                if end != self.p:
                    self.warn.append(f"{pn}[{i}] end offset {end:#x} != pos {self.p:#x}")
                out.append(o)
            return out
        if pt in Decoder.FIXED:
            return "raw:" + self.raw(Decoder.FIXED[pt]).hex()
        self.unknown.setdefault(pt, pn)
        raise Fail(f"unknown type {pt:#x} for {pn} @{self.p:#x}")

    def run(self):
        if self.u32() != MAGIC:
            raise Fail("magic")
        self.hdr = [self.u32() for _ in range(5)]
        idx = self.class_marker()
        return self.object(idx)


def decode_bytes(bs):
    """QVariant (BE) / ascii text / hex."""
    if len(bs) >= 5 and bs[4] == 0:
        t = struct.unpack(">I", bs[:4])[0]
        if t == 3 and len(bs) == 9:
            return {"uint": struct.unpack(">I", bs[5:9])[0]}
        if t == 10 and len(bs) >= 9:
            n = struct.unpack(">I", bs[5:9])[0]
            if 9 + n == len(bs):
                return {"str": bs[9:].decode("utf-16-be", "replace")}
    try:
        s = bs.decode("utf-8")
        if all(32 <= ord(c) < 127 for c in s):
            return s
    except UnicodeDecodeError:
        pass
    return "hex:" + bs.hex()


def decode_file(path):
    b = open(path, "rb").read()
    d = Decoder(b)
    root = d.run()
    return root, d, len(b)


def main():
    root, d, n = decode_file(sys.argv[1])
    print(f"parsed {d.p}/{n} warnings={len(d.warn)}")
    for w in d.warn[:5]:
        print("  warn:", w)
    if "-q" not in sys.argv:
        print(json.dumps(root, indent=1)[:6000])


if __name__ == "__main__":
    main()
