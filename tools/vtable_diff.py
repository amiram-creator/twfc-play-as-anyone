"""List the virtual functions a class overrides, from a full minidump.

usage: uv run --with minidump python tools\\vtable_diff.py <full.dmp> <ObjectNamePrefix> <BaseObjectName> [count]

Finds the first live object whose name starts with ObjectNamePrefix (e.g. TnThirdPersoncollisionCameraBehavior_)
and the object named BaseObjectName (e.g. Default__HmCameraBehavior), reads both vtables (object +0) and prints
the slots whose function differs, as RVAs (ImageBase 0x10900000), ready for Ghidra's Decomp.java.
"""
import struct, sys
from minidump.minidumpfile import MinidumpFile

BASE = 0x10900000
md = MinidumpFile.parse(sys.argv[1]); rd = md.get_reader()
prefix, base_name = sys.argv[2], sys.argv[3]
count = int(sys.argv[4]) if len(sys.argv) > 4 else 160

def u32(a): return struct.unpack("<I", rd.read(a, 4))[0]
def ws(a): return rd.read(a, 200).decode("utf-16le", "replace").split("\0")[0]
gn, go, gc = u32(0x12B2F0F4), u32(0x12B2F0A4), u32(0x12B2F0A8)
cache = {}
def name(o):
    fn = u32(o + 4); i, n = fn >> 14, fn & 0x3FFF
    if i not in cache: cache[i] = ws(u32(gn + i * 4) + 8)
    return cache[i] + (f"_{n-1}" if n else "")

objs = rd.read(go, gc * 4)
obj = base = None
for i in range(gc):
    o = struct.unpack_from("<I", objs, i * 4)[0]
    if not o: continue
    try: n = name(o)
    except Exception: continue
    if obj is None and n.startswith(prefix): obj = (o, n)
    if n == base_name: base = (o, n)
    if obj and base: break
if not (obj and base):
    sys.exit(f"not found: {prefix if not obj else ''} {base_name if not base else ''}")
vt, bvt = u32(obj[0]), u32(base[0])
print(f"{obj[1]} @ {obj[0]:08X} vtable {vt:08X} (RVA {vt-BASE:X}); {base[1]} vtable {bvt:08X}")
for k in range(count):
    f, b = u32(vt + k * 4), u32(bvt + k * 4)
    if not (BASE <= f < BASE + 0x2500000): break
    if f != b:
        print(f"slot {k:3d} (+0x{k*4:03X}): {f:08X} RVA 0x{f-BASE:X}   base {b:08X}")
