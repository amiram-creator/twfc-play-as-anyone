"""Decompress a TWFC cooked package (.xxx = UE3 package, LZO1X chunks).

usage: python upk_decompress.py <in.xxx> <out.upk>

Output = the uncompressed header bytes followed by every decompressed chunk,
i.e. the package as the engine sees it in memory. Header compression fields
are left as-is, so this is for reading/searching, not for loading in-game.
"""
import struct, sys

TAG = 0x9E2A83C1


def lzo1x_decompress(src, out_len):
    out = bytearray()
    ip = 0

    def literals(n):
        nonlocal ip
        out.extend(src[ip:ip + n]); ip += n

    def match(dist, n):
        start = len(out) - dist
        if dist >= n:
            out.extend(out[start:start + n])
        else:
            for i in range(n):
                out.append(out[start + i])

    def run_len(t, base):
        nonlocal ip
        while src[ip] == 0:
            t += 255; ip += 1
        t += base + src[ip]; ip += 1
        return t

    state = 0
    if src[0] > 17:
        t = src[0] - 17; ip = 1
        literals(t)
        state = t if t < 4 else 4
    while True:
        t = src[ip]; ip += 1
        if t < 16:
            if state == 0:
                if t == 0:
                    t = run_len(0, 15)
                literals(t + 3)
                state = 4
                continue
            if state != 4:
                dist = 1 + (t >> 2) + (src[ip] << 2); ip += 1
                match(dist, 2)
            else:
                dist = 1 + 0x800 + (t >> 2) + (src[ip] << 2); ip += 1
                match(dist, 3)
            nxt = t & 3
        elif t >= 64:
            dist = 1 + ((t >> 2) & 7) + (src[ip] << 3); ip += 1
            match(dist, (t >> 5) + 1)
            nxt = t & 3
        elif t >= 32:
            n = (t & 31) + 2
            if n == 2:
                n = run_len(2, 31)
            v = src[ip] | (src[ip + 1] << 8); ip += 2
            match(1 + (v >> 2), n)
            nxt = v & 3
        else:
            hi = (t & 8) << 11
            n = (t & 7) + 2
            if n == 2:
                n = run_len(2, 7)
            v = src[ip] | (src[ip + 1] << 8); ip += 2
            dist = hi + (v >> 2)
            if dist == 0:
                break  # end-of-stream marker
            match(dist + 0x4000, n)
            nxt = v & 3
        literals(nxt)
        state = nxt
    assert len(out) == out_len, f"lzo size {len(out)} != {out_len}"
    return bytes(out)


def decompress_chunk(d, o):
    tag, block, csize, usize = struct.unpack_from("<IIII", d, o)
    assert tag == TAG, f"bad chunk tag at {o:#x}"
    sizes, p, total = [], o + 16, 0
    while total < usize:
        c, u = struct.unpack_from("<II", d, p); p += 8
        sizes.append((c, u)); total += u
    out = []
    for c, u in sizes:
        out.append(lzo1x_decompress(d[p:p + c], u)); p += c
    return b"".join(out), p


def decompress_package(d):
    first = d.find(struct.pack("<I", TAG), 4)
    if first < 0:
        return d  # package is stored uncompressed
    parts, o = [d[:first]], first
    try:
        while o < len(d):
            data, o = decompress_chunk(d, o)
            parts.append(data)
    except AssertionError:
        # Not a whole-package compressed file: the tag belonged to one object's
        # compressed bulk data. Name/export tables are stored plain in the file.
        return d
    return b"".join(parts)


if __name__ == "__main__":
    data = decompress_package(open(sys.argv[1], "rb").read())
    open(sys.argv[2], "wb").write(data)
    print(f"{sys.argv[1]} -> {sys.argv[2]} ({len(data)} bytes)")
