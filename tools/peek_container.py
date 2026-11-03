"""Print text members of a Painter HDF5 container and test resource-name hashing.

usage: peek_container.py FILE [member ...]
"""
import hashlib
import sys

import h5py

TEXT_EXT = (".ini", ".json", ".txt")


def main():
    path = sys.argv[1]
    want = sys.argv[2:]
    with h5py.File(path, "r") as f:
        names = []
        f.visititems(lambda n, o: names.append(n) if isinstance(o, h5py.Dataset) else None)
        print("datasets:", len(names))
        for n in names:
            if want and n not in want:
                continue
            if not want and not n.endswith(TEXT_EXT):
                continue
            b = f[n][()].tobytes()
            print(f"\n=== {n} ({len(b)} bytes)")
            print(b.decode("utf-8", "replace")[:1800])
        print("\n=== hash test (resources/<name> sha1(content) vs name)")
        for n in names:
            if n.startswith("resources/") and ".alg_meta" not in n and "." in n.split("/")[-1]:
                b = f[n][()].tobytes()
                stem = n.split("/")[-1].split(".")[0]
                s1 = hashlib.sha1(b).hexdigest()
                print(f"{stem[:12]}  sha1(content)={s1[:12]}  match={s1 == stem}")
                break


if __name__ == "__main__":
    main()
