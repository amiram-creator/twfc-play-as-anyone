"""Dump the tagged properties of exports in a TWFC package (UE3 v511, licensee 145).

usage: python upk_props.py <pkg.xxx> <export_path_regex> [max_exports]
Object references print as their full path; arrays of object references are expanded;
structs and other arrays print their size (and raw hex when short).
"""
import re, struct, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from upk_tables import Package


def fname(p, d, o):
    i, n = struct.unpack_from("<ii", d, o)
    return p.name(i, n) if 0 <= i < len(p.names) else None


def props(p, e):
    d = p.d
    start = e["off"] + p.shift
    # cooked objects begin with a NetIndex; find the first offset that parses as a tag
    for skip in (4, 0, 8, 12):
        o = start + skip
        if fname(p, d, o) and fname(p, d, o + 8) and fname(p, d, o + 8).endswith("Property") or fname(p, d, o) == "None":
            break
    out = []
    end = start + e["size"]
    while o < end:
        name = fname(p, d, o)
        if name is None or name == "None":
            break
        typ = fname(p, d, o + 8); size, idx = struct.unpack_from("<ii", d, o + 16); o += 24
        extra = ""
        if typ == "StructProperty":
            extra = fname(p, d, o); o += 8
        if typ == "BoolProperty":
            val = str(bool(d[o])); o += 4              # UBOOL (4 bytes) before v673
        else:
            raw = d[o:o + size]; o += size
            if typ in ("ObjectProperty", "ClassProperty", "ComponentProperty", "InterfaceProperty") and size == 4:
                val = p.path(struct.unpack("<i", raw)[0])
            elif typ == "IntProperty":
                val = str(struct.unpack("<i", raw)[0])
            elif typ == "FloatProperty":
                val = f"{struct.unpack('<f', raw)[0]:g}"
            elif typ in ("NameProperty",) or (typ == "ByteProperty" and size == 8):
                val = fname(p, raw, 0) if size == 8 else raw.hex()
            elif typ == "ByteProperty":
                val = str(raw[0])
            elif typ == "ArrayProperty" and size >= 4:
                n, = struct.unpack_from("<i", raw, 0)
                if size == 4 + 4 * n and n:
                    refs = [struct.unpack_from("<i", raw, 4 + 4 * k)[0] for k in range(n)]
                    val = f"[{n}] " + ", ".join(p.path(r) if -len(p.imports) <= r <= len(p.exports) else str(r) for r in refs)
                else:
                    val = f"[{n}] ({size} bytes) {raw[4:40].hex()}"
            elif typ == "StrProperty":
                val = raw[4:].decode("latin1", "replace").rstrip("\0")
            else:
                val = f"({size} bytes) {raw[:32].hex()}"
        out.append(f"  {name}{f'[{idx}]' if idx else ''} : {typ.replace('Property','')}{' '+extra if extra else ''} = {val}")
    return out


if __name__ == "__main__":
    p = Package(sys.argv[1])
    rx = re.compile(sys.argv[2], re.I)
    limit = int(sys.argv[3]) if len(sys.argv) > 3 else 20
    shown = 0
    for i, e in enumerate(p.exports):
        full = p.path(i + 1)
        if rx.search(full):
            print(f"== {p.ref_name(e['cls'])} {full} (size {e['size']})")
            print("\n".join(props(p, e)))
            shown += 1
            if shown >= limit:
                break
