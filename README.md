**Childhood dream came true :)**

# Transformers: War for Cybertron: Play as Anyone

> 🛠️ **Work in progress:** we are building big single-player and multiplayer maps, so giant robots like Omega Supreme have room to fight.

An open-source mod for the PC version of *Transformers: War for Cybertron* (2010). The goal is to make **every character in the game
playable**. The first release is **Omega Supreme**: play the giant Autobot boss in the campaign, with his real boss-fight attacks.

## How to play

> First time? [Install](#install) the mod, then:

1. Start the game normally and load any **Decepticon** campaign chapter.
2. When you are in control, press **F4**. This loads the **Iacon** level with Omega Supreme's data.
3. You spawn as **Omega Supreme**. The guide appears on the right, and god mode switches on about 2 seconds after you get control (keep the game window in front).
4. Press **T** to pick an attack and **Q** to use it. Press **F11** while aiming at an enemy to grab it. All keys: [Controls](#controls).

### Video guides

<!-- VIDEO LINKS: replace this comment with the guide videos -->
*Coming soon.*

## Features

- **Play as Omega Supreme** in the Iacon campaign level, with his own model, animations, walk and reload, and a giant third-person camera.
- **His real boss attacks**, taken from his boss fight and the Iacon set pieces:
  - **Missile Barrage**: a volley of homing rockets launched from his back.
  - **Missile Gun (long range)**: his arm launcher. A slower, long-range salvo.
  - **Missile Gun (short range)**: his arm launcher. A fast, many-missile salvo.
  - **Ground Pound (AOE)**: a shock wave around him.
  - **Tractor Grab**: the real boss grab. He pulls an enemy into his claw, crushes it and throws it away.
  - **Homing rockets** from his left arm on the fire button, plus his **Shooting Frenzy** arm.
- **Auto-targeting**: rockets go for the enemy you aim at, or spread over the nearest enemies in front of you.
- **On-screen guide** on the right of the screen. It shows the active attack mode live, the grab status, god mode and every key.
- **God mode turns on by itself** when you spawn as Omega.
- No Cheat Engine, no extra tools while playing: one DLL, loaded by the game itself.
- **Fully reversible**: the installer backs up every file it changes, and `--uninstall` restores them.

## Requirements

- *Transformers: War for Cybertron* for PC. The mod was made for `TWFC.exe` with MD5 `69acde90af160650ee5f60fff2f57d50`. On a
  different build the mod's hooks check the code first and stay off, so nothing breaks.
- Windows, and [Python 3](https://www.python.org/downloads/) to run the installer.
- **Single-player only.** Don't use it online.

## Install

1. Download the latest zip from [Releases](../../releases) and unzip it anywhere.
2. Close the game. Open a command prompt in the unzipped folder and run (use your own game path):
   ```
   python install.py "C:\Program Files (x86)\Steam\steamapps\common\Transformers War for Cybertron"
   ```
   The game folder is the one that contains `Binaries\TWFC.exe`.

The installer:
1. adds the mod's key bindings to `Coalesced.ini`;
2. copies a small DLC pack that loads Omega Supreme into the Iacon level;
3. copies `dinput8.dll` (the mod) next to `TWFC.exe`;
4. sets the Large Address Aware flag on `TWFC.exe`, so the 32-bit game can use 4 GB of memory for the extra boss data.

Every changed file is backed up first as `<file>.bak-playasanyone`.

## Controls

| Key | Action |
| --- | --- |
| **Q** | Melee: the selected boss attack |
| **T** | Switch attack: Missile Barrage → Missile Gun (long) → Missile Gun (short) → Ground Pound. One beep per position. |
| **F11** | Tractor grab on the enemy under your crosshair: pull, claw crush, throw |
| **Fire** | Homing rockets from the left arm |
| **F2** | Shooting Frenzy arm |
| **F8** | God mode on/off |
| **Home** | Hide / show the guide |
| **F6 / F7** | Slow motion on / off (for videos) |
| **F4** | Load the Iacon level as Omega Supreme |

## Uninstall

```
python install.py "<game folder>" --uninstall
```
This restores `Coalesced.ini` and `TWFC.exe` from their backups and removes the DLL and the DLC pack.

## Known limitations

- Omega only appears in the Iacon level (his data only exists in his own levels; the DLC pack loads it there).
- He is huge: narrow streets and doorways can block him. His collision is reduced so he fits through most of the level.
- Transforming gives Megatron's tank, and the HUD and a small gun model are Megatron's: Omega borrows Megatron's player setup.
- Auto-targeting picks Autobots, which are the enemies in the Decepticon campaign.

## Troubleshooting

- **Still Megatron?** Make sure you loaded Iacon with **F4** (or the Iacon chapter as Megatron).
- **Something went wrong?** The mod writes `Binaries\omega_mod.log`. Crash dumps are in `%LOCALAPPDATA%\CrashDumps`. Please attach both when you open an issue.

## Build from source

Install [Rust](https://rustup.rs/) with the 32-bit MSVC target (the game is 32-bit), then:
```
rustup target add i686-pc-windows-msvc
cd omega_mod
cargo build --release
```
The DLL is `omega_mod/target/i686-pc-windows-msvc/release/dinput8.dll`. `install.py` picks it up from there when no `dinput8.dll` is next to it.

## For modders: continue the work

This is an open project and anyone can pick it up. Everything we found is documented, and the tools we built are here.

- **[docs/TECHNICAL.md](docs/TECHNICAL.md)**: how the game works inside (config, packages, object system, characters, bosses), how the mod
  works, every address it uses, dead ends to avoid, and a step-by-step recipe for adding another character.
- **[tools/](tools/)**: Python tools for the config (`coalesced.py`), packages (decompress, tables, property dump, ROBODEF index), crash dumps and the exe.
- **[cheat-engine/boss_swap.lua](cheat-engine/boss_swap.lua)**: the live research prototype. Every feature is a Cheat Engine Lua function, for trying ideas before porting them.
- **[omega_mod/](omega_mod/)**: the mod itself (Rust): code caves, the spawn hook, attacks, grab and the Direct3D 9 guide panel.

Pull requests, issues and new characters are welcome.

## Repository layout

```
install.py              installer / uninstaller
mod/                    key bindings + DLC pack (Iacon loads Omega's packages)
omega_mod/              the mod (Rust, builds dinput8.dll)
tools/                  research and modding tools (Python)
cheat-engine/           Cheat Engine Lua prototype
docs/TECHNICAL.md       technical guide
```

## Credits

- Made by [@amiram-creator](https://github.com/amiram-creator).
- Community work that helped: Barricade24's War for Cybertron modding package (Coalesced format, DLC map packs, free camera),
  [TWFC-FPS-Unlocker](https://github.com/StyxSL/TWFC-FPS-Unlocker) (proved DLL loading with this exe) and
  [WFCDLC](https://github.com/DarkElementPL/WFCDLC) (DLC pack folders).

## Disclaimer

A fan-made, non-commercial mod. Not affiliated with or endorsed by Activision, High Moon Studios or Hasbro. Transformers is a trademark
of Hasbro. This repository contains no game files: you need your own copy of the game.

## License

[MIT](LICENSE)
