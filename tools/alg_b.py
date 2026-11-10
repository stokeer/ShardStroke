"""Alg variant B decoder (header ver 0): per-property tagged stream.

  header : magic u32 | ver=0 u32 | 2 u32 | root value
  value  : u8 type, payload:
     0x01 float32 LE | 0x09 int (8 bytes LE) | 0x0A bool u8 | 0x0B u32 BE
     0x10 bytes (u32 len + data)
     0x11 list   : u32 end, u32 count, count x element(value)
     0x12/0x14   : u8 flag (0=present,0xFF=null); if present: u32 end, class(u32 len+name), u16 nprops,
                   nprops x [name (u32 len+bytes), value]
usage: alg_b.py FILE
"""
import json
import struct
import sys

from alg_decode import decode_bytes

MAGIC = 0x1B7C2FDD


class Fail(Exception):
    pass


class B:
    # type -> byte size (fitted by end-offset consistency over the starter_assets corpus)
    # 0x02 vec2f, 0x04 vec4f, 0x05 i32, 0x06 int2, 0x0D mat3f (36), 0x0E mat4f (64, assumed)
    FIXED = {0x02: 8, 0x04: 16, 0x05: 4, 0x06: 8, 0x0D: 36, 0x0E: 64, 0x15: 8}

    def __init__(self, b):
        self.b, self.p, self.warn = b, 0, []
        self.unknown = {}

    def need(self, n):
        if self.p + n > len(self.b):
            raise Fail(f"eof need {n} @{self.p:#x}")

    def u8(self):
        self.need(1)
        v = self.b[self.p]
        self.p += 1
        return v

    def u16(self):
        self.need(2)
        v = struct.unpack_from("<H", self.b, self.p)[0]
        self.p += 2
        return v

    def u32(self):
        self.need(4)
        v = struct.unpack_from("<I", self.b, self.p)[0]
        self.p += 4
        return v

    def raw(self, n):
        self.need(n)
        v = self.b[self.p:self.p + n]
        self.p += n
        return v

    def name(self):
        n = self.u32()
        if n > 4096:
            raise Fail(f"name len {n} @{self.p - 4:#x}")
        return self.raw(n).decode("utf-8", "replace")

    def value(self, ctx=""):
        t = self.u8()
        return self.typed(t, ctx)

    def typed(self, t, ctx):
        if t == 0x01:
            return struct.unpack("<f", self.raw(4))[0]
        if t == 0x09:
            return struct.unpack("<q", self.raw(8))[0]
        if t == 0x0A:
            return self.u8()
        if t == 0x0B:
            return struct.unpack(">I", self.raw(4))[0]
        if t == 0x10:
            n = self.u32()
            if n > 1 << 26:
                raise Fail(f"bytes len {n} @{self.p - 4:#x}")
            return decode_bytes(self.raw(n))
        if t in B.FIXED:
            return "raw:" + self.raw(B.FIXED[t]).hex()
        if t in (0x0C, 0x0F):
            return struct.unpack("<Q", self.raw(8))[0]
        if t == 0x03:
            return list(struct.unpack("<3f", self.raw(12)))
        if t in (0x11, 0x13):
            end = self.u32()
            n = self.u32()
            if n > 1 << 22:
                raise Fail(f"count {n} @{self.p - 4:#x}")
            out = [self.value(ctx) for _ in range(n)]
            if end != self.p:
                self.warn.append(f"{ctx} list end {end:#x} != {self.p:#x}")
            return out
        if t in (0x12, 0x14):
            flag = self.u8()
            if flag != 0:
                return None
            end = self.u32()
            cls = self.name()
            n = self.u16()
            obj = {"_class": cls}
            for _ in range(n):
                k = self.name()
                obj[k] = self.value(f"{cls}.{k}")
            if end != self.p:
                self.warn.append(f"{cls} end {end:#x} != {self.p:#x}")
            return obj
        self.unknown.setdefault(t, ctx)
        raise Fail(f"unknown type {t:#x} ({ctx}) @{self.p - 1:#x}")

    def run(self):
        if self.u32() != MAGIC:
            raise Fail("magic")
        ver = self.u32()
        if ver != 0:
            raise Fail(f"not variant B (ver={ver})")
        self.u32()
        return self.value("root")


def main():
    b = open(sys.argv[1], "rb").read()
    d = B(b)
    root = d.run()
    print(f"parsed {d.p}/{len(b)} warnings={len(d.warn)}")
    for w in d.warn[:5]:
        print("  warn:", w)
    if "-q" not in sys.argv:
        print(json.dumps(root, indent=1)[:5000])


if __name__ == "__main__":
    main()
