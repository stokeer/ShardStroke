"""Hex dump with UTF-16BE string hints. usage: hexdump.py FILE [offset=0] [length=512]"""
import sys


def main():
    p = sys.argv[1]
    off = int(sys.argv[2], 0) if len(sys.argv) > 2 else 0
    ln = int(sys.argv[3], 0) if len(sys.argv) > 3 else 512
    with open(p, "rb") as f:
        f.seek(off)
        d = f.read(ln)
    for i in range(0, len(d), 16):
        c = d[i:i + 16]
        h = " ".join(f"{x:02X}" for x in c).ljust(47)
        a = "".join(chr(x) if 32 <= x < 127 else "." for x in c)
        # UTF-16BE view: take low byte where high byte is 0
        print(f"{off + i:06X}  {h}  |{a}|")


if __name__ == "__main__":
    main()
