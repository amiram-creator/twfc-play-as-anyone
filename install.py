"""Install / uninstall the TWFC Play-as-Anyone mod (Omega Supreme).

usage:
  python install.py "<game folder>"              install
  python install.py "<game folder>" --uninstall  restore everything

<game folder> is the folder that contains Binaries\\TWFC.exe, e.g.
  "C:\\Program Files (x86)\\Steam\\steamapps\\common\\Transformers War for Cybertron"

What install does (every changed game file is backed up first, as <file>.bak-playasanyone):
  1. Coalesced.ini: adds the key bindings from mod\\bindings.ini (rebuilt from the backup each time, so installs never stack).
  2. Copies mod\\DLC\\DLCMaps to <game>\\DLC\\DLCMaps (makes Iacon load Omega Supreme's packages).
  3. Copies dinput8.dll (the mod) next to TWFC.exe.
  4. Sets the Large Address Aware flag on TWFC.exe (lets the 32-bit game use 4 GB; the extra boss packages need it).
"""
import hashlib, os, shutil, struct, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "tools"))
import coalesced  # noqa: E402

SUFFIX = ".bak-playasanyone"
# TWFC.exe the mod was made for (the DLL also checks the patched bytes and refuses to hook another build)
KNOWN_EXE_MD5 = {"69acde90af160650ee5f60fff2f57d50"}
INI_DIR = os.path.join("TransGame", "Config", "PC", "Cooked")
SECTION = "[Engine.PlayerInput]"
DLL_CANDIDATES = [os.path.join(HERE, "dinput8.dll"),
                  os.path.join(HERE, "omega_mod", "target", "i686-pc-windows-msvc", "release", "dinput8.dll")]


def paths(game):
    return {
        "exe": os.path.join(game, "Binaries", "TWFC.exe"),
        "ini": os.path.join(game, INI_DIR, "Coalesced.ini"),
        "dll": os.path.join(game, "Binaries", "dinput8.dll"),
        "log": os.path.join(game, "Binaries", "omega_mod.log"),
        "dlc": os.path.join(game, "DLC", "DLCMaps"),
    }


def md5(path):
    return hashlib.md5(open(path, "rb").read()).hexdigest()


def backup(path):
    if not os.path.exists(path + SUFFIX):
        shutil.copyfile(path, path + SUFFIX)
        print(f"  backup: {path + SUFFIX}")


def restore(path):
    if os.path.exists(path + SUFFIX):
        shutil.copyfile(path + SUFFIX, path)
        os.remove(path + SUFFIX)
        print(f"  restored {path}")


def set_laa(exe, on):
    d = bytearray(open(exe, "rb").read())
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    ch = struct.unpack_from("<H", d, pe + 0x16)[0]
    new = ch | 0x20 if on else ch & ~0x20
    if new != ch:
        struct.pack_into("<H", d, pe + 0x16, new)
        open(exe, "wb").write(d)


def bindings():
    f = os.path.join(HERE, "mod", "bindings.ini")
    return [l for l in open(f).read().splitlines() if l.strip() and not l.lstrip().startswith(";")]


def install(game):
    p = paths(game)
    if not os.path.exists(p["exe"]):
        sys.exit(f"TWFC.exe not found at {p['exe']}. Pass the game folder (the one that contains Binaries).")
    dll = next((c for c in DLL_CANDIDATES if os.path.exists(c)), None)
    if not dll:
        sys.exit("dinput8.dll not found. Use the release zip, or build it first (see README, 'Build from source').")
    exe_md5 = md5(p["exe"] + SUFFIX if os.path.exists(p["exe"] + SUFFIX) else p["exe"])
    if exe_md5 not in KNOWN_EXE_MD5:
        print(f"WARNING: unknown TWFC.exe (MD5 {exe_md5}). The mod was made for MD5 {next(iter(KNOWN_EXE_MD5))};")
        print("         its hooks check the code first and stay off on other builds.")
    if os.path.exists(p["dll"]) and b"omega_mod" not in open(p["dll"], "rb").read():
        backup(p["dll"])  # someone else's dinput8.dll (another mod): keep it

    print("1/4 key bindings -> Coalesced.ini")
    backup(p["ini"])
    with tempfile.TemporaryDirectory() as tmp:
        coalesced.decode(p["ini"] + SUFFIX, tmp)
        f = os.path.join(tmp, INI_DIR, "PC-TransInput.ini")
        text = open(f, encoding="latin-1", newline="").read()
        assert text.count(SECTION + "\r\n") == 1, "unexpected PC-TransInput.ini layout"
        text = text.replace(SECTION + "\r\n", SECTION + "\r\n" + "".join(l + "\r\n" for l in bindings()))
        open(f, "w", encoding="latin-1", newline="").write(text)
        coalesced.encode(tmp, p["ini"])

    print("2/4 DLC pack -> " + p["dlc"])
    shutil.rmtree(p["dlc"], ignore_errors=True)
    shutil.copytree(os.path.join(HERE, "mod", "DLC", "DLCMaps"), p["dlc"])

    print("3/4 dinput8.dll -> Binaries")
    try:
        shutil.copyfile(dll, p["dll"])
    except PermissionError:
        sys.exit("Binaries\\dinput8.dll is in use: close the game and run the installer again.")

    print("4/4 Large Address Aware flag on TWFC.exe")
    backup(p["exe"])
    set_laa(p["exe"], True)
    print("Done. Start the game normally (see README, 'How to play').")


def uninstall(game):
    p = paths(game)
    restore(p["ini"])
    restore(p["exe"])
    if os.path.exists(p["dll"] + SUFFIX):
        restore(p["dll"])
    elif os.path.exists(p["dll"]):
        os.remove(p["dll"])
        print(f"  removed {p['dll']}")
    if os.path.exists(p["log"]):
        os.remove(p["log"])
    if os.path.isdir(p["dlc"]):
        shutil.rmtree(p["dlc"])
        print(f"  removed {p['dlc']}")
        dlc_root = os.path.dirname(p["dlc"])
        if not os.listdir(dlc_root):
            os.rmdir(dlc_root)
    print("Uninstalled; the game is back to its original files.")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    (uninstall if "--uninstall" in sys.argv[2:] else install)(sys.argv[1])
