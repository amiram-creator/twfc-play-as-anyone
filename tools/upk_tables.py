"""Read the name / import / export tables of a TWFC package (UE3 v511, licensee 145).

usage: python upk_tables.py <pkg.xxx> [filter_regex]
Prints exports (objects defined in this package) and imports (objects it references
from other packages). Used to tell where a ROBODEF really lives.
"""
import re, struct, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from upk_decompress import decompress_package


class Package:
    def __init__(self, path):
        raw = open(path, "rb").read()
        self.d = d = decompress_package(raw)
        o = 0x10
        n, = struct.unpack_from("<i", d, o)
        o += 4 + (n if n > 0 else -2 * n)          # folder name FString
        (self.flags, nc, no, ec, eo, ic, io) = struct.unpack_from("<7I", d, o)
        # Offsets in the summary are logical (uncompressed package). When the file is
        # chunk-compressed, decompress_package keeps the raw header (which also holds
        # the chunk table), so shift by: raw header length - first chunk's logical offset.
        o += 28 + 4 + 16                             # 7 fields above, depends offset, guid
        gc, = struct.unpack_from("<i", d, o); o += 4 + 12 * gc   # generations
        o += 8                                       # engine, cooker version
        comp, chunks = struct.unpack_from("<II", d, o); o += 8
        self.shift = 0
        if comp and chunks and len(d) != len(raw):
            first_logical, = struct.unpack_from("<I", d, o)
            # raw header (+ chunk table + 4 licensee bytes) runs up to the first chunk tag
            self.shift = raw.find(struct.pack("<I", 0x9E2A83C1), 4) - first_logical
        self.names = self._names(no + self.shift, nc)
        self.imports = self._imports(io + self.shift, ic)
        self.exports = self._exports(eo + self.shift, ec)
        self.depends_ok = self._end == struct.unpack_from("<I", d, 0x10 + 4 + (n if n > 0 else -2 * n) + 28)[0] + self.shift

    def name(self, idx, num=0):
        s = self.names[idx] if 0 <= idx < len(self.names) else f"?{idx}"
        return f"{s}_{num - 1}" if num else s

    def _names(self, o, n):
        out = []
        for _ in range(n):
            ln, = struct.unpack_from("<i", self.d, o); o += 4
            if ln >= 0:
                out.append(self.d[o:o + ln - 1].decode("latin1")); o += ln
            else:
                out.append(self.d[o:o - 2 * ln - 2].decode("utf-16le")); o += -2 * ln
            o += 8                                   # object flags
        return out

    def _imports(self, o, n):
        out = []
        for _ in range(n):
            cp, cpn, cn, cnn, outer, on, onn = struct.unpack_from("<7i", self.d, o); o += 28
            out.append(dict(cls_pkg=self.name(cp, cpn), cls=self.name(cn, cnn), outer=outer, name=self.name(on, onn)))
        return out

    def _exports(self, o, n):
        out = []
        for _ in range(n):
            cls, sup, outer, on, onn, arch = struct.unpack_from("<6i", self.d, o); o += 24
            oflags, size, off = struct.unpack_from("<QiI", self.d, o); o += 16
            # licensee 145 layout: export flags, then a bool; forced-export (package) entries
            # carry generation net counts + guid + package flags after it
            o += 4
            has_pkg = self.d[o]; o += 1
            if has_pkg:
                gn, = struct.unpack_from("<i", self.d, o); o += 4 + 4 * gn + 16 + 4
            out.append(dict(cls=cls, outer=outer, name=self.name(on, onn), size=size, off=off))
        self._end = o
        return out

    def ref_name(self, ref):
        if ref > 0: return self.exports[ref - 1]["name"]
        if ref < 0: return self.imports[-ref - 1]["name"]
        return "None"

    def path(self, ref):
        parts = []
        while ref:
            if ref > 0:
                e = self.exports[ref - 1]; parts.append(e["name"]); ref = e["outer"]
            else:
                i = self.imports[-ref - 1]; parts.append(i["name"]); ref = i["outer"]
        return ".".join(reversed(parts))


if __name__ == "__main__":
    p = Package(sys.argv[1])
    flt = re.compile(sys.argv[2], re.I) if len(sys.argv) > 2 else None
    print(f"names {len(p.names)} imports {len(p.imports)} exports {len(p.exports)} shift {p.shift:#x} tables_ok {p.depends_ok}")
    for i, e in enumerate(p.exports):
        full = p.path(i + 1)
        if not flt or flt.search(full):
            print(f"EXPORT {p.ref_name(e['cls']):28} {full}  size={e['size']}")
    for i, im in enumerate(p.imports):
        full = p.path(-(i + 1))
        if not flt or flt.search(full):
            print(f"IMPORT {im['cls']:28} {full}")
