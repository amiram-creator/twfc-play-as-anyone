"""List a script class's UFunctions and the native code pointers stored in them, from a full minidump.

usage: uv run --with minidump python tools\\class_funcs.py <full.dmp> <ClassName> [ClassName ...]

TWFC compiles script to native code, so each UFunction holds a pointer into .text. Prints every dword in the
UFunction object (first 0x80 bytes) that points into TWFC.exe's code range, as an RVA (ImageBase 0x10900000).
"""
import struct, sys
from minidump.minidumpfile import MinidumpFile

BASE, TEXT_END = 0x10900000, 0x12500000
md = MinidumpFile.parse(sys.argv[1]); rd = md.get_reader()
wanted = set(sys.argv[2:])
def u32(a): return struct.unpack("<I", rd.read(a, 4))[0]
def ws(a): return rd.read(a, 200).decode("utf-16le", "replace").split("\0")[0]
gn, go, gc = u32(0x12B2F0F4), u32(0x12B2F0A4), u32(0x12B2F0A8)
cache = {}
def name(o):
    fn = u32(o + 4); i, n = fn >> 14, fn & 0x3FFF
    if i not in cache: cache[i] = ws(u32(gn + i * 4) + 8)
    return cache[i] + (f"_{n-1}" if n else "")
def outer(o):
    oi = (u32(o + 0x24) >> 4) - 1
    return None if oi < 0 else u32(go + oi * 4)

objs = rd.read(go, gc * 4)
for i in range(gc):
    o = struct.unpack_from("<I", objs, i * 4)[0]
    if not o: continue
    try:
        p = outer(o)
        if not p or name(p) not in wanted or name(outer(p) or 0) not in ("TransGame", "HM_Engine", "Engine"):
            continue
        n = name(o)
        raw = rd.read(o, 0x80)
        ptrs = [(k, struct.unpack_from("<I", raw, k)[0]) for k in range(0, 0x80, 4)]
        code = [f"+{k:X}:{v-BASE:X}" for k, v in ptrs if BASE + 0x1000 <= v < TEXT_END and k != 0]
        print(f"{name(p)}.{n} @ {o:08X} code: {' '.join(code)}")
    except Exception:
        pass
