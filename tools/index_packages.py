"""Index every cooked package's name table for character-related names.

usage: python index_packages.py <CookedPC dir> <out.tsv>
Writes: package <TAB> matching names (comma separated). Uses upk_decompress for LZO packages.
Runs one worker per CPU core.
"""
import os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from upk_decompress import decompress_package

PAT = re.compile(r"(?i)robodef|_skel|skelmesh|_pawn$|archetype|_pcd_|zeta|trypticon|omega|starscreamboss|shockwave|blueprint")
NAME = re.compile(rb"[\x02-\x50]\x00\x00\x00([A-Za-z_][A-Za-z0-9_]{2,70})\x00")


def scan(path):
    f = os.path.basename(path)
    try:
        d = decompress_package(open(path, "rb").read())
    except Exception as e:
        return f"{f}\tERROR {e}\n"
    hits = sorted({m.group(1).decode() for m in NAME.finditer(d) if PAT.search(m.group(1).decode())})
    return f"{f}\t{','.join(hits)}\n" if hits else ""


if __name__ == "__main__":
    from multiprocessing import Pool
    src, out = sys.argv[1], sys.argv[2]
    paths = [os.path.join(src, f) for f in os.listdir(src) if f.endswith(".xxx")]
    paths.sort(key=os.path.getsize, reverse=True)  # big ones first so they don't finish last
    with Pool() as pool, open(out, "w") as o:
        for i, line in enumerate(pool.imap_unordered(scan, paths, chunksize=4)):
            o.write(line)
            if i % 200 == 0:
                print(f"{i}/{len(paths)}", flush=True)
    print("done")
