# Technical guide

Everything we learned about *Transformers: War for Cybertron* (PC) while making Omega Supreme playable. With this page and the
tools in this repo, you can continue the work or add another character. Every address and offset here was checked in the real
game; none are guessed. Addresses are for `TWFC.exe` with MD5 `69acde90af160650ee5f60fff2f57d50`, ImageBase `0x10900000`, no ASLR.
That makes them absolute and the same on every run.

- [1. The game files](#1-the-game-files)
- [2. Engine object system](#2-engine-object-system)
- [3. How a character is made](#3-how-a-character-is-made)
- [4. Why bosses are not simply swappable](#4-why-bosses-are-not-simply-swappable)
- [5. How the Omega mod works](#5-how-the-omega-mod-works)
- [6. Address reference](#6-address-reference)
- [7. Lessons learned and dead ends](#7-lessons-learned-and-dead-ends)
- [8. Tools](#8-tools)
- [9. Adding another character](#9-adding-another-character)

---

## 1. The game files

| What | Where / format |
| --- | --- |
| Executable | `Binaries\TWFC.exe`, 32-bit x86, Unreal Engine 3 (package version 511, licensee 145). It imports `d3d9.dll` and `DINPUT8.dll` (a `dinput8.dll` placed next to it is loaded instead of the system one: that is how the mod loads). Logging is stripped. |
| Config | `TransGame\Config\PC\Cooked\Coalesced.ini`: 14 ini files in one XOR-obfuscated container. The game reads only this file. Format: `int32 count`, then (path, contents) string pairs; each string is `int32 length`, XORed UTF-16 chars and a raw 0 terminator. The key is the UTF-16 string at exe file offset `0x1FFF9B0`; its position carries on across the file. `tools/coalesced.py` decodes and encodes it byte-exact. |
| Packages | `TransGame\CookedPC\*.xxx` (2158 files): UE3 packages, LZO1X-compressed chunks (tag `C1 83 2A 9E`). `tools/upk_decompress.py` decompresses them for reading. |
| DLC packs | The exe loads `<game>\DLC\<Pack>\`. Files named `<Pack>__<IniName>.ini` are merged into the config at startup, so mods can add config without touching Coalesced.ini. The `[Engine.PackagesToFullyLoadForDLC]` section (`.MapName=` + `.Package=` lines) makes a map load extra packages. This is how Omega's packages get into Iacon. (The same section in Coalesced.ini itself is ignored.) |

**Console commands.** The console UI is disabled, but every key binding in `[Engine.PlayerInput]` (PC-TransInput.ini) runs as a
console command, and a `CheatManager` exists in single player. Working: `God`, `Slomo n`, `Teleport`, `giveweapon <Package.Class>`,
`open <map>?options`, `ToggleDebugCamera`, `toggledebugcamerastrategy`, `set <Class> <Property> <value>`. Not present in this build:
`Ghost`, `Fly`, `Walk`, `EnableCheats`. Campaign maps open with `open <Map>?GameTeamStatus=4?IconicMode=1`.

## 2. Engine object system

| What | Value |
| --- | --- |
| GNames | array at `[0x12B2F0F4]`, count `[0x12B2F0F8]`. Entry: `+0` index, `+4` hash next, `+8` UTF-16 name |
| FName | one dword: `index << 14 \| number` (number 0 = no suffix, else `_<number-1>`) |
| GObjects | array at `[0x12B2F0A4]`, count `[0x12B2F0A8]` (about 100k objects) |
| UObject | `+0x00` vtable, `+0x04` Name, `(+0x14 & 0xFFF) - 1` = class id, `(+0x24 >> 4) - 1` = outer's GObjects index |
| UField / UStruct | `+0x2C` Next, `+0x30` Children |
| Property objects | dword `+0x30`: `>> 20` = byte offset of the property, `>> 12 & 0xFF` = size in dwords. Listing a class's properties this way gives you any field's offset. |
| Pointers | many native functions use `object + 0x10` (an interface) as "the pointer", with `0x10` meaning null |
| Class defaults | objects named `Default__<Class>`: skip them when you look for live objects |

Class ids (stable across runs): TnTransformer (ROBODEF) 1879, chassis provider 1743, TnPlayerReplicationInfo 1820,
TnPlayerPawn 1737, TnAiPawn 1732, TnPawn 1720, TnPlayerController 1738 (the live one is subclass 3460), TnAiController 1712,
TnWeapon 1785, SkeletalMeshComponent 336.

**Actors.** Location `+0xD8`, Velocity `+0xF0`, Physics byte `+0x64` (1 walking, 2 falling).
**Pawns.** CylinderComponent `+0x15C` (CollisionRadius `+0x1B0`, CollisionHeight `+0x1B4`), Controller `+0x1D4`,
Health `+0x35C` (float), Mesh `+0x3F4` (AnimSets TArray at mesh `+0x220`), current weapon `+0x428`, TnGrabber `+0x568`, ROBODEF `+0x628`.
**Controller.** Pawn `+0x1C8`, pitch `+0xE4`, yaw `+0xE8` (65536 = 360 degrees), bGodMode = bit `0x2` of dword `+0x1D8`,
TnPlayerTargeting `+0x568` (its `+0x40` = the enemy under the crosshair).

## 3. How a character is made

- The playable character comes from a **chassis** entry in `TransCustomization.ini`, e.g. `[Tank TnDataProvider_Chassis]` (Megatron)
  with `ObjectPath=TR_Megatron_ROBODEF_p.Megatron_ROBODEF`. At runtime the provider is an object of class 1743 named `Tank`; its
  ObjectPath FString is at `+0x78`, and it is looked up by name at every spawn.
- The **ROBODEF** (class TnTransformer) holds the whole character: mesh, anim tree, physics asset, collision size, vehicle form, camera
  set, health, melee set, hit reactions, effects and voice. **Every character, bosses included, is a TnTransformer ROBODEF.**
- Changing a chassis ObjectPath to another full-package ROBODEF in the config works, e.g. Megatron's chassis → Brawl: Brawl's
  robot and vehicle with Megatron's loadout.
- The player's `PlayerReplicationInfo +0x420` holds the ROBODEF in use. The first spawn in a level uses it. Respawns read the chassis path again.

**Where character data lives.** About half the `TR_<Name>_ROBODEF_p.xxx` files are full packages (Megatron, Optimus, Brawl, Soundwave,
Starscream, Jetfire, ...). The rest are 109-byte stubs: those characters only exist inside the levels that use them, cooked under their
original path. Examples: Omega Supreme's ROBODEF in `D5_OME_DSN_m` and his mesh and AnimSets in `D5_OME_BASE_m`; Trypticon in
`A5_TRY_DSN_m`; Starscream (boss) in `A1_IAC_DecoEnd_*`; Frenzy, Rumble and Laserbeak in `A2_KON_Soundwave_Design_m`; Titans and
soldiers in many design levels; Zeta Prime, Shockwave, Arcee and Jazz in multiplayer maps. Run `tools/index_robodefs.py` to list which
package **defines** each ROBODEF.

Boss **attacks** are not in the ROBODEF. They are animations in the boss's AnimSets plus AI data
(`*_AICD` TnAiCharacterDefinition, GOAP actions such as `TnAiActionGrabBlueprint`). The animations carry anim notifies that do the
real work (spawn projectiles, deal damage), so playing a boss animation on the player pawn gives you the real attack.

## 4. Why bosses are not simply swappable

1. **Config swap crashes.** A chassis pointing at a stub ROBODEF crashes every mission load: the game resolves all chassis entries, and the object doesn't exist outside its level.
2. **Possession doesn't work.** `Avatar TnAiPawn` possesses the NPC, then crashes: TnPlayerController's native tick casts its pawn to TnPlayerPawn (`0x10B18130`) and uses the result unchecked, in about 45 places. NPCs and even squadmates are TnAiPawns.
3. **Boss ROBODEFs are body only.** All player-side fields are null, and the player spawn dereferences them (crash at `0x116D2A4B`):

| ROBODEF offset | Megatron | Omega Supreme |
| --- | --- | --- |
| `+0x64` / `+0x68` / `+0x6C` | mesh, physics asset, anim tree | Omega's own |
| `+0x7C` / `+0x80` | collision 200 / 175 | 3100 / 2400 |
| `+0xA4` vehicle form, `+0xA8` camera set, `+0xB0` targeting | set | **null** |
| `+0xD4` acrobatics, `+0xE8` momentum, `+0xF0..0xF8` health | set | **null** |
| `+0x104` melee set, `+0x11C` hit reactions | set | **null** |
| `+0x138`..`+0x16C` interact skeleton, wounded, materials, arm parts | set | **null** |

4. **Checkpoint restarts reload streamed packages**, so a boss ROBODEF edited earlier is replaced by a fresh copy before the spawn. Edits must happen when the spawn reads the chassis path, not before.

## 5. How the Omega mod works

`omega_mod/src/lib.rs` (Rust, built as `dinput8.dll`) is a proxy: it forwards `DirectInput8Create` to the system DLL and starts the mod.
Three code caves run on the game thread. A background thread handles everything that is timed or input-driven.

**Loading Omega into a normal level.** `mod/DLC/DLCMaps/DLCMaps__Xe-TransEngine.ini` makes Iacon (`D3A_IAC_Base_m`) also load
`D5_OME_BASE_m` (mesh), `D5_OME_DSN_m` (ROBODEF and AI data) and `D3B_IAC_MiddleDome_VIG_m` (his missile-launcher animations).

**Spawn (cave at `0x10A801F8`, the chassis ObjectPath read).** When the `Tank` chassis is read and Omega's ROBODEF is loaded:
1. **Graft**: copy Megatron's values into Omega's null player fields `A4 A8 B0 D4 E8 F0 F4 F8 104 138 140 148 14C 150 160 164 168 16C`.
   `+0x11C` (hit reactions) is deliberately left null, otherwise the giant body flinches hard on every hit.
2. Point the chassis ObjectPath at `TR_OmegaSupreme_ROBODEF_p.OmegaSupreme_ROBODEF`, and the PRI `+0x420` too (for the first spawn).
3. Prepare data that is copied when the pawn and weapons are created:
   - **weapons**: Megatron's bazooka (`Bazooka_SP_WEPDATA`) fires `TnProjectileOmegaHomingRocket` (damage 150, radius 1500, scale 3);
   - **sockets**: `WeaponSocket_Primary` moves to the left missile bone, so rockets leave his left arm;
   - **camera**: the robot camera template `OverTheShoulder_STRATEGY` / `TnLocationOffsetCameraBehavior` gets orbit 6000, side 2500 and up 1500;
   - **set-piece missiles**: `TnAnimNotify_SpawnProjectile` objects that fire the set-piece rocket now fire the homing barrage rocket;
   - **melee and grab**: animation name ids, the melee set and Omega's grab description are cached.

In any other level the cave puts Megatron's path back, so the rest of the game is untouched.

**After spawn (background thread).**
- Collision radius 3100 → 800 (`cylinder +0x1B0`), checked every tick.
- Adds the D3B AnimSet `VIG_D4_ROBO_ANIM` to the mesh AnimSets.
- Turns god mode on once: it presses F8 (`God`), learns the bGodMode bit from the before/after value, and from then on presses F8 only while the bit is clear.

**Melee modes (T).** The grafted melee set (ROBODEF `+0x104`) has an Attacks array at `+0x48`, 0x50 bytes per entry (`+0x10` AnimName,
`+0x14` BlendIn, `+0x18` BlendOut, `+0x1C` RootMotionMode, `+0x1D` AnimationMaskType, `+0x1E` AnimationPriority). Entry 4 is used for
both ground and air melee. Writing a boss animation's name there makes Q play it, notifies included:

| Mode | Animation | Notes |
| --- | --- | --- |
| Missile Barrage | `Melee_RocketBarrage` | rockets from his back |
| Missile Gun, long range | `Big_Omega_Missile_4thEnc` (D3B) | `MissileSocket_Left_01..04` pitch `+0x3C` flipped from 49152 to 16384 (the set piece aimed down at the player) |
| Missile Gun, short range | `Omega_2nd_Missile_Shoot` (D3B) | same sockets |
| Ground Pound | `Melee_AOE` | shock wave |

**Rocket targeting (cave at `0x10B39DA0`).** The barrage rocket's target picker uses the AI controller's enemy, else a random player or
squad pawn, so for the player it hit teammates. The cave returns the enemy under the crosshair; with nothing aimed, it round-robins
through an auto-target list (the nearest 4 living Autobots in front, within 30000 units) that the background thread refreshes 4 times a second.

**Tractor grab (F11; cave at `0x117E8670`, CheatManager.Teleport's native wrapper).** Every pawn has a TnGrabber (`pawn +0x568`). Its
state byte (`+0xB4`): 0 Inactive, 1 LockedOn, 2 Lifting, 3 Tractoring, 4 Holding. The interface at grabber `+0x38` has:
- slot `+0x14` = grab (`0x10AB5390`, thiscall, `ret 8`), called with (&GrabDescription, &{victim, 0, victim+0x10 → vtbl[1](L"TnServiceProvider")});
- slot `+0x18` = go Inactive (drops the victim unharmed);
- detach-with-damage = `0x10AB66C0` (thiscall, this = grabber `+0x10`, `ret 4`).

GrabDescription = Omega's `TnAiActionGrabBlueprint +0xD8` (0x38 bytes: lift time 1, tractor speed 3000, attach socket
`WeaponSocket_R`, detach damage 1000, ...). The background thread then plays the sequence:
1. `Shooting_Tell` (his aiming pose) is played as a melee via a simulated Q, and held while the enemy is pulled in (AnimSequence `+0x40` RateScale = 0.05; it is applied every frame);
2. when the grabber reaches Holding, the rest plays at 16x, then `Melee_ClawCrush_Omega` starts;
3. 5.3 s later a simulated F11 throws the enemy with 1000 damage.

**Guide panel (`omega_mod/src/hud.rs`).** The DLL redirects the exe's import of `d3d9!Direct3DCreate9` (IAT slot `0x119488DC`), hooks
`IDirect3D9::CreateDevice` (vtable 16) on the returned object and then `IDirect3DDevice9::Present` (vtable 17) on the game's device.
Before each Present it draws one textured quad on the back buffer, saving the render state and restoring it after. The panel is drawn
with GDI on the mod thread and uploaded as a managed A8R8G8B8 texture, which works with anti-aliasing and survives Alt-Tab.

## 6. Address reference

| Address | What |
| --- | --- |
| `0x10A801F8` | chassis ObjectPath read, `mov eax,[esi+68]` (esi = chassis + 0x10). Original bytes `8B 46 68 EB 05` |
| `0x10B39DA0` | barrage-rocket target picker (EAX = owner + 0x10; returns target + 0x10, or 0x10 for none). Original `83 EC 18 53 56` |
| `0x117E8670` | CheatManager.Teleport native wrapper (thiscall, `ret 8`). Original `57 8B F9 E8 B8` |
| `0x10AB5390` / `0x10AB66C0` / `0x10AB5470` | TnGrabber grab / detach with damage / go Inactive |
| `0x10BCCF10` | TnAnimNotify_SpawnProjectile Notify (only checks owner Role > 2). Notify: `+0x20` socket, `+0x34` ProjectileClass (live) |
| `0x10BA0A6E` | player spawn reads `[pawn+0x568]+0xA8` (the camera set): the null-camera crash site for boss ROBODEFs |
| `0x10B18130` | `Cast<TnPlayerPawn>(Controller.Pawn)` (54 callers; returns 0x10 on failure) |
| `0x110031A0` | `FindClass(&cache, L"Name")` |
| `0x119488DC` | IAT slot `d3d9!Direct3DCreate9` |
| `0x12A03F30` | frame-rate cap float (30 / 60 / 80), from the community FPS unlocker |

## 7. Lessons learned and dead ends

- **Use code caves, not debugger breakpoints.** Cheat Engine's hardware-breakpoint callbacks were dropped or froze the game more than once.
- **The pause-menu "restart from checkpoint" crashed** in the early runtime-swap tests. The death restart was fine. (The DLL doesn't need either.)
- **Memory.** The 32-bit game is close to its 2 GB limit with the boss packages loaded. The DLL never aborts on allocation failure, and the
  installer sets the Large Address Aware flag (4 GB).
- **A melee animation can't interrupt itself.** Priority doesn't help. Setting the upper-body mask with low priority breaks the animation.
- **Camera.** The distance is `TnLocationOffsetCameraBehavior` (`+0x2C` side, `+0x30` up, `+0x34` default orbit, `+0x58` current orbit).
  The `TnScreenSpaceOffsetByPitch` curves only move the aim point, not the distance. Strategy settings are copied when the strategy activates.
- **Weapons copy their settings when created**: edit WEPDATA before the weapon is given or the pawn spawns.
- **Projectile scale is collision**: rockets scaled 10x exploded on cover.
- The **CheatManager is stripped** to God and Teleport (plus engine commands); `Avatar` needs dozens of patches (see section 4).

## 8. Tools

All Python 3. Some need `pip install minidump` or `pip install pefile capstone`.

| Tool | Usage | What it does |
| --- | --- | --- |
| `install.py` | `python install.py "<game>" [--uninstall]` | installs the mod (bindings, DLC pack, DLL, LAA flag) with backups; uninstall restores |
| `tools/coalesced.py` | `decode <Coalesced.ini> <dir>` / `encode <dir> <Coalesced.ini>` | edit the game config |
| `tools/upk_decompress.py` | `<in.xxx> <out.upk>` | decompress a package (for reading/searching) |
| `tools/upk_tables.py` | `<pkg.xxx> [regex]` | name / import / export tables (which package defines what) |
| `tools/upk_props.py` | `<pkg.xxx> <export regex> [max]` | dump the tagged properties of exports (weapon data, blueprints, ...) |
| `tools/index_packages.py` | `<CookedPC> <out.tsv>` | index all packages' name tables |
| `tools/index_robodefs.py` | `<CookedPC> <out.tsv>` | which packages define each `*_ROBODEF` |
| `tools/dump_classes.py` | `<full.dmp> [Class ...]` (set `TWFC_EXE`) | rebuild the native class hierarchy from a full memory dump |
| `tools/class_funcs.py` | `<full.dmp> <Class ...>` | a class's UFunctions and their native code pointers |
| `tools/vtable_diff.py` | `<full.dmp> <ObjectPrefix> <BaseObject>` | which virtual functions a class overrides |
| `tools/robodef_cmp.py` | `<full.dmp> <Name ...> [all]` | compare ROBODEFs field by field (how the graft list was found) |
| `tools/crash_info.py` | `<TWFC.exe.N.dmp>` | crash summary: exception, registers, engine objects, return addresses |
| `tools/x86dis.py` | `<TWFC.exe> <VA:len> ...` | disassemble the exe at virtual addresses |
| `cheat-engine/boss_swap.lua` | Cheat Engine Lua console | the research prototype: every feature as a live function (`omega_before_respawn`, `omega_after_respawn`, `omega_melee`, `omega_real_grab`, camera helpers, ...). Fastest way to try things before porting them to the DLL. |

Also useful: Ghidra (for the exe), Cheat Engine, x32dbg (run the game under it to get a real crash dump; the engine's own crash
handler is disabled when a debugger is attached), and full dumps taken from Task Manager for the dump tools.
Crash dumps are written to `%LOCALAPPDATA%\CrashDumps`.

## 9. Adding another character

What worked for Omega applies to any boss or NPC:
1. **Find its data**: `tools/index_robodefs.py` tells you which level package defines the ROBODEF. Mesh and AnimSets are usually in that level's `*_BASE_m`.
2. **Load it where you play**: a DLC pack `[Engine.PackagesToFullyLoadForDLC]` with `.MapName=<map>` and `.Package=<its packages>`.
3. **Pick a donor chassis** whose campaign character appears in that map (`CampaignCharacters=` in `TransLevels.ini`), e.g. `Tank` = Megatron.
4. **Compare ROBODEFs** (`tools/robodef_cmp.py`, or live in Cheat Engine) and graft the donor's player fields into the null ones at the chassis read.
5. **Camera and collision**: scale the camera orbit to the body size; shrink the collision radius so it fits through the level.
6. **Attacks**: list its animations (AnimSequence names in its AnimSets) and play them through the melee set. Look at the anim notifies to see what they spawn, and find what targets them.
7. Prototype live with `cheat-engine/boss_swap.lua`, then port to the DLL.
