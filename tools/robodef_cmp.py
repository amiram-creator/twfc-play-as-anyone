"""Compare ROBODEF objects field by field in a full minidump (object pointers / FStrings)."""
import struct, sys
from minidump.minidumpfile import MinidumpFile

md = MinidumpFile.parse(sys.argv[1]); rd = md.get_reader()
names = sys.argv[2:]
def u32(a): return struct.unpack("<I", rd.read(a, 4))[0]
def ws(a, n=80):
    try: return rd.read(a, n * 2).decode("utf-16le", "replace").split("\0")[0]
    except Exception: return None
gn, go, gc = u32(0x12B2F0F4), u32(0x12B2F0A4), u32(0x12B2F0A8)
nc = {}
def name(o):
    fn = u32(o + 4); i, n = fn >> 14, fn & 0x3FFF
    if i not in nc: nc[i] = ws(u32(gn + i * 4) + 8) or "?"
    return nc[i] + (f"_{n-1}" if n else "")
def path(o):
    s, k = name(o), 0; oi = (u32(o + 0x24) >> 4) - 1
    while oi >= 0 and k < 6:
        p = u32(go + oi * 4); s = name(p) + "." + s; oi = (u32(p + 0x24) >> 4) - 1; k += 1
    return s
objs = rd.read(go, gc * 4)
found = {}
for i in range(gc):
    o = struct.unpack_from("<I", objs, i * 4)[0]
    if not o: continue
    try:
        if (u32(o + 0x14) & 0xFFF) - 1 == 1879 and name(o) in names: found[name(o)] = o
    except Exception: pass
print({k: hex(v) for k, v in found.items()})
def desc(v):
    if v < 0x10000: return ""
    try:
        if 0x10900000 <= u32(v) < 0x13000000: return path(v)
    except Exception: return ""
    return ""
SIZE = 0x180
cols = [found[n] for n in names if n in found]
mem = [rd.read(o, SIZE) for o in cols]
print("off  " + "  ".join(n[:30].ljust(30) for n in names if n in found))
for off in range(0x40, SIZE, 4):
    vals = [struct.unpack_from("<I", m, off)[0] for m in mem]
    ds = [desc(v) for v in vals]
    if True:  # object in some, not in others
        print(f"{off:03X}  " + "  ".join((d or hex(v))[-60:].ljust(30) for v, d in zip(vals, ds)))
    elif any(ds) and len(set(d.split('.')[0] for d in ds)) > 0 and sys.argv[-1] == "all":
        print(f"{off:03X}  " + "  ".join(d[-60:].ljust(30) for d in ds))
