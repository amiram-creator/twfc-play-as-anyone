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
    fn AddVectoredExceptionHandler(first: u32, handler: extern "system" fn(*const u32) -> i32) -> *mut c_void;
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
const CID_CHASSIS: u32 = 1743; // TnDataProvider_Chassis
// player controller / replication info classes: campaign, and Escalation (TnSurvivalGame) which has its own subclasses
const CID_PCS: [u32; 2] = [3460, 3464]; // TnPlayerControllerCampaign, TnPlayerControllerSurvival
const CID_PRIS: [u32; 2] = [1820, 3465]; // TnPlayerReplicationInfo, TnPlayerReplicationInfoSurvival

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

// FName id (idx << 14, number 0) of an existing name; full name-table scan, so only for one-off setup
fn name_id(want: &str) -> Option<u32> {
    let (tab, n) = (r(GNAMES), r(GNAMES + 4));
    (0..n).find(|&i| { let e = r(tab + i * 4); e != 0 && fname(i << 14) == want }).map(|i| i << 14)
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
// The chassis Tank's original ObjectPath text (restored on maps without Omega).
static mut TANK_ORIG: Option<String> = None;
static mut OMEGA_LOADED: bool = false;

const OMEGA_PATH_STR: &str = "TR_OmegaSupreme_ROBODEF_p.OmegaSupreme_ROBODEF";
// ROBODEF player fields boss ROBODEFs leave empty (docs/TECHNICAL.md, "graft"). Not grafted: +0x11C HitReactionBlueprint
// (Megatron's RobotHitReactions made the giant's body flinch hard on every hit; Omega keeps none). Not grafted: +0x164..0x16C
// BuffMaterials, a TArray header: copying it made both ROBODEFs own one heap buffer, freed twice when Iacon unloads -> corrupted
// allocator -> crash on the next chapter load (docs/TECHNICAL.md). Only object references are safe to copy.
const GRAFT: [u32; 15] = [0xA4, 0xA8, 0xB0, 0xD4, 0xE8, 0xF0, 0xF4, 0xF8, 0x104, 0x138, 0x140, 0x148, 0x14C, 0x150, 0x160];

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

// Sky arena (docs/TECHNICAL.md): [factory, pending, spawned actor, calls, location xyz, rotation pyr]. F12 is bound to
// HmCheatManager.ToggleDebugCamera; that UFunction's native pointer (+0x68) is redirected here (not the code: script
// functions share it). With `pending` set it spawns the floor via the factory's CreateActor (vtable +0x12C); otherwise
// it does nothing (the debug camera is not used).
#[no_mangle]
static mut ARENA: [u32; 10] = [0; 10];

#[unsafe(naked)]
unsafe extern "C" fn arena_cave() {
    naked_asm!(
        "pushad",
        "cmp dword ptr [{a}+4], 0",
        "je 2f",
        "mov dword ptr [{a}+4], 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "lea eax, [{a}+28]",
        "push eax",
        "lea eax, [{a}+16]",
        "push eax",
        "mov ecx, dword ptr [{a}]",
        "mov eax, [ecx]",
        "call dword ptr [eax+0x12C]",
        "mov dword ptr [{a}+8], eax",
        "inc dword ptr [{a}+12]",
        "2:",
        "popad",
        "ret 8",
        a = sym ARENA,
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
    AddVectoredExceptionHandler(1, crash_logger);
}

// Crash logger: the game's crash dumps only show its shutdown code (a destructor at TWFC.exe+0x8A6D53), not the
// real fault. Log the first few access violations (first chance) with registers and TWFC.exe return addresses.
static CRASH_LOGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
extern "system" fn crash_logger(ptrs: *const u32) -> i32 {
    unsafe {
        let rec = *ptrs as *const u32;
        let ctx = *ptrs.add(1) as *const u32;
        if *rec != 0xC000_0005 || CRASH_LOGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 6 {
            return 0; // EXCEPTION_CONTINUE_SEARCH
        }
        let c = |off: usize| *ctx.add(off / 4);
        let (eip, esp) = (c(0xB8), c(0xC4));
        let mut ret = Vec::new();
        for i in 0..256u32 {
            if let Some(v) = rd32(esp + i * 4) {
                if (0x1090_0000..0x12E0_A000).contains(&v) && ret.len() < 16 { ret.push(format!("{v:08X}")); }
            }
        }
        log(&format!(
            "ACCESS VIOLATION at {eip:08X} ({} {:08X}) eax {:08X} ebx {:08X} ecx {:08X} edx {:08X} esi {:08X} edi {:08X} ebp {:08X} esp {esp:08X}
  stack: {}",
            if *rec.add(5) == 0 { "read" } else { "write" }, *rec.add(6),
            c(0xB0), c(0xA4), c(0xAC), c(0xA8), c(0xA0), c(0x9C), c(0xB4), ret.join(" ")));
        // which objects were involved: registers and the first stack slots that hold a game object
        let mut objs = Vec::new();
        let regs = [("eax", 0xB0), ("ebx", 0xA4), ("ecx", 0xAC), ("edx", 0xA8), ("esi", 0xA0), ("edi", 0x9C), ("ebp", 0xB4)];
        for (n, off) in regs { if let Some(d) = crash_obj(c(off)) { objs.push(format!("{n}={d}")); } }
        for i in 0..64u32 {
            if let Some(d) = rd32(esp + i * 4).and_then(crash_obj) { objs.push(format!("esp+{:X}={d}", i * 4)); }
        }
        if !objs.is_empty() { log(&format!("  objects: {}", objs.join(" | "))); }
    }
    0
}

// "Outer.Name cid" if v looks like a live UObject (vtable in TWFC.exe), for the crash logger: plain reads, no name cache
// (the handler may run inside fname on our own thread).
fn crash_obj(v: u32) -> Option<String> {
    if !(0x10000..0x8000_0000).contains(&v) || !(0x1090_0000..0x1300_0000).contains(&rd32(v)?) { return None; }
    let name = |o: u32| -> Option<String> {
        let id = rd32(o + 4)?;
        let b = rbuf(rd32(rd32(GNAMES)? + (id >> 14) * 4)? + 8, 128)?;
        let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        let s = String::from_utf16_lossy(&u);
        if s.is_empty() { return None; }
        Some(if id & 0x3FFF != 0 { format!("{s}_{}", (id & 0x3FFF) - 1) } else { s })
    };
    let me = name(v)?;
    let oi = (rd32(v + 0x24)? >> 4).wrapping_sub(1);
    let outer = if oi < 0x40_0000 { rd32(rd32(GOBJECTS)? + oi * 4).and_then(name) } else { None };
    let cid = (rd32(v + 0x14)? & 0xFFF).wrapping_sub(1);
    Some(format!("{}{me} cid={cid}", outer.map(|o| o + ".").unwrap_or_default()))
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

static mut EARLY_SEEN: Option<(u32, std::time::Instant)> = None; // (Omega ROBODEF, first seen)

// Chassis ObjectPath (+0x78 FString: data, count, max). The engine owns that buffer and reallocates/frees it when it re-reads the
// ini at chapter load, so it must always be a GMalloc block: pointing it at the DLL's static array (and restoring a saved pointer)
// corrupted the allocator, crash on the next chapter load (docs/TECHNICAL.md). Each set gets a fresh block; the previous one is
// leaked on purpose (freeing it from this thread could race a game-thread reader), ~128 bytes per level change.
const GMALLOC: u32 = 0x12B0_0204;
const PATH_CAP: u32 = 64;

fn engine_alloc(bytes: u32) -> u32 {
    let m = r(GMALLOC);
    if m == 0 { return 0; }
    // FMalloc vtable +0xC = Realloc(ptr, size, alignment); Realloc(0, n, 8) allocates (as the engine's TArray code does, 1091F196)
    let f: extern "thiscall" fn(u32, u32, u32, u32) -> u32 = unsafe { std::mem::transmute(r(r(m) + 0xC) as usize) };
    f(m, 0, bytes, 8)
}

fn fstring(a: u32) -> String {
    let (d, n) = (r(a), r(a + 4));
    if d == 0 || n == 0 || n > 512 { return String::new(); }
    rbuf(d, (n as usize - 1) * 2)
        .map(|b| String::from_utf16_lossy(&b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()))
        .unwrap_or_default()
}

fn chassis_is_omega(ch: u32) -> bool { fstring(ch + 0x78) == OMEGA_PATH_STR }

fn set_chassis_path(ch: u32, text: &str) -> bool {
    let u: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if u.len() as u32 > PATH_CAP { return false; }
    let buf = engine_alloc(PATH_CAP * 2);
    if buf == 0 { return false; }
    unsafe { std::ptr::copy_nonoverlapping(u.as_ptr(), buf as *mut u16, u.len()); }
    // pointer first: the old count is never larger than either buffer's 64 chars, so a reader in between stays in bounds
    w(ch + 0x78, buf); w(ch + 0x7C, u.len() as u32); w(ch + 0x80, PATH_CAP);
    true
}

fn point_chassis_at_omega(ch: u32) {
    if chassis_is_omega(ch) { return; }
    let cur = fstring(ch + 0x78);
    if !cur.is_empty() { unsafe { TANK_ORIG = Some(cur); } }
    set_chassis_path(ch, OMEGA_PATH_STR);
}

fn early_tank_flip(s: &Snap) {
    ship_weapons(s);
    let Some(ch) = s.find_cls(CID_CHASSIS, "Tank") else { return };
    if chassis_is_omega(ch) {
        // Omega unloaded (back at the menu): restore Megatron's path NOW. Choosing Megatron at chapter select looks the chassis up by
        // its path before the next hooked read; with Omega's path still in it the lookup found nothing and the game crashed on the
        // null chassis (10B0521F, obj +0x56C), seen 2026-10-16. The lazy restore in chassis_read ran inside that same call, too late.
        if s.find_cls(CID_ROBODEF, "OmegaSupreme_ROBODEF").is_none() {
            // the ini's value (TransCustomization.ini [Tank]) if the original was never seen
            let o = unsafe { TANK_ORIG.clone() }.unwrap_or_else(|| "TR_Megatron_ROBODEF_p.Megatron_ROBODEF".into());
            if set_chassis_path(ch, &o) { log("Omega unloaded: chassis Tank restored to Megatron"); }
            unsafe { OMEGA_LOADED = false; PREPARED = (0, 0); }
        }
        return;
    }
    let (Some(omega), Some(mega)) = (s.find_cls(CID_ROBODEF, "OmegaSupreme_ROBODEF"), s.find_cls(CID_ROBODEF, "Megatron_ROBODEF")) else { return };
    // wait for the jet donor (Balcony_DSN_m loads after Omega's packages): flipping before it gave the first pawn Megatron's tank
    // form, which is fixed at spawn (seen 2026-10-16). Maps without the donor (Escalation) flip after 8 s with the tank fallback.
    unsafe {
        let now = std::time::Instant::now();
        let seen = match EARLY_SEEN { Some((o, t)) if o == omega => t, _ => { EARLY_SEEN = Some((omega, now)); now } };
        if s.find_cls(CID_ROBODEF, "SoldierJetDec_ROBODEF").is_none() && now.duration_since(seen).as_secs() < 8 { return; }
    }
    // the first pawn can spawn from this path before the hooked chassis read (prepare_data), and its vehicle form (class, mesh) is
    // built from ROBODEF +0xA4 at spawn: put the ship in BEFORE the graft and the flip. Doing it after raced the game thread, which
    // built the pawn in between from the grafted MegatronVehicleForm (seen 2026-10-16: TnTankForm with Megatron's mesh, jet camera).
    ship_form(&s);
    // graft the player parts (health, hit reactions, melee set...) BEFORE the flip: the game may build Omega from this path at once,
    // and without them he spawns "dead" (seen 2026-10-16 in Iacon and Escalation). Same rule as chassis_read: only empty fields.
    for off in GRAFT {
        if r(omega + off) == 0 { w(omega + off, r(mega + off)); }
    }
    // jets have no transform momentum (+0xE8): ship_form cleared it, the graft just refilled it from Megatron
    if r(omega + 0xA4) != r(mega + 0xA4) { w(omega + 0xE8, 0); }
    point_chassis_at_omega(ch);
    log("early: chassis Tank path -> Omega (Omega's ROBODEF loaded)");
}

fn chassis_read(esi: u32) {
    let ch = esi.wrapping_sub(0x10);
    let slot = fname(r(ch + 4));
    if slot != "Tank" {
        return;
    }
    // already prepared for this Omega ROBODEF (same level): only re-point the path, no object scan
    unsafe {
        let (b, n) = PREPARED;
        if b != 0 && r(b + 4) == n && OMEGA_LOADED {
            point_chassis_at_omega(ch);
            return;
        }
    }
    let s = snapshot();
    let omega = s.find_cls(CID_ROBODEF, "OmegaSupreme_ROBODEF");
    let mega = s.find_cls(CID_ROBODEF, "Megatron_ROBODEF");
    unsafe {
        match (omega, mega) {
            (Some(b), Some(d)) => {
                point_chassis_at_omega(ch);
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
                for o in s.objs.iter().filter(|o| CID_PRIS.contains(&o.cls)) {
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
                if chassis_is_omega(ch) {
                    if let Some(o) = TANK_ORIG.clone() { set_chassis_path(ch, &o); }
                    log("Omega not loaded here: chassis Tank restored to Megatron");
                }
                OMEGA_LOADED = false;
                PREPARED = (0, 0);
            }
        }
    }
}

// TnLocationOffsetCameraBehavior fields set for Omega's giant orbit, those values, and the template's own values before that
const CAM_OFFS: [u32; 3] = [0x2C, 0x30, 0x34];
const CAM_GIANT: [f32; 3] = [2500.0, 1500.0, 6000.0];
static mut CAM_ORIG: Option<[f32; 3]> = None;

// Any other player pawn (the vanilla heroes) copies the giant template too: its camera strategy's live copies live in
// Transient (outer HmCameraStrategy_N) and are made again on form changes, so this runs every tick. Only exact giant values are
// touched, which no other camera uses (checked in CE 2026-10-17: the rest are 0/0/800).
fn normal_camera(s: &Snap) {
    let Some(orig) = (unsafe { CAM_ORIG }) else { return };
    for o in &s.objs {
        if s.name(o).starts_with("TnLocationOffsetCameraBehavior_") && CAM_OFFS.map(|k| rf(o.addr + k)) == CAM_GIANT
            && s.outer_name(o).starts_with("HmCameraStrategy_") {
            for (k, v) in CAM_OFFS.iter().zip(orig) { wf(o.addr + k, v); }
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
    // camera: the robot camera template (copied into each new pawn's strategy) -> giant orbit. Shared by every robot in the level;
    // normal_camera() puts the live copies of other pawns back to the values kept here.
    for o in &s.objs {
        if s.name(o).starts_with("TnLocationOffsetCameraBehavior_") && rf(o.addr + 0x34) == 800.0 && s.outer_name(o) == "OverTheShoulder_STRATEGY" {
            unsafe { CAM_ORIG = Some(CAM_OFFS.map(|k| rf(o.addr + k))); }
            for (k, v) in CAM_OFFS.iter().zip(CAM_GIANT) { wf(o.addr + k, v); }
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
    ship_form(s);
}

// Omega's own vehicle form (docs/TECHNICAL.md): his ship on a JET form. No Omega VEHDEF exists, so a player plane form gets the
// ship's look. Donor: the Decepticon seeker drone SoldierJetDec (TnPlaneFormBlueprint; shared with AI seekers, none in Iacon).
// VehicleForm fields: +0x28 SkeletalMesh, +0x2C PhysicsAsset, +0x30 AnimTree (kept: the jet's VEH_Plane_ANIMTREE, which the plane
// form drives), +0x34 AnimSets (1 entry, swapped in place: the engine owns the buffer). Omega ROBODEF: +0xA4 VehicleForm,
// +0xA8 camera set (the jet's Plane_Starscream_CAMSET), +0xE8 transform momentum (null for jets). Ship assets come from the DLC
// pack (D3B_IAC_Base_m mesh, D3B_IAC_GLB_DSN_m / Balcony_DSN_m physics; the D5 cinematic mesh as fallback).
// Without the jet donor (other maps) the ship goes on Megatron's hover tank instead.
fn ship_form(s: &Snap) {
    let mesh = s.find("VH_OmegaSupreme_SKEL").or_else(|| s.find("VH_VIGOmegaSupreme_SKEL"));
    let set = s.objs.iter().find(|o| fname(o.name) == "OmegaSupreme_VEH" && s.outer_name(o) == "TR_OmegaSupreme_ANM_p").map(|o| o.addr);
    let (Some(mesh), Some(set)) = (mesh, set) else { log("ship form: ship mesh or anim set not loaded"); return };
    let phys = s.find("VH_OmegaSupreme_PHYSSYS");
    let look = |vf: u32| {
        w(vf + 0x28, mesh);
        if let Some(p) = phys { w(vf + 0x2C, p); }
        let (data, n) = (r(vf + 0x34), r(vf + 0x38));
        if data != 0 && n >= 1 { w(data, set); }
    };
    let jet = s.find_cls(CID_ROBODEF, "SoldierJetDec_ROBODEF");
    let omega = s.find_cls(CID_ROBODEF, "OmegaSupreme_ROBODEF");
    if let (Some(jrd), Some(omega)) = (jet, omega) {
        let vf = r(jrd + 0xA4);
        look(vf);
        // gun sockets (TnVehicleFormBlueprint +0x88 WeaponSockets, FName array): the drone's WeaponSocket_Primary/2 don't exist on
        // the ship; fire from the fronts of the two engines (ship sockets L_Rocket1 / R_Rocket1). Entries swapped in place.
        if let (Some(l), Some(rr)) = (name_id("L_Rocket1"), name_id("R_Rocket1")) {
            let (d, k) = (r(vf + 0x88), r(vf + 0x8C));
            if d != 0 && k >= 2 { w(d, l); w(d + 4, rr); }
        }
        w(omega + 0xA4, vf);
        w(omega + 0xA8, r(jrd + 0xA8));
        w(omega + 0xE8, 0);
        ship_jet_tuning(s, set);
        log(&format!("ship form: jet {} with mesh {}, camera {}", fname(r(vf + 4)), fname(r(mesh + 4)), fname(r(r(jrd + 0xA8) + 4))));
        return;
    }
    // fallback: Megatron's hover tank with the ship's look and tree, camera pulled back (incl. OrbitDistancesByPCS +0x38)
    let Some(vf) = s.find("MegatronVehicleForm") else { return };
    look(vf);
    if let Some(t) = s.find("VEH_OmegaSupreme_ANIMTREE") { w(vf + 0x30, t); }
    for o in &s.objs {
        if s.name(o).starts_with("TnLocationOffsetCameraBehavior_") && s.outer_name(o) == "Tank_Megatron_STRATEGY" {
            wf(o.addr + 0x34, 25000.0); wf(o.addr + 0x2C, 0.0); wf(o.addr + 0x30, 7000.0);
            let (d, k) = (r(o.addr + 0x38), r(o.addr + 0x3C));
            for e in 0..k.min(16) { wf(d + e * 16 + 0xC, 25000.0 * 1.05); }
        }
    }
    log("ship form: no jet donor here, ship on Megatron's tank form");
}

// Omega's ship guns (docs/TECHNICAL.md). The vehicle weapons stay the jet CLASSES (cooked into every map, resolved at chapter load:
// ini [TankCannon] -> TnWeaponAssaultRiflePlane), Omega's ship weapons are only their DATA: weapon +0x244 PlayerData (AI reads +0x248
// AiData, so AI jets keep theirs). Second slot: Megatron_PCD_SP +0x128 VehicleWeapons (TArray of FStrings) swapped with Skywarp_PCD_SP's
// [PlaneAssaultRifle, HomingRocketVehicle]; only effective before a chapter loads (PCDs exist from the main menu). Headers are swapped, never
// aliased, so each engine buffer keeps one owner. Muzzles: the weapon meshes look their socket names (+0xE4, 4-byte FNames) up on the ship
// skeleton; the jet's WeaponSocket_Primary/2 are missing there and left projectiles at the world origin -> L_Rocket1 / R_Rocket1,
// on the mesh templates (weapon default +0x28C; new meshes copy them) and on live player meshes (weapon +0x290 list, +0x294 count).
const SHIP_GUNS: [(&str, &str); 2] = [
    ("TnWeaponAssaultRiflePlane", "OmegaMGSentry_WEPDATA"),       // slot 1: MG bursts
    ("TnWeaponHomingRocketVehicle", "OmegaRocketSentry_WEPDATA"), // slot 2: ship rockets
];

// The game's own values of what ship_weapons changes, kept before the first write: (class default, its FName, PlayerData,
// mesh template, muzzle socket FNames). Class defaults live for the whole session.
static mut SHIP_ORIG: Vec<(u32, u32, u32, u32, Vec<u32>)> = Vec::new();

// controller field holding its PlayerReplicationInfo (found once by scanning, see player_pri)
static PRI_OFF: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// The human player's own PlayerReplicationInfo: the live campaign/Escalation controller's PRI field. The AI squadmates (Iacon:
// Soundwave, Breakdown) have PRIs of the same class, so "the first PRI with a ROBODEF" was sometimes theirs (Omega's ship guns then
// got un-set mid-level and fired from the world origin; user 2026-10-17). The field offset is not in the cooked data: take the only
// pointer to a PRI-class object in the controller and cache its offset.
fn player_pri(s: &Snap) -> Option<u32> {
    let pc = s.objs.iter().filter(|o| CID_PCS.contains(&o.cls) && !s.name(o).starts_with("Default__")).last()?.addr;
    let is_pri = |p: u32| p > 0x10000 && rd32(p + 0x14).is_some_and(|c| CID_PRIS.contains(&(c & 0xFFF).wrapping_sub(1)));
    let off = PRI_OFF.load(std::sync::atomic::Ordering::Relaxed);
    if off != 0 { return Some(r(pc + off)).filter(|&p| is_pri(p)); }
    let hits: Vec<u32> = (0x100..0x800).step_by(4).filter(|&k| is_pri(r(pc + k))).collect();
    if hits.len() != 1 { return None; }
    PRI_OFF.store(hits[0], std::sync::atomic::Ordering::Relaxed);
    log(&format!("player PlayerReplicationInfo at controller +0x{:X}", hits[0]));
    Some(r(pc + hits[0]))
}

// ROBODEF name the player's PlayerReplicationInfo will spawn (+0x420), None outside a campaign/Escalation level
fn player_robodef(s: &Snap) -> Option<String> {
    player_pri(s).map(|p| r(p + 0x420)).filter(|&b| b != 0).map(|b| fname(r(b + 4)))
}

fn ship_weapons(s: &Snap) {
    if let (Some(m), Some(k)) = (s.find("Megatron_PCD_SP"), s.find("Skywarp_PCD_SP")) {
        if r(m + 0x12C) == 1 && r(k + 0x12C) == 2 {
            for off in [0x128, 0x12C, 0x130] { let (a, b) = (r(m + off), r(k + off)); w(m + off, b); w(k + off, a); }
            log("ship guns: Megatron gets the jets' two vehicle weapons (PCD swap with Skywarp)");
        }
    }
    // The ship setup below is shared class data, so it is only right while the player is Omega: the other jets fire these same two
    // classes from their own WeaponSocket_Primary.
    let omega = match player_robodef(s).as_deref() {
        Some("OmegaSupreme_ROBODEF") | Some("Megatron_ROBODEF") => true,
        Some(_) => false,
        None => return,
    };
    let (Some(l), Some(rr)) = (name_id_cached("L_Rocket1"), name_id_cached("R_Rocket1")) else { return };
    let sockets = |mesh: u32| {
        let (d, n) = (r(mesh + 0xE4), r(mesh + 0xE8));
        if d != 0 && (1..=8).contains(&n) {
            for j in 0..n { let v = if j % 2 == 0 { l } else { rr }; if r(d + j * 4) != v { w(d + j * 4, v); } }
        }
    };
    // Class defaults and their mesh templates only: a weapon takes PlayerData and socket names when it is created (live-only writes
    // left the jet MG, user test 2026-10-16), so the first pawn keeps the jet's until a respawn (checkpoint restart). Never write live
    // weapons from this 0.5 s tick: at level teardown their mesh arrays are freed while the objects are still listed, and the writes
    // corrupted the allocator (crash on the next chapter load, docs/TECHNICAL.md). Write only on change to keep even these rare.
    for (cls, data) in SHIP_GUNS {
        let (Some(def), Some(data)) = (s.find(&format!("Default__{cls}")), s.find(data)) else { continue };
        let tmpl = r(def + 0x28C);
        let orig = unsafe { &mut SHIP_ORIG };
        let i = match orig.iter().position(|o| o.0 == def && o.1 == r(def + 4)) {
            Some(i) => i,
            None => {
                let socks = if tmpl != 0 && (1..=8).contains(&r(tmpl + 0xE8)) { (0..r(tmpl + 0xE8)).map(|j| r(r(tmpl + 0xE4) + j * 4)).collect() } else { Vec::new() };
                orig.push((def, r(def + 4), r(def + 0x244), tmpl, socks));
                orig.len() - 1
            }
        };
        if omega {
            if r(def + 0x244) != data { w(def + 0x244, data); }
            if tmpl != 0 { sockets(tmpl); }
        } else {
            let (_, _, pd, t, socks) = &orig[i];
            if r(def + 0x244) != *pd { w(def + 0x244, *pd); }
            if *t == tmpl && tmpl != 0 && r(tmpl + 0xE8) as usize == socks.len() {
                let d = r(tmpl + 0xE4);
                for (j, &v) in socks.iter().enumerate() { if r(d + j as u32 * 4) != v { w(d + j as u32 * 4, v); } }
            }
        }
    }
}

// name_id scans the whole name table: cache the ids (names never move)
fn name_id_cached(want: &'static str) -> Option<u32> {
    thread_local! { static C: std::cell::RefCell<HashMap<&'static str, u32>> = std::cell::RefCell::new(HashMap::new()); }
    if let Some(v) = C.with(|c| c.borrow().get(want).copied()) { return Some(v); }
    let v = name_id(want)?;
    C.with(|c| c.borrow_mut().insert(want, v));
    Some(v)
}

// Barrage (B): the live jet-tree node playing Rocket_TellHoverLoop_VEH (ascend node, SHIP_ANIM_MAP). AnimNodeSequence +0xAC AnimSeqName,
// +0xB4 flags (bNoNotifies 0x100), +0xB8 CurrentTime, +0xC0 AnimSeq (0 on the template). Notifies stay blocked; B plays one cycle.
const BARRAGE_CYCLE: f32 = 2.7; // the last SpawnProjectile notify is at 2.5 s

fn barrage_node(s: &Snap) -> u32 {
    let Some(id) = name_id_cached("Rocket_TellHoverLoop_VEH") else { return 0 };
    s.objs.iter()
        .find(|o| r(o.addr + 0xAC) == id && r(o.addr + 0xC0) != 0 && fname(o.name).starts_with("AnimNodeSequence"))
        .map(|o| o.addr).unwrap_or(0)
}

fn barrage_block(node: u32, on: bool) {
    let f = r(node + 0xB4);
    w(node + 0xB4, if on { f | 0x100 } else { f & !0x100 });
}

// Jet node (VEH_Plane_ANIMTREE template, AnimNodeSequence +0xAC AnimSeqName) -> ship sequence, from the user's preview review
// (docs/TECHNICAL.md; preview numbers in brackets). One ship anim can serve several nodes.
const SHIP_ANIM_MAP: [(&str, &str); 11] = [
    ("Nav_Hover_Pose_VEH", "Nav_Hover_IdleShort_VEH"),   // [23] hover idle
    ("Nav_Hover_F_VEH", "Nav_Hover_IdleShort_VEH"),      // W/S: steady engines
    ("Nav_Hover_B_VEH", "Nav_Hover_IdleShort_VEH"),
    ("Nav_Hover_L_VEH", "Nav_Boost_L_VEH"),              // [15] steer left (A)
    ("Nav_Hover_R_VEH", "Nav_Boost_R_VEH"),              // [19]
    ("Nav_Boost_VEH", "Nav_Boost_F_VEH"),                // [13] boost (hold right mouse)
    ("Nav_Boost_L_VEH", "Nav_Boost_F_VEH"),              // boost turns keep the boost pose (turn poses fold the engines)
    ("Nav_Boost_R_VEH", "Nav_Boost_F_VEH"),
    ("Nav_Hover_to_Boost_VEH", "Nav_Boost_F_VEH"),       // straight into the boost
    ("Nav_Boost_to_Hover_VEH", "Deploy_Hover_VEH"),      // [5] leaving boost
    // ascend node = hidden barrage launcher: it ticks even at zero blend weight, so its anim's own SpawnProjectile
    // notifies fire the real homing barrage; ship_barrage() keeps them blocked until B. Costs the ascend (C) anim.
    ("Nav_Hover_Ascent_VEH", "Rocket_TellHoverLoop_VEH"),
];
static mut ANIM_MAPPED: (u32, u32) = (0, 0); // (jet tree template, its FName) already mapped: a second pass would remap
                                            // nodes whose new name is also a jet name (Nav_Boost_L_VEH)

fn ship_jet_tuning(s: &Snap, set: u32) {
    let Some(si) = s.index_of(set) else { return };
    // ship sequences of the set: name -> (object, FName id)
    let seqs: HashMap<String, (u32, u32)> = s.objs.iter().filter(|o| o.outer == si)
        .map(|o| { let id = r(o.addr + 0x28); (fname(id), (o.addr, id)) }).collect();
    // anim map on the jet tree template (once per loaded template)
    if let Some(tree) = s.find("VEH_Plane_ANIMTREE") {
        let done = unsafe { ANIM_MAPPED.0 == tree && ANIM_MAPPED.1 == r(tree + 4) };
        if let (false, Some(ti)) = (done, s.index_of(tree)) {
            let mut n = 0;
            for o in s.objs.iter().filter(|o| o.outer == ti && fname(o.name).contains("AnimNodeSequence")) {
                let cur = fname(r(o.addr + 0xAC));
                if let Some((_, ship)) = SHIP_ANIM_MAP.iter().find(|(jet, _)| *jet == cur) {
                    if let Some(&(_, id)) = seqs.get(*ship) { w(o.addr + 0xAC, id); n += 1; }
                }
            }
            unsafe { ANIM_MAPPED = (tree, r(tree + 4)); }
            log(&format!("ship anims: {n} jet tree nodes mapped"));
        }
    }
    // speeds (AnimSequence +0x40 RateScale): leaving boost x2, boost engines unfold sooner x1.5
    if let Some(&(q, _)) = seqs.get("Deploy_Hover_VEH") { wf(q + 0x40, 2.0); }
    if let Some(&(q, _)) = seqs.get("Nav_Boost_F_VEH") { wf(q + 0x40, 1.5); }
    // Nav_Boost_L_VEH's StopEffect (notify +0x30 Template = OmegaShip_hover_FX) switched the engines off when steering: point it
    // at Trails_Jet_D_FX (never spawned on the ship). Notifies: AnimSequence +0x2C TArray of 12-byte {time, notify, comment}.
    if let (Some(&(q, _)), Some(hover), Some(dummy)) = (seqs.get("Nav_Boost_L_VEH"), s.find("OmegaShip_hover_FX"), s.find("Trails_Jet_D_FX")) {
        let (d, n) = (r(q + 0x2C), r(q + 0x30));
        for j in 0..n.min(64) {
            let no = r(d + j * 12 + 4);
            if no != 0 && fname(r(no + 4)).starts_with("HmAnimNotify_StopEffect") && r(no + 0x30) == hover { w(no + 0x30, dummy); }
        }
    }
    // bank at full steer: Plane_Physics (TnPlaneSimulationBlueprint) +0x74 RollDueToYawValue 77 -> 20 (the giant ship's poses
    // break up when banked hard)
    if let Some(pp) = s.find("Plane_Physics") { wf(pp + 0x74, 20.0); }
    // camera: Starscream's jet strategies (HmLocationOffsetCameraBehavior +0x2C back, +0x34 up)
    for o in &s.objs {
        let on = s.outer_name(o);
        if (on == "FlyingPlane_Starscream_STRATEGY" || on == "HoverPlane_Starscream_STRATEGY") && s.name(o).starts_with("HmLocationOffsetCameraBehavior_") {
            wf(o.addr + 0x2C, -25000.0); wf(o.addr + 0x34, 7000.0);
        }
    }
}

// ------------------------------------------------------------------------------------------------ sky arena

const ARENA_SCALE: f32 = 8.0;
const ARENA_HEIGHT: f32 = 20000.0;
const UCLASS_VTABLE: u32 = 0x1290_39C8;

struct Arena {
    plat: u32,        // spawned floor actor
    plat_name: u32,
    center: (f32, f32, f32),
    calls: u32,       // ARENA[3] when the spawn was requested
    t_req: f32,
    tp_left: u32,     // teleport writes still to do (the game overrides the first ones)
    ground: Option<(f32, f32, f32)>, // the mission's own spawn point (pawn location when it spawned, before the arena lift)
    on_ground: bool,  // F12 toggled to the mission spawn: no arena lift, no fall-off return
}

impl Arena {
    fn target(&self) -> (f32, f32, f32) { if self.on_ground { self.ground.unwrap_or(self.center) } else { self.center } }
}

// Arms the F12 UFunction and the factory. Returns false if this map has no Floor_8192_STAT (not Iacon).
fn arena_prepare(s: &Snap, pawn: u32) -> bool {
    let mut ufn = 0;
    let (mut fac, mut cls, mut mesh) = (0, 0, 0);
    for o in &s.objs {
        match s.name(o).as_str() {
            "ToggleDebugCamera" if s.outer_name(o) == "HmCheatManager" => ufn = o.addr,
            "Default__ActorFactoryDynamicSM" => fac = o.addr,
            "DynamicSMActor_Spawnable" if r(o.addr) == UCLASS_VTABLE => cls = o.addr,
            "Floor_8192_STAT" => mesh = o.addr,
            _ => {}
        }
    }
    if ufn == 0 || fac == 0 || cls == 0 || mesh == 0 { return false; }
    let cave = arena_cave as usize as u32;
    if r(ufn + 0x68) != cave { w(ufn + 0x68, cave); }
    // InterpActor is bNoDelete (refused at runtime); DynamicSMActor_Spawnable spawns. The default factory swaps NewActorClass
    // for GameplayActorClass, so set both. CollisionPreset 2 = BlockAll.
    // the mesh is Iacon's: arena_factory_restore() puts the original back once the cave ran (the default factory outlives the level)
    unsafe { if r(fac + 0x48) != mesh { FAC_MESH_ORIG = (fac, r(fac + 0x48)); } }
    w(fac + 0x28, cls); w(fac + 0x3C, cls); w(fac + 0x48, mesh);
    wf(fac + 0x4C, ARENA_SCALE); wf(fac + 0x50, ARENA_SCALE); wf(fac + 0x54, 1.0);
    wb(fac + 0x5C, 2);
    unsafe {
        ARENA[0] = fac;
        ARENA[4] = rf(pawn + 0xD8).to_bits();
        ARENA[5] = rf(pawn + 0xDC).to_bits();
        ARENA[6] = (rf(pawn + 0xE0) + ARENA_HEIGHT).to_bits();
        for k in 7..10 { ARENA[k] = 0; }
    }
    true
}

static mut FAC_MESH_ORIG: (u32, u32) = (0, 0); // (Default__ActorFactoryDynamicSM, its StaticMesh before arena_prepare)

fn arena_factory_restore() {
    unsafe {
        let (fac, mesh) = FAC_MESH_ORIG;
        if fac != 0 { w(fac + 0x48, mesh); FAC_MESH_ORIG = (0, 0); }
    }
}

// After the cave spawned it: keep Omega's physics bodies from flinging it away, and compute the landing spot.
fn arena_spawned(a: &mut Arena, plat: u32) {
    let comp = r(plat + 0x1CC);
    w(comp + 0xF0, r(comp + 0xF0) & !0x0800_0000);          // component BlockRigidBody off
    w(plat + 0x80, r(plat + 0x80) & !0x2000);                // actor BlockRigidBody off
    w(plat + 0x78, (r(plat + 0x78) & !0x8000) | 0x800);      // bPushedByEncroachers off, bWorldGeometry on
    a.plat = plat;
    a.plat_name = r(plat + 4);
    // the mesh pivot is a corner (4608 x 4608 tile), top at the pivot's Z
    a.center = (rf(plat + 0xD8) - 2304.0 * ARENA_SCALE, rf(plat + 0xDC) + 2304.0 * ARENA_SCALE, rf(plat + 0xE0) + 2600.0);
    a.tp_left = 3;
    log(&format!("sky arena: floor {} at {:.0} {:.0} {:.0}", fname(a.plat_name), a.center.0, a.center.1, a.center.2));
}

fn arena_alive(a: &Arena) -> bool { a.plat != 0 && r(a.plat + 4) == a.plat_name }

// Teleport works only in robot form (in vehicle form the physics body owns the position).
fn robot_form(pawn: u32) -> bool { let c = r(pawn + 0x15C); c != 0 && c == r(pawn + 0x3F8) }

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
// Only in robot form: +0x15C is the active CollisionComponent, which is the pawn's CylinderComponent (+0x3F8) in robot
// form but the vehicle's mesh component in vehicle form (writing +0x1B0 there corrupted it; crash with the ship form).
fn keep_collision(pawn: u32) {
    let cyl = r(pawn + 0x15C);
    if cyl != 0 && cyl == r(pawn + 0x3F8) && rf(cyl + 0x1B0) != 800.0 {
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
    let mut barrage = (0u32, 0u32, 0.0f32, 0.0f32); // (pawn, live launcher node, last search, notifies open until)
    let mut b_was_down = false;
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

    let mut t_tank = 0.0f32;
    // sky arena: requested on every new pawn in Iacon (spawned once, reused while it exists)
    let mut arena = Arena { plat: 0, plat_name: 0, center: (0.0, 0.0, 0.0), calls: 0, t_req: -10.0, tp_left: 0, ground: None, on_ground: false };
    let mut arena_want = false;
    let mut f12_was_down = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let tn = now();
        // Early Tank flip: Escalation's PlayerReplicationInfo resolves "Megatron" -> Tank chassis path through a reader the
        // chassis cave does not see, before the first hooked read. Point the path at Omega as soon as Omega's ROBODEF is
        // loaded; the hooked read then grafts and prepares as usual.
        if tn - t_tank > 0.5 {
            t_tank = tn;
            let s = snapshot();
            early_tank_flip(&s);
            // the player is someone else than Omega (a vanilla hero): undo the giant camera on their live copies
            let pawn = if pc != 0 && r(pc + 4) == pc_name { r(pc + 0x1C8) } else { 0 };
            // no campaign/Escalation controller at all = nobody can be Omega; while one exists but
            // its pawn is not known yet (level start), leave the camera alone
            let campaign_pc = s.objs.iter().any(|o| CID_PCS.contains(&o.cls) && !s.name(o).starts_with("Default__"));
            if (pawn != 0 && fname(r(r(pawn + 0x628) + 4)) != "OmegaSupreme_ROBODEF") || (pawn == 0 && !campaign_pc) { normal_camera(&s); }
        }
        // player controller (re-validate; a level change frees everything)
        if pc == 0 || r(pc + 4) != pc_name || !CID_PCS.contains(&(r(pc + 0x14) & 0xFFF).wrapping_sub(1)) {
            if tn - t_pc < 1.0 { continue; }
            t_pc = tn;
            let s = snapshot();
            // the live controller, not the class default object (Default__TnPlayerControllerCampaign)
            pc = s.objs.iter().filter(|o| CID_PCS.contains(&o.cls) && !s.name(o).starts_with("Default__")).last().map(|o| o.addr).unwrap_or(0);
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
            // the game just placed the new pawn at the mission's spawn (level start or checkpoint): remember it for F12
            let at = (rf(pawn + 0xD8), rf(pawn + 0xDC), rf(pawn + 0xE0));
            if !arena_alive(&arena) || at.2 < arena.center.2 - 4000.0 { arena.ground = Some(at); }
            if arena_alive(&arena) { if !arena.on_ground { arena.tp_left = 3; } } else { arena.on_ground = false; arena_want = true; }
        }
        keep_collision(pawn);

        // sky arena: spawn through F12 (game thread) once the pawn has settled, then land on its centre
        let calls = unsafe { ARENA[3] };
        if calls != arena.calls {
            // the cave ran (it clears `pending` itself)
            arena.calls = calls;
            arena_factory_restore();
            let plat = unsafe { ARENA[2] };
            if plat != 0 { arena_spawned(&mut arena, plat); } else { log("sky arena: spawn failed"); }
        } else if unsafe { ARENA[1] } != 0 {
            if tn - arena.t_req > 2.0 {
                unsafe { ARENA[1] = 0; } // the press was not seen (menu, window in back): retry
                arena_factory_restore();
                arena_want = true;
            }
        } else if arena_want && tn - t_spawn > 1.5 && robot_form(pawn) && game_in_front() {
            arena_want = false;
            if arena_prepare(&snapshot(), pawn) {
                arena.calls = calls;
                arena.t_req = tn;
                unsafe { ARENA[1] = 1; }
                press(0x7B); // F12
            }
        }
        // F12 (the player's press; the arena spawn's own press is ignored for 1 s): sky arena <-> the mission's spawn point.
        // The game also runs F12's native (arena_cave), which does nothing without a pending spawn. Robot form only (teleport).
        let f12_down = unsafe { GetAsyncKeyState(0x7B) } as u16 & 0x8000 != 0;
        if f12_down && !f12_was_down && game_in_front() && tn - arena.t_req > 1.0 && unsafe { ARENA[1] } == 0
            && arena_alive(&arena) && arena.ground.is_some() && robot_form(pawn) {
            arena.on_ground = !arena.on_ground;
            arena.tp_left = 3;
            log(if arena.on_ground { "F12: to the mission spawn" } else { "F12: back to the sky arena" });
        }
        f12_was_down = f12_down;
        if arena_alive(&arena) && robot_form(pawn) {
            if !arena.on_ground && rf(pawn + 0xE0) < arena.center.2 - 4000.0 && arena.tp_left == 0 { arena.tp_left = 3; } // fell off
            if arena.tp_left > 0 {
                arena.tp_left -= 1;
                let (x, y, mut z) = arena.target();
                // the mission's spawn point is placed for a normal-size pawn: lift Omega by his collision half-height
                // (+0x1B4, 2400) so he stands on the floor instead of half below it, then he drops the last bit (user 2026-10-17)
                if arena.on_ground { z += rf(r(pawn + 0x15C) + 0x1B4); }
                wf(pawn + 0xD8, x); wf(pawn + 0xDC, y); wf(pawn + 0xE0, z);
                wf(pawn + 0xF0, 0.0); wf(pawn + 0xF4, 0.0); wf(pawn + 0xF8, 0.0);
            }
        }

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
        hud::update(hud::View { mode: st().mode, grab: ph.hud(), god, ground: arena.on_ground }, &MODE_TEXT);

        // T: melee mode cycle (only while the game window is in front)
        let t_down = unsafe { GetAsyncKeyState(0x54) } as u16 & 0x8000 != 0;
        if t_down && !t_was_down && game_in_front() && unsafe { OMEGA_GRAB[2] } == 0 {
            let m = { let mut g = st(); g.mode = (g.mode + 1) % MODES.len(); g.mode };
            set_melee(4, MODES[m].1);
            missile_sockets(MODES[m].0.starts_with("missile"));
        }
        t_was_down = t_down;

        // B: Omega's ship rocket barrage (one cycle of the hidden launcher node; re-found per pawn, a respawn rebuilds the tree)
        if barrage.0 != pawn || (barrage.1 == 0 && tn - barrage.2 > 1.0) {
            let node = if barrage.0 != pawn || barrage.1 == 0 { barrage_node(&snapshot()) } else { barrage.1 };
            if node != 0 { barrage_block(node, true); }
            barrage = (pawn, node, tn, 0.0);
        }
        let b_down = unsafe { GetAsyncKeyState(0x42) } as u16 & 0x8000 != 0;
        if barrage.1 != 0 && r(barrage.1 + 0xAC) == name_id_cached("Rocket_TellHoverLoop_VEH").unwrap_or(0) {
            if b_down && !b_was_down && game_in_front() && tn > barrage.3 {
                wf(barrage.1 + 0xB8, 0.0);
                barrage_block(barrage.1, false);
                barrage.3 = tn + BARRAGE_CYCLE;
            } else if barrage.3 > 0.0 && tn > barrage.3 {
                barrage_block(barrage.1, true);
                barrage.3 = 0.0;
            }
        } else if barrage.1 != 0 {
            barrage.1 = 0; // node freed (respawn / level change): find it again
        }
        b_was_down = b_down;

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
