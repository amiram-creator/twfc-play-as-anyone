"""Decode / encode TWFC Coalesced.ini (XOR-obfuscated UE3 coalesced ini).

usage:
  python coalesced.py decode <Coalesced.ini> <out_dir>
  python coalesced.py encode <in_dir> <Coalesced.ini>

Format (little-endian): int32 string_count (= 2 x files), then per file two strings
(path, contents). Each string is int32 length (incl. terminator), the XORed
chars, then a raw 0 terminator. The XOR key position runs on across the whole
file and the terminator does not advance it.
"""
import os, struct, sys

# UTF-16 string at TWFC.exe file offset 0x1fff9b0 (exe MD5 69acde90af160650ee5f60fff2f57d50)
KEY = b"as;dwepo2345098]qw]{}p2039458pseasdfzcvvp;aseiurwefsdcfszdcvn"


class Cipher:
    def __init__(self):
        self.pos = 0

    def __call__(self, b):
        out = bytes(c ^ KEY[(self.pos + i) % len(KEY)] for i, c in enumerate(b))
        self.pos += len(b)
        return out


def local_path(root, name):
    return os.path.join(root, name.replace("..\\", "").replace("\\", os.sep))


def read_str(d, o, xor):
    n = struct.unpack_from("<i", d, o)[0]; o += 4
    assert d[o + n - 1] == 0, f"missing terminator at {o + n - 1:#x}"
    return xor(d[o:o + n - 1]).decode("latin-1"), o + n


def write_str(s, xor):
    b = s.encode("latin-1")
    return struct.pack("<i", len(b) + 1) + xor(b) + b"\0"


def decode(src, out):
    d = open(src, "rb").read()
    count, o = struct.unpack_from("<i", d, 0)[0], 4
    xor, names = Cipher(), []
    for _ in range(count // 2):
        name, o = read_str(d, o, xor)
        body, o = read_str(d, o, xor)
        names.append(name)
        p = local_path(out, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "w", encoding="latin-1", newline="").write(body)
    assert o == len(d), f"trailing bytes: {len(d) - o}"
    open(os.path.join(out, "_order.txt"), "w").write("\n".join(names))
    print(f"decoded {count // 2} files -> {out}")


def encode(src, dst):
    names = open(os.path.join(src, "_order.txt")).read().split("\n")
    xor, out = Cipher(), [struct.pack("<i", 2 * len(names))]
    for name in names:
        body = open(local_path(src, name), encoding="latin-1", newline="").read()
        out += [write_str(name, xor), write_str(body, xor)]
    open(dst, "wb").write(b"".join(out))
    print(f"encoded {len(names)} files -> {dst}")


if __name__ == "__main__":
    {"decode": decode, "encode": encode}[sys.argv[1]](sys.argv[2], sys.argv[3])
