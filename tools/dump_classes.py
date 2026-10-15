"""Rebuild TWFC's native class hierarchy from a full minidump.

usage: set TWFC_EXE=<game>\Binaries\TWFC.exe, then python dump_classes.py <full.dmp> [ClassName ...]

How it works (exe MD5 69acde90af160650ee5f60fff2f57d50, ImageBase 0x10900000, RVAs below):
- Native code caches class pointers via FindClass(&cache, L"Name") at RVA 0x7031A0;
  we scan .text for `push L"Name"; push &cache; call 0x7031A0` to get name->cache pairs.
- cache holds UClass+0x10; the class id is at UClass+0x84.
- An object's class id is (obj+0x14 & 0xFFF) - 1.
- IsA table at RVA 0x212E3E8: 8 bytes per class id, five packed 12-bit ancestor ids,
  bits 16..27 of the second dword chain to a continuation entry.
"""
import struct, sys
import os
import pefile
from minidump.minidumpfile import MinidumpFile

EXE = os.environ.get("TWFC_EXE", "TWFC.exe")  # path to the game exe (set TWFC_EXE)
BASE = 0x10900000
FINDCLASS_RVA = 0x7031A0
ISA_TABLE = BASE + 0x212E3E8


class Dump:
    def __init__(self, path):
        self.md = MinidumpFile.parse(path)
        self.rd = self.md.get_reader()

    def u32(self, a):
        return struct.unpack("<I", self.rd.read(a, 4))[0]


def cached_class_names(pe):
    """name -> cache VA, from push imm32 / push imm32 / call FindClass."""
    text = pe.sections[0]
    data, tva = text.get_data(), text.VirtualAddress
    out = {}
    for i in range(10, len(data) - 5):
        if data[i] != 0xE8 or tva + i + 5 + struct.unpack_from("<i", data, i + 1)[0] != FINDCLASS_RVA:
            continue
        # expect: 68 <wstr>  68 <cache>  e8 <FindClass>
        if data[i - 10] == 0x68 and data[i - 5] == 0x68:
            s_va, cache = struct.unpack_from("<I", data, i - 9)[0], struct.unpack_from("<I", data, i - 4)[0]
            try:
                raw = pe.get_data(s_va - BASE, 128)
                name = raw.decode("utf-16le").split("\0")[0]
            except Exception:
                continue
            if name.isidentifier():
                out[name] = cache
    return out


def ancestors(d, cid):
    """All ancestor ids listed for class id `cid` (includes itself)."""
    ids, e = [], cid
    while True:
        a, b = d.u32(ISA_TABLE + e * 8), d.u32(ISA_TABLE + e * 8 + 4)
        ids += [a & 0xFFF, (a >> 12) & 0xFFF, (a >> 24) | ((b & 0xF) << 8), (b >> 4) & 0xFFF]
        nxt = (b >> 16) & 0xFFF
        if nxt == 0:
            return [x for x in ids if x]
        e = nxt


def main():
    d = Dump(sys.argv[1])
    pe = pefile.PE(EXE, fast_load=True)
    names = cached_class_names(pe)
    by_id = {}
    for n, cache in names.items():
        p = d.u32(cache)
        if p:
            by_id[d.u32(p - 0x10 + 0x84)] = n
    print(f"{len(names)} cached class names, {len(by_id)} resolved in this dump")
    for n in sys.argv[2:] or ["TnPlayerPawn"]:
        cid = next((i for i, x in by_id.items() if x == n), None)
        if cid is None:
            print(n, ": not cached/loaded"); continue
        print(f"{n} (id {cid}): " + " <- ".join(by_id.get(a, f"#{a}") for a in ancestors(d, cid)))
    return d, by_id


if __name__ == "__main__":
    main()
