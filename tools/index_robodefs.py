"""List which packages DEFINE (export) each *_ROBODEF character definition.

usage: python index_robodefs.py <CookedPC dir> <out.tsv>
Writes: robodef object path <TAB> class <TAB> package file. Imports are ignored.
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from upk_tables import Package


def scan(path):
    try:
        p = Package(path)
    except Exception as e:
        return [f"ERROR {e}\t\t{os.path.basename(path)}"]
    out = []
    for i, e in enumerate(p.exports):
        if e["name"].lower().endswith("_robodef"):
            out.append(f"{p.path(i + 1)}\t{p.ref_name(e['cls'])}\t{os.path.basename(path)}")
    return out


if __name__ == "__main__":
    from multiprocessing import Pool
    src, out = sys.argv[1], sys.argv[2]
    paths = sorted((os.path.join(src, f) for f in os.listdir(src) if f.endswith(".xxx")), key=os.path.getsize, reverse=True)
    with Pool() as pool:
        rows = [r for rs in pool.imap_unordered(scan, paths, chunksize=4) for r in rs]
    open(out, "w").write("\n".join(sorted(rows)) + "\n")
    print(len(rows), "rows")
