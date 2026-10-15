//! Omega Supreme player mod for Transformers: War for Cybertron (PC, TWFC.exe, ImageBase 0x10900000, no ASLR).
//!
//! Loaded as a dinput8.dll proxy (the game imports DINPUT8.dll for DirectInput8Create only). Standalone port of
//! cheat-engine/boss_swap.lua (no Cheat Engine, no death restart). See docs/TECHNICAL.md for how every
//! offset and function below was found and verified.
//!
//! Game thread (code caves):
//!   * chassis path read (TWFC.exe+0x1801F8): if Omega's packages are loaded (DLC pack, Iacon), point Megatron's
//!     chassis "Tank" at Omega's ROBODEF, graft Megatron's player parts and prepare all data (weapons, sockets,
//!     camera template, melee) before the pawn spawns; otherwise restore Megatron.
//!   * rocket target picker (+0x239DA0): aimed enemy, else the auto-target list.
//!   * CheatManager.Teleport (F11, +0x13E8670): real tractor grab / detach-with-damage.
//! Background thread: new-pawn setup, T melee modes, grab sequence (tractor -> crush -> throw), auto-target list.
//! Render thread: guide panel on the right of the screen (hud.rs, Direct3D 9 Present hook; Home hides it).
#![allow(non_snake_case, static_mut_refs, clippy::missing_safety_doc)]

mod hud;

use core::arch::naked_asm;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::io::Write;
use std::time::Instant;

type HANDLE = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> HANDLE;
    fn ReadProcessMemory(h: HANDLE, base: *const c_void, buf: *mut c_void, size: usize, read: *mut usize) -> i32;
    fn VirtualProtect(addr: *mut c_void, size: usize, newp: u32, old: *mut u32) -> i32;
    fn FlushInstructionCache(h: HANDLE, addr: *const c_void, size: usize) -> i32;
    fn LoadLibraryW(name: *const u16) -> HANDLE;
    fn GetProcAddress(h: HANDLE, name: *const u8) -> *mut c_void;
    fn GetSystemDirectoryW(buf: *mut u16, n: u32) -> u32;
    fn GetModuleFileNameW(h: HANDLE, buf: *mut u16, n: u32) -> u32;
    fn GetCurrentProcessId() -> u32;
    fn Sleep(ms: u32);
}
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn GetForegroundWindow() -> HANDLE;
    fn GetWindowThreadProcessId(h: HANDLE, pid: *mut u32) -> u32;
    fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    fn MapVirtualKeyW(code: u32, map: u32) -> u32;
}

// ------------------------------------------------------------------------------------------------ dinput8 proxy

type Di8Create = unsafe extern "system" fn(HANDLE, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
static mut REAL_DI8: Option<Di8Create> = None;

#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(hinst: HANDLE, ver: u32, riid: *const c_void, out: *mut *mut c_void, outer: *mut c_void) -> i32 {
    if REAL_DI8.is_none() {
        let mut buf = [0u16; 260];
        let n = GetSystemDirectoryW(buf.as_mut_ptr(), 260) as usize;
        let mut path: Vec<u16> = buf[..n].to_vec();
        path.extend("\\dinput8.dll".encode_utf16());
        path.push(0);
        let h = LoadLibraryW(path.as_ptr());
        let f = GetProcAddress(h, b"DirectInput8Create\0".as_ptr());
        if f.is_null() {
            return -1;
        }
        REAL_DI8 = Some(std::mem::transmute::<*mut c_void, Di8Create>(f));
    }
    (REAL_DI8.unwrap())(hinst, ver, riid, out, outer)
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(_h: HANDLE, reason: u32, _r: *mut c_void) -> i32 {
    if reason == 1 {
        // before the game creates its Direct3D device (it does so after all imports are initialised)
        hud::install();
        std::thread::spawn(|| main_thread());
    }
    1
}

// ------------------------------------------------------------------------------------------------ log

fn log(msg: &str) {
    unsafe {
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 260) as usize;
        let exe = String::from_utf16_lossy(&buf[..n]);
        let dir = exe.rsplit_once('\\').map(|x| x.0).unwrap_or(".");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{dir}\\omega_mod.log")) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

// ------------------------------------------------------------------------------------------------ memory

fn rd32(a: u32) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 0usize;
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), a as *const c_void, &mut v as *mut u32 as *mut c_void, 4, &mut n) };
    if ok != 0 && n == 4 { Some(v) } else { None }
}
fn r(a: u32) -> u32 { rd32(a).unwrap_or(0) }
fn rf(a: u32) -> f32 { f32::from_bits(r(a)) }
fn rb(a: u32) -> u8 {
    let mut v = 0u8;
    let mut n = 0usize;
    unsafe { ReadProcessMemory(GetCurrentProcess(), a as *const c_void, &mut v as *mut u8 as *mut c_void, 1, &mut n) };
    v
}
fn rbuf(a: u32, len: usize) -> Option<Vec<u8>> {
    let mut v = vec![0u8; len];
    let mut n = 0usize;
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), a as *const c_void, v.as_mut_ptr() as *mut c_void, len, &mut n) };
    if ok != 0 && n == len { Some(v) } else { None }
}
// Writes only go to objects we just validated by reading; data pages are read-write.
fn w(a: u32, v: u32) { if a > 0x10000 && rd32(a).is_some() { unsafe { std::ptr::write_volatile(a as *mut u32, v) } } }
fn wf(a: u32, v: f32) { w(a, v.to_bits()) }
fn wb(a: u32, v: u8) { if a > 0x10000 && rd32(a).is_some() { unsafe { std::ptr::write_volatile(a as *mut u8, v) } } }

unsafe fn patch(at: u32, bytes: &[u8]) {
    let mut old = 0u32;
    VirtualProtect(at as *mut c_void, bytes.len(), 0x40, &mut old);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
    VirtualProtect(at as *mut c_void, bytes.len(), old, &mut old);
    FlushInstructionCache(GetCurrentProcess(), at as *const c_void, bytes.len());
}
unsafe fn patch_jmp(at: u32, target: u32) {
    let rel = target.wrapping_sub(at + 5);
    let mut b = [0xE9u8, 0, 0, 0, 0];
    b[1..].copy_from_slice(&rel.to_le_bytes());
    patch(at, &b);
}

// ------------------------------------------------------------------------------------------------ objects and names

const GNAMES: u32 = 0x12B2_F0F4;
const GOBJECTS: u32 = 0x12B2_F0A4;
const CID_ROBODEF: u32 = 1879; // TnTransformer
const CID_PC: u32 = 3460; // TnPlayerControllerCampaign (live subclass)
const CID_PRI: u32 = 1820; // TnPlayerReplicationInfo

thread_local! {
    // per thread: the game thread (chassis hook) and the mod thread both resolve names
    static NAME_CACHE: std::cell::RefCell<HashMap<u32, String>> = std::cell::RefCell::new(HashMap::new());
}

fn fname(id: u32) -> String {
    let idx = id >> 14;
    let num = id & 0x3FFF;
    let base = NAME_CACHE.with(|c| {
        if let Some(s) = c.borrow().get(&idx) {
            return s.clone();
        }
        let e = r(r(GNAMES) + idx * 4);
        let s = rbuf(e + 8, 256)
            .map(|b| {
                let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                String::from_utf16_lossy(&u)
            })
            .unwrap_or_else(|| "?".into());
        c.borrow_mut().insert(idx, s.clone());
        s
    });
    if num > 0 { format!("{base}_{}", num - 1) } else { base }
}

#[derive(Clone, Copy)]
struct Obj {
    addr: u32,
    name: u32,
    cls: u32,
    outer: i32,
}

struct Snap {
    objs: Vec<Obj>,
    arr: Vec<u32>,
}

// Copies the object table. The game is 32-bit and close to its address-space limit, so every allocation is
// fallible: on failure the snapshot is empty (callers just find nothing) instead of aborting the process.
fn snapshot() -> Snap {
    let go = r(GOBJECTS);
    let gc = (r(GOBJECTS + 4) as usize).min(400_000);
    let mut arr: Vec<u32> = Vec::new();
    let mut objs: Vec<Obj> = Vec::new();
    if arr.try_reserve_exact(gc).is_err() || objs.try_reserve_exact(gc).is_err() {
        log("snapshot: out of memory, skipped");
        return Snap { objs: Vec::new(), arr: Vec::new() };
    }
    let mut chunk = [0u8; 4096];
    let mut i = 0usize;
    while i < gc {
        let n = (gc - i).min(1024);
        let mut got = 0usize;
        let ok = unsafe {
            ReadProcessMemory(GetCurrentProcess(), (go as usize + i * 4) as *const c_void, chunk.as_mut_ptr() as *mut c_void, n * 4, &mut got)
        };
        if ok == 0 { break; }
        for k in 0..n {
            arr.push(u32::from_le_bytes([chunk[k * 4], chunk[k * 4 + 1], chunk[k * 4 + 2], chunk[k * 4 + 3]]));
        }
        i += n;
    }
    let mut h = [0u8; 0x28];
    for &o in &arr {
        if o == 0 { continue; }
        let mut got = 0usize;
        let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), o as *const c_void, h.as_mut_ptr() as *mut c_void, 0x28, &mut got) };
        if ok == 0 { continue; }
        let g = |k: usize| u32::from_le_bytes([h[k], h[k + 1], h[k + 2], h[k + 3]]);
        objs.push(Obj { addr: o, name: g(4), cls: (g(0x14) & 0xFFF).wrapping_sub(1), outer: (g(0x24) >> 4) as i32 - 1 });
    }
    Snap { objs, arr }
}

impl Snap {
    fn name(&self, o: &Obj) -> String { fname(o.name) }
    fn outer_name(&self, o: &Obj) -> String {
        if o.outer < 0 || o.outer as usize >= self.arr.len() { return String::new(); }
        let p = self.arr[o.outer as usize];
        if p == 0 { String::new() } else { fname(r(p + 4)) }
    }
    fn find(&self, name: &str) -> Option<u32> { self.objs.iter().find(|o| fname(o.name) == name).map(|o| o.addr) }
    fn find_cls(&self, cls: u32, name: &str) -> Option<u32> {
        self.objs.iter().find(|o| o.cls == cls && fname(o.name) == name).map(|o| o.addr)
    }
    fn index_of(&self, addr: u32) -> Option<i32> { self.arr.iter().position(|&a| a == addr).map(|i| i as i32) }
}

// ------------------------------------------------------------------------------------------------ shared state

// Auto-target list read by the target cave: [count, round-robin index, up to 8 pawns].
#[no_mangle]
static mut OMEGA_FB: [u32; 10] = [0; 10];
// Grab cave data: [pc, desc ptr, holding, calls]; victim ref {victim, 0, service provider}; L"TnServiceProvider".
#[no_mangle]
static mut OMEGA_GRAB: [u32; 4] = [0; 4];
#[no_mangle]
static mut OMEGA_REF: [u32; 3] = [0; 3];
#[no_mangle]
static mut OMEGA_WSTR: [u16; 18] = [0; 18];
#[no_mangle]
static mut OMEGA_DESC: [u8; 0x40] = [0; 0x40];
// Omega's ROBODEF path for the chassis FString (never freed).
static mut OMEGA_PATH: [u16; 64] = [0; 64];
static mut TANK_ORIG: Option<(u32, u32, u32)> = None;
static mut OMEGA_LOADED: bool = false;

const OMEGA_PATH_STR: &str = "TR_OmegaSupreme_ROBODEF_p.OmegaSupreme_ROBODEF";
// ROBODEF player fields boss ROBODEFs leave empty (docs/TECHNICAL.md, "graft"). Not grafted: +0x11C HitReactionBlueprint
// (Megatron's RobotHitReactions made the giant's body flinch hard on every hit; Omega keeps none).
const GRAFT: [u32; 18] = [0xA4, 0xA8, 0xB0, 0xD4, 0xE8, 0xF0, 0xF4, 0xF8, 0x104, 0x138, 0x140, 0x148, 0x14C, 0x150, 0x160, 0x164, 0x168, 0x16C];

// ------------------------------------------------------------------------------------------------ caves

// Chassis path read: TWFC.exe+0x1801F8 `mov eax,[esi+68]` (esi = chassis+0x10; +0x68 = ObjectPath FString data).
#[unsafe(naked)]
unsafe extern "C" fn chassis_cave() {
    naked_asm!(
        "pushad",
        "pushfd",
        "lea eax, [esp+0x24]",
        "push eax",
        "push esi",
        "call {f}",
        "add esp, 8",
        "popfd",
        "popad",
        "mov eax, [esi+0x68]",
        "push 0x10A80202",
        "ret",
        f = sym on_chassis_read,
    );
}

// Rocket target picker TWFC.exe+0x239DA0 (EAX = owner pawn + 0x10, returns target + 0x10 or 0x10 = none).
#[unsafe(naked)]
unsafe extern "C" fn target_cave() {
    naked_asm!(
        "cmp eax, 0x10",
        "je 29f",
        "mov ecx, [eax+0x1C4]",
        "test ecx, ecx",
        "je 29f",
        "mov edx, [ecx+0x14]",
        "and edx, 0xFFF",
        "cmp edx, 0xD85",
        "jne 29f",
        "mov edx, [ecx+0x568]",
        "test edx, edx",
        "je 22f",
        "mov eax, [edx+0x40]",
        "test eax, eax",
        "je 22f",
        "add eax, 0x10",
        "ret",
        "22:",
        "mov ecx, dword ptr [{fb}]",
        "test ecx, ecx",
        "je 23f",
        "mov eax, dword ptr [{fb}+4]",
        "inc eax",
        "cmp eax, ecx",
        "jb 24f",
        "xor eax, eax",
        "24:",
        "mov dword ptr [{fb}+4], eax",
        "mov eax, dword ptr [{fb}+eax*4+8]",
        "test eax, eax",
        "je 23f",
        "add eax, 0x10",
        "ret",
        "23:",
        "mov eax, 0x10",
        "ret",
        "29:",
        "sub esp, 0x18",
        "push ebx",
        "push esi",
        "push 0x10B39DA5",
        "ret",
        fb = sym OMEGA_FB,
    );
}

// F11 = CheatManager.Teleport native wrapper TWFC.exe+0x13E8670 (thiscall, ret 8): real tractor grab / throw.
#[unsafe(naked)]
unsafe extern "C" fn grab_cave() {
    naked_asm!(
        "pushad",
        "mov esi, dword ptr [{g}]",
        "test esi, esi",
        "je 38f",
        "mov eax, [esi+0x1C8]",
        "test eax, eax",
        "je 38f",
        "mov ebx, [eax+0x568]",
        "test ebx, ebx",
        "je 38f",
        "add ebx, 0x38",
        "cmp dword ptr [{g}+8], 0",
        "jne 36f",
        "mov edx, [esi+0x568]",
        "test edx, edx",
        "je 38f",
        "mov edi, [edx+0x40]",
        "test edi, edi",
        "je 38f",
        "mov dword ptr [{rf}], edi",
        "mov dword ptr [{rf}+4], 0",
        "lea ecx, [edi+0x10]",
        "mov eax, [ecx]",
        "lea edx, [{ws}]",
        "push edx",
        "call dword ptr [eax+4]",
        "test eax, eax",
        "je 38f",
        "mov dword ptr [{rf}+8], eax",
        "mov ecx, ebx",
        "lea edx, [{rf}]",
        "push edx",
        "push dword ptr [{g}+4]",
        "mov eax, [ecx]",
        "call dword ptr [eax+0x14]",
        "mov dword ptr [{g}+8], 1",
        "inc dword ptr [{g}+12]",
        "jmp 38f",
        "36:",
        "cmp byte ptr [ebx+0x7C], 4",
        "jne 37f",
        "lea ecx, [ebx-0x28]",
        "push 0",
        "mov eax, 0x10AB66C0",
        "call eax",
        "37:",
        "mov ecx, ebx",
        "mov eax, [ecx]",
        "call dword ptr [eax+0x18]",
        "mov dword ptr [{g}+8], 0",
        "inc dword ptr [{g}+12]",
        "38:",
        "popad",
        "ret 8",
        g = sym OMEGA_GRAB,
        rf = sym OMEGA_REF,
        ws = sym OMEGA_WSTR,
    );
}

unsafe fn install_caves() {
    // expected original bytes (refuse to patch an unknown exe)
    let checks: [(u32, &[u8]); 3] = [
        (0x10A8_01F8, &[0x8B, 0x46, 0x68, 0xEB, 0x05]),
        (0x10B3_9DA0, &[0x83, 0xEC, 0x18, 0x53, 0x56]),
        (0x117E_8670, &[0x57, 0x8B, 0xF9, 0xE8, 0xB8]),
    ];
    for (a, b) in checks {
        if rbuf(a, 5).as_deref() != Some(b) {
            log(&format!("unexpected bytes at {a:08X}; not patching (wrong TWFC.exe?)"));
            return;
        }
    }
    for (i, c) in "TnServiceProvider".encode_utf16().enumerate() {
        OMEGA_WSTR[i] = c;
    }
    patch_jmp(0x10A8_01F8, chassis_cave as usize as u32);
    patch_jmp(0x10B3_9DA0, target_cave as usize as u32);
    patch_jmp(0x117E_8670, grab_cave as usize as u32);
    log("caves installed: chassis path, rocket targets, F11 grab");
}

// ------------------------------------------------------------------------------------------------ game-thread setup

extern "C" fn on_chassis_read(esi: u32, stack: u32) {
    // diagnostic: return addresses into TWFC.exe on the stack (who reads the chassis path)
    if fname(r(esi.wrapping_sub(0x10) + 4)) == "Tank" {
        let callers: Vec<String> = (0..64u32)
            .map(|i| r(stack + i * 4))
            .filter(|&v| (0x1090_1000..0x1250_0000).contains(&v))
            .take(8)
            .map(|v| format!("{v:08X}"))
            .collect();
        log(&format!("chassis read by {}", callers.join(" ")));
    }
    // never let a panic unwind into the game
    let _ = std::panic::catch_unwind(|| chassis_read(esi));
}

static mut PREPARED: (u32, u32) = (0, 0); // (Omega ROBODEF, its FName) we already prepared

fn chassis_read(esi: u32) {
    let ch = esi.wrapping_sub(0x10);
    if fname(r(ch + 4)) != "Tank" {
        return;
    }
    // already prepared for this Omega ROBODEF (same level): only re-point the path, no object scan
    unsafe {
        let (b, n) = PREPARED;
        if b != 0 && r(b + 4) == n && OMEGA_LOADED {
            let our = OMEGA_PATH.as_ptr() as u32;
            if r(ch + 0x78) != our {
                if r(ch + 0x78) != 0 { TANK_ORIG = Some((r(ch + 0x78), r(ch + 0x7C), r(ch + 0x80))); }
                let len = OMEGA_PATH_STR.len() as u32 + 1;
                w(ch + 0x78, our); w(ch + 0x7C, len); w(ch + 0x80, len);
            }
            return;
        }
    }
    let s = snapshot();
    let omega = s.find_cls(CID_ROBODEF, "OmegaSupreme_ROBODEF");
    let mega = s.find_cls(CID_ROBODEF, "Megatron_ROBODEF");
    unsafe {
        let our = OMEGA_PATH.as_ptr() as u32;
        let cur = (r(ch + 0x78), r(ch + 0x7C), r(ch + 0x80));
        match (omega, mega) {
            (Some(b), Some(d)) => {
                if cur.0 != our {
                    TANK_ORIG = Some(cur);
                }
                for (i, c) in OMEGA_PATH_STR.encode_utf16().enumerate() {
                    OMEGA_PATH[i] = c;
                }
                let n = OMEGA_PATH_STR.len() as u32 + 1;
                w(ch + 0x78, our);
                w(ch + 0x7C, n);
                w(ch + 0x80, n);
                let mut grafted = 0;
                for off in GRAFT {
                    if r(b + off) == 0 {
                        w(b + off, r(d + off));
                        grafted += 1;
                    }
                }
                OMEGA_LOADED = true;
                // the first spawn takes the character from the player's PlayerReplicationInfo (+0x420 = ROBODEF in use,
                // set during loading); the chassis path is only re-read on a respawn. Point it at Omega too.
                for o in s.objs.iter().filter(|o| o.cls == CID_PRI) {
                    if r(o.addr + 0x420) == d {
                        w(o.addr + 0x420, b);
                        log("PlayerReplicationInfo ROBODEF Megatron -> Omega");
                    }
                }
                prepare_data(&s, b);
                PREPARED = (b, r(b + 4));
                log(&format!("chassis Tank -> Omega Supreme ({grafted} fields grafted)"));
            }
            _ => {
                if cur.0 == our {
                    if let Some(o) = TANK_ORIG {
                        w(ch + 0x78, o.0);
                        w(ch + 0x7C, o.1);
                        w(ch + 0x80, o.2);
                    }
                    log("Omega not loaded here: chassis Tank restored to Megatron");
                }
                OMEGA_LOADED = false;
                PREPARED = (0, 0);
            }
        }
    }
}

// Data edits that must exist before Omega's weapons/camera/mesh are created (ported from omega_weapons,
// omega_left_arm_sockets, boss_camera_giant template, omega_add_d3b_anims rocket switch).
fn prepare_data(s: &Snap, omega: u32) {
    // weapons: rocket arm data -> projectile rockets; bazooka fires Omega's homing rocket; rocket damage/size
    let ra = s.find("OMEGA_Arm_AI_WEPDATA");
    let sp = s.find("Bazooka_SP_WEPDATA");
    let rocket = s.find("TnProjectileOmegaHomingRocket");
    if let (Some(ra), Some(rocket)) = (ra, rocket) {
        let (f, n) = (r(ra + 0x28), r(ra + 0x2C));
        for i in 0..n.min(8) { wb(f + i, 1); }
        let (a, n) = (r(ra + 0x34), r(ra + 0x38));
        for i in 0..n.min(8) { w(a + i * 4, rocket); }
        if let Some(sp) = sp {
            w(ra + 0x160, r(sp + 0x160));
            let (b, n) = (r(sp + 0x34), r(sp + 0x38));
            for i in 0..n.min(8) { w(b + i * 4, rocket); }
        }
    }
    if let Some(pd) = s.find("OMEGA_HomingRocket_AI_PROJDATA") { wf(pd + 0x44, 150.0); wf(pd + 0x48, 1500.0); }
    if let Some(pm) = s.find("OMEGA_HomingRocket_AI_PROJMESH") { wf(pm + 0x7C, 3.0); }
    if let (Some(dr), Some(df)) = (s.find("Default__TnWeaponOmegaRocketArm"), s.find("Default__TnWeaponOmegaShootingFrenzyArm")) {
        w(dr + 0x28C, r(df + 0x28C));
    }
    // sockets: gun socket (WeaponSocket_Primary) to the left missile bone; WeaponSocket_R stays (grab hold point)
    if let Some(skel) = s.find("RB_OmegaSupreme_SKEL") {
        if let Some(si) = s.index_of(skel) {
            let socks: Vec<(String, u32)> = s.objs.iter().filter(|o| o.outer == si).map(|o| (fname(r(o.addr + 0x28)), o.addr)).collect();
            let src = socks.iter().find(|x| x.0 == "MissileSocket_Left_01").map(|x| x.1);
            if let (Some(src), Some(p)) = (src, socks.iter().find(|x| x.0 == "WeaponSocket_Primary").map(|x| x.1)) {
                w(p + 0x2C, r(src + 0x2C));
                wf(p + 0x30, 0.0); wf(p + 0x34, 0.0); wf(p + 0x38, -800.0);
                for k in [0x3C, 0x40, 0x44] { w(p + k, r(src + k)); }
            }
            st().missile_socks = socks.iter().filter(|x| x.0.starts_with("MissileSocket_Left_0")).map(|x| (x.1, r(x.1 + 4))).collect();
        }
    }
    // camera: the robot camera template (copied into each new pawn's strategy) -> giant orbit
    for o in &s.objs {
        if s.name(o).starts_with("TnLocationOffsetCameraBehavior_") && rf(o.addr + 0x34) == 800.0 && s.outer_name(o) == "OverTheShoulder_STRATEGY" {
            wf(o.addr + 0x34, 6000.0); wf(o.addr + 0x2C, 2500.0); wf(o.addr + 0x30, 1500.0);
        }
    }
    // D3B missile spawns: set-piece rocket (no targeting) -> homing barrage rocket (ProjectileClass at +0x34)
    if let (Some(vig), Some(manga)) = (s.find("TnProjectileOmegaHomingRocketVignette"), s.find("TnProjectileMangaRocketOmegaSupreme")) {
        for o in &s.objs {
            if r(o.addr + 0x34) == vig && s.name(o).starts_with("TnAnimNotify_SpawnProjectile") { w(o.addr + 0x34, manga); }
        }
    }
    // animation name ids (SequenceName at AnimSequence +0x28) for melee
    let mut ids = BTreeMap::new();
    for o in &s.objs {
        if s.name(o).starts_with("AnimSequence_") {
            let id = r(o.addr + 0x28);
            let nm = fname(id);
            if nm == "Shooting_Tell" { st().tell_seq = (o.addr, o.name); }
            ids.insert(nm, id);
        }
    }
    // grab description: Omega's TnAiActionGrabBlueprint (+0xD8, 0x38 bytes) copied for the F11 cave
    for o in &s.objs {
        if s.name(o).starts_with("TnAiActionGrabBlueprint_") && rf(o.addr + 0xEC) == 5000.0 && rf(o.addr + 0xFC) == 1000.0 {
            if let Some(d) = rbuf(o.addr + 0xD8, 0x38) {
                unsafe {
                    OMEGA_DESC[..0x38].copy_from_slice(&d);
                    OMEGA_GRAB[1] = OMEGA_DESC.as_ptr() as u32;
                }
            }
        }
    }
    let d3b = s.objs.iter()
        .find(|o| fname(o.name) == "VIG_D4_ROBO_ANIM" && s.outer_name(o) == "TR_OmegaSupreme_ANM_p")
        .map(|o| o.addr)
        .unwrap_or(0);
    {
        let mut g = st();
        g.anim_ids = ids;
        g.melee_set = r(omega + 0x104);
        g.d3b_set = d3b;
        g.omega_robodef = omega;
    }
    melee_defaults();
}

// ------------------------------------------------------------------------------------------------ background state

struct State {
    omega_robodef: u32,
    melee_set: u32,
    anim_ids: BTreeMap<String, u32>,
    tell_seq: (u32, u32),
    d3b_set: u32,
    missile_socks: Vec<(u32, u32)>,
    mode: usize,
}
static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State {
    omega_robodef: 0,
    melee_set: 0,
    anim_ids: BTreeMap::new(),
    tell_seq: (0, 0),
    d3b_set: 0,
    missile_socks: Vec::new(),
    mode: 0,
});
fn st() -> std::sync::MutexGuard<'static, State> { STATE.lock().unwrap_or_else(|e| e.into_inner()) }

// T cycles these (the guide panel shows the active one). Long/short range use the D3B launcher animations with the sockets flipped up.
const MODES: [(&str, &str); 4] = [
    ("barrage", "Melee_RocketBarrage"),
    ("missile", "Big_Omega_Missile_4thEnc"),
    ("missile_short", "Omega_2nd_Missile_Shoot"),
    ("aoe", "Melee_AOE"),
];
// what the guide panel shows for each mode
const MODE_TEXT: [&str; 4] = ["Missile Barrage", "Missile Gun - long range", "Missile Gun - short range", "Ground Pound (AOE)"];

fn entry(i: u32) -> u32 { let ms = st().melee_set; if ms == 0 { 0 } else { r(ms + 0x48) + i * 0x50 } }
fn set_melee(i: u32, anim: &str) {
    let id = st().anim_ids.get(anim).copied();
    let e = entry(i);
    if let (Some(id), true) = (id, e != 0) { w(e + 0x10, id); }
}
fn melee_defaults() {
    set_melee(6, "Melee_AOE");
    set_melee(7, "Melee_AOE");
    set_melee(8, "Melee_AOE");
    let m = st().mode;
    set_melee(4, MODES[m].1);
    missile_sockets(MODES[m].0.starts_with("missile"));
}
fn missile_sockets(up: bool) {
    let socks = st().missile_socks.clone();
    for (a, n) in socks {
        if r(a + 4) == n { w(a + 0x3C, if up { 16384 } else { 49152 }); }
    }
}
fn press(vk: u8) {
    unsafe {
        let sc = MapVirtualKeyW(vk as u32, 0) as u8;
        keybd_event(vk, sc, 0, 0);
        Sleep(30);
        keybd_event(vk, sc, 2, 0);
    }
}
fn game_in_front() -> bool {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
        pid == GetCurrentProcessId()
    }
}

// Per-pawn setup (new mesh / camera strategy / collision on every respawn).
fn setup_pawn(pc: u32, pawn: u32, s: &Snap) {
    // live camera copies
    for o in &s.objs {
        if s.name(o).starts_with("TnLocationOffsetCameraBehavior_") && rf(o.addr + 0x34) == 800.0 && rf(o.addr + 0x58) == 800.0
            && s.outer_name(o).starts_with("HmCameraStrategy_") {
            wf(o.addr + 0x34, 6000.0); wf(o.addr + 0x58, 6000.0); wf(o.addr + 0x2C, 2500.0); wf(o.addr + 0x30, 1500.0);
        }
    }
    // collision radius 3100 -> 800
    keep_collision(pawn);
    // D3B animation set into the mesh AnimSets (array of AnimSets whose first entry is OmegaSupreme_ROBO)
    let set = st().d3b_set;
    let mesh = r(pawn + 0x3F4);
    if set != 0 && mesh != 0 {
        for k in (0x60..0x400).step_by(4) {
            let (data, n, max) = (r(mesh + k), r(mesh + k + 4), r(mesh + k + 8));
            if data != 0 && n > 0 && n <= 32 && max >= n && max <= 64 && fname(r(r(data) + 4)) == "OmegaSupreme_ROBO" {
                let present = (0..n).any(|j| r(data + j * 4) == set);
                if !present && max > n {
                    w(data + n * 4, set);
                    w(mesh + k + 4, n + 1);
                }
                break;
            }
        }
    }
    unsafe { OMEGA_GRAB[0] = pc; OMEGA_GRAB[2] = 0; }
    melee_defaults();
    log("Omega pawn set up (camera, collision, D3B animations, melee)");
}

// Collision radius 3100 -> 800 (Omega's own radius blocks him everywhere). The game resets it after spawn and on
// transform, so the mod loop re-applies it every tick.
fn keep_collision(pawn: u32) {
    let cyl = r(pawn + 0x15C);
    if cyl != 0 && rf(cyl + 0x1B0) != 800.0 {
        log(&format!("collision radius {} -> 800 (height {})", rf(cyl + 0x1B0), rf(cyl + 0x1B4)));
        wf(cyl + 0x1B0, 800.0);
    }
}

// Controller.bGodMode: byte offset of its dword from the property object (dword +0x30 >> 20, docs/TECHNICAL.md).
fn god_offset(s: &Snap) -> u32 {
    s.objs.iter()
        .find(|o| s.name(o) == "bGodMode" && s.outer_name(o) == "Controller")
        .map(|o| r(o.addr + 0x30) >> 20)
        .unwrap_or(0)
}

fn main_thread() {
    std::panic::set_hook(Box::new(|info| log(&format!("PANIC (caught): {info}"))));
    std::thread::sleep(std::time::Duration::from_secs(3));
    log("omega_mod loaded");
    unsafe { install_caves(); }
    loop {
        // a panic in the loop is logged and the loop restarts; the game keeps running
        if std::panic::catch_unwind(mod_loop).is_ok() { break; }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn mod_loop() {
    let t0 = Instant::now();
    let now = || t0.elapsed().as_secs_f32();
    let mut pc = 0u32;
    let mut pc_name = 0u32;
    let mut last_pawn = 0u32;
    let mut t_pc = -10.0f32;
    let mut t_scan = -10.0f32;
    let mut t_at = 0.0f32;
    let mut cands: Vec<(u32, u32)> = Vec::new();
    let mut t_was_down = false;
    let mut home_was_down = false;
    let mut hud_hidden = false;
    // god mode on every spawn: F8 runs the game's own God command; its bit inside the bGodMode dword is learned
    // from the first press (value before vs after), then only pressed while the bit is clear.
    let (mut god_off, mut god_mask) = (0u32, 0u32);
    let mut god_pawn = 0u32; // pawn already handled
    let mut god_try: Option<(f32, u32)> = None; // (press time, dword before)
    let mut t_spawn = 0.0f32;
    // grab sequence
    #[derive(PartialEq)]
    enum Ph { Idle, Pull, Finish, Crush, Thrown }
    impl Ph {
        // index into hud::GRAB_TEXT
        fn hud(&self) -> usize { match self { Ph::Idle => 0, Ph::Pull | Ph::Finish => 1, Ph::Crush => 2, Ph::Thrown => 3 } }
    }
    let mut ph = Ph::Idle;
    let (mut t_start, mut t_next, mut t_crush) = (0.0f32, 0.0f32, 0.0f32);
    let mut t_hold: Option<f32> = None;
    let mut grab_pawn = 0u32;

    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let tn = now();
        // player controller (re-validate; a level change frees everything)
        if pc == 0 || r(pc + 4) != pc_name || (r(pc + 0x14) & 0xFFF).wrapping_sub(1) != CID_PC {
            if tn - t_pc < 1.0 { continue; }
            t_pc = tn;
            let s = snapshot();
            // the live controller, not the class default object (Default__TnPlayerControllerCampaign)
            pc = s.objs.iter().filter(|o| o.cls == CID_PC && !s.name(o).starts_with("Default__")).last().map(|o| o.addr).unwrap_or(0);
            pc_name = if pc != 0 { r(pc + 4) } else { 0 };
            if pc != 0 { log(&format!("player controller {} at {pc:08X}", fname(pc_name))); }
            hud::SHOW.store(false, std::sync::atomic::Ordering::Relaxed);
            last_pawn = 0;
            unsafe { OMEGA_GRAB[0] = 0; OMEGA_GRAB[2] = 0; OMEGA_FB[0] = 0; }
            cands.clear();
            continue;
        }
        let pawn = r(pc + 0x1C8);
        let active = unsafe { OMEGA_LOADED } && pawn != 0 && r(pawn + 0x628) == st().omega_robodef;
        // Home: hide / show the guide panel
        let home_down = unsafe { GetAsyncKeyState(0x24) } as u16 & 0x8000 != 0;
        if home_down && !home_was_down && game_in_front() { hud_hidden = !hud_hidden; }
        home_was_down = home_down;
        hud::SHOW.store(active && !hud_hidden, std::sync::atomic::Ordering::Relaxed);
        if !active { continue; }
                if pawn != last_pawn {
            last_pawn = pawn;
            t_spawn = tn;
            let s = snapshot();
            setup_pawn(pc, pawn, &s);
            if god_off == 0 {
                god_off = god_offset(&s);
                log(&format!("bGodMode dword at controller +0x{god_off:X}"));
            }
        }
        keep_collision(pawn);

        // god mode (2 s after spawn, game in front so the F8 press reaches it; retried while ignored, e.g. cutscene)
        if let Some((t, before)) = god_try {
            if tn - t > 0.4 {
                god_try = None;
                let after = r(pc + god_off);
                let diff = before ^ after;
                if diff.count_ones() == 1 {
                    god_mask = diff;
                    log(&format!("bGodMode mask {diff:08X}"));
                    if after & diff == 0 { press(0x77); } // it was on: turn it back on
                    god_pawn = pawn;
                } else if diff != 0 {
                    log(&format!("bGodMode: unexpected change {before:08X} -> {after:08X}; auto god mode off"));
                    god_pawn = pawn;
                    god_off = 0;
                }
            }
        } else if god_off != 0 && god_pawn != pawn && tn - t_spawn > 2.0 && game_in_front() {
            if god_mask != 0 {
                if r(pc + god_off) & god_mask == 0 { press(0x77); }
                god_pawn = pawn;
            } else {
                god_try = Some((tn, r(pc + god_off)));
                press(0x77);
                t_spawn = tn - 1.0; // retry in 1 s if the press was ignored
            }
        }
        let god = if god_mask == 0 { 0 } else if r(pc + god_off) & god_mask != 0 { 2 } else { 1 };
        hud::update(hud::View { mode: st().mode, grab: ph.hud(), god }, &MODE_TEXT);

        // T: melee mode cycle (only while the game window is in front)
        let t_down = unsafe { GetAsyncKeyState(0x54) } as u16 & 0x8000 != 0;
        if t_down && !t_was_down && game_in_front() && unsafe { OMEGA_GRAB[2] } == 0 {
            let m = { let mut g = st(); g.mode = (g.mode + 1) % MODES.len(); g.mode };
            set_melee(4, MODES[m].1);
            missile_sockets(MODES[m].0.starts_with("missile"));
        }
        t_was_down = t_down;

        // auto-target list: nearest 4 living Autobots in front within 30000 (candidates rescanned every 3 s)
        if tn - t_scan > 3.0 {
            t_scan = tn;
            cands.clear();
            let go = r(GOBJECTS);
            let gc = (r(GOBJECTS + 4)).min(400_000);
            for i in 0..gc {
                let o = r(go + i * 4);
                if o == 0 { continue; }
                let nm = r(o + 4);
                if !fname(nm).starts_with("TnAiPawn_") { continue; }
                let rd = r(o + 0x628);
                if rd != 0 && fname(r(rd + 4)).contains("Aut") && cands.len() < 256 { cands.push((o, nm)); }
            }
        }
        if tn - t_at > 0.25 {
            t_at = tn;
            let (px, py) = (rf(pawn + 0xD8), rf(pawn + 0xDC));
            let yaw = (r(pc + 0xE8) & 0xFFFF) as f32 * std::f32::consts::PI / 32768.0;
            let (fx, fy) = (yaw.cos(), yaw.sin());
            let mut list: Vec<(u32, f32)> = cands.iter()
                .filter(|(a, n)| r(a + 4) == *n && rf(a + 0x35C) > 0.0)
                .filter_map(|(a, _)| {
                    let (dx, dy) = (rf(a + 0xD8) - px, rf(a + 0xDC) - py);
                    let d = (dx * dx + dy * dy).sqrt();
                    (d > 1.0 && d < 30000.0 && (dx * fx + dy * fy) / d > 0.3).then_some((*a, d))
                })
                .collect();
            list.sort_by(|a, b| a.1.total_cmp(&b.1));
            let n = list.len().min(4);
            unsafe {
                for (i, x) in list.iter().take(n).enumerate() { OMEGA_FB[2 + i] = x.0; }
                OMEGA_FB[0] = n as u32;
            }
        }

        // grab sequence (F11): aiming pose held while pulling -> claw crush -> throw at 5.3 s (docs/TECHNICAL.md)
        let holding = unsafe { OMEGA_GRAB[2] } == 1;
        let (seq, seq_name) = st().tell_seq;
        let seq_ok = seq != 0 && r(seq + 4) == seq_name;
        let reset = || {
            if seq_ok { wf(seq + 0x40, 1.0); }
            wf(entry(4) + 0x18, 0.35);
            let m = st().mode;
            set_melee(4, MODES[m].1);
        };
        if !holding {
            if ph != Ph::Idle { ph = Ph::Idle; reset(); }
            continue;
        }
        let grabber = r(pawn + 0x568);
        let gstate = if grabber != 0 { rb(grabber + 0xB4) } else { 0 };
        if ph != Ph::Idle && (pawn != grab_pawn || (gstate == 0 && tn - t_start > 1.5)) {
            unsafe { OMEGA_GRAB[2] = 0; }
            ph = Ph::Idle;
            reset();
            continue;
        }
        if !seq_ok { continue; }
        match ph {
            Ph::Idle => {
                ph = Ph::Pull;
                grab_pawn = pawn;
                t_start = tn;
                t_hold = None;
                wf(seq + 0x40, 1.0);
                set_melee(4, "Shooting_Tell");
                wf(entry(4) + 0x18, 0.05);
                press(0x51); // Q
            }
            Ph::Pull => {
                if t_hold.is_none() && tn - t_start >= 2.0 {
                    wf(seq + 0x40, 0.05);
                    t_hold = Some(tn);
                }
                if gstate == 4 {
                    let pos = match t_hold { Some(h) => 2.0 + (tn - h) * 0.05, None => tn - t_start };
                    wf(seq + 0x40, 16.0);
                    t_next = tn + (rf(seq + 0x38) - pos).max(0.0) / 16.0 + 0.05;
                    ph = Ph::Finish;
                }
            }
            Ph::Finish if tn >= t_next => {
                wf(seq + 0x40, 1.0);
                wf(entry(4) + 0x18, 0.35);
                set_melee(4, "Melee_ClawCrush_Omega");
                press(0x51);
                t_crush = tn;
                ph = Ph::Crush;
            }
            Ph::Crush if tn - t_crush >= 5.3 => {
                ph = Ph::Thrown;
                press(0x7A); // F11 -> cave release (detach with 1000 damage)
            }
            _ => {}
        }
    }
}
