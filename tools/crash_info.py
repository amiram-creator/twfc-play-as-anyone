"""Summarise a TWFC.exe crash dump: exception, registers, engine objects in registers, return addresses.

usage: python crash_info.py <TWFC.exe.NNNN.dmp>   (needs: pip install minidump)
"""
import struct, sys
from minidump.minidumpfile import MinidumpFile

md = MinidumpFile.parse(sys.argv[1])
rd = md.get_reader()
exc = md.exception.exception_records[0]
er = exc.ExceptionRecord
print("thread", exc.ThreadId, "code", hex(er.ExceptionCode_raw), "addr", hex(er.ExceptionAddress),
      "info", [hex(x) for x in er.ExceptionInformation[:er.NumberParameters]])
ctx = None
for t in md.threads.threads:
    if t.ThreadId == exc.ThreadId:
        ctx = t.ContextObject
        break
regs = {k: getattr(ctx, k) for k in ("Eax", "Ebx", "Ecx", "Edx", "Esi", "Edi", "Ebp", "Esp", "Eip")}
print({k: hex(v) for k, v in regs.items()})

def u32(a):
    return struct.unpack("<I", rd.read(a, 4))[0]

def ws(a, n=80):
    try:
        b = rd.read(a, n * 2)
        return b.decode("utf-16le", "replace").split("\0")[0]
    except Exception:
        return None

# object helpers (TWFC object system)
gn, go = u32(0x12B2F0F4), u32(0x12B2F0A4)
def name(o):
    fn = u32(o + 4); i, n = fn >> 14, fn & 0x3FFF
    s = ws(u32(gn + i * 4) + 8) or "?"
    return s + (f"_{n-1}" if n else "")
def path(o):
    s, k = name(o), 0
    oi = (u32(o + 0x24) >> 4) - 1
    while oi >= 0 and k < 8:
        p = u32(go + oi * 4); s = name(p) + "." + s; oi = (u32(p + 0x24) >> 4) - 1; k += 1
    return s
def desc(v):
    try:
        vt = u32(v)
        if 0x10900000 <= vt < 0x13000000:
            return f"obj {path(v)} cid={(u32(v+0x14)&0xFFF)-1}"
    except Exception:
        pass
    return ""

for k, v in regs.items():
    d = desc(v) if v > 0x10000 else ""
    if d:
        print(" ", k, hex(v), d)

print("code around eip:")
print(rd.read(regs["Eip"] - 16, 32).hex())
print("stack return-address candidates:")
esp = regs["Esp"]
n = 0
for off in range(0, 0x3000, 4):
    try:
        v = u32(esp + off)
    except Exception:
        break
    if 0x10901000 <= v < 0x12000000:
        # check preceded by a call
        try:
            b = rd.read(v - 6, 6)
        except Exception:
            continue
        if b[1] == 0xE8 or b[0] == 0xFF or b[4] == 0xFF:
            print(f"  esp+{off:04X} {v:08X} rva {v-0x10900000:06X}")
            n += 1
            if n > 40:
                break
    else:
        d = desc(v) if v > 0x10000 else ""
        if d and off < 0x400:
            print(f"  esp+{off:04X} {v:08X} {d}")
