// On-screen guide panel (right side of the screen) drawn inside the game's own Direct3D 9 frame.
//
// Hook chain: the exe's IAT entry for d3d9!Direct3DCreate9 -> our wrapper patches IDirect3D9::CreateDevice
// -> on the game's device we patch IDirect3DDevice9::Present. Right before each Present we draw one textured
// quad on the back buffer. Drawing goes through the pipeline (not GetDC), so it works with multisampling.
//
// The panel image is rendered with GDI on the mod thread whenever its content changes (mode, grab phase,
// screen size) and handed to the render thread as premultiplied ARGB pixels.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

type HANDLE = *mut c_void;
type P = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn VirtualProtect(addr: *mut c_void, size: usize, newp: u32, old: *mut u32) -> i32;
    fn GetModuleHandleW(name: *const u16) -> HANDLE;
    fn GetProcAddress(h: HANDLE, name: *const u8) -> *mut c_void;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(dc: HANDLE) -> HANDLE;
    fn CreateDIBSection(dc: HANDLE, bmi: *const u32, usage: u32, bits: *mut *mut u32, sec: HANDLE, off: u32) -> HANDLE;
    fn SelectObject(dc: HANDLE, o: HANDLE) -> HANDLE;
    fn DeleteObject(o: HANDLE) -> i32;
    fn DeleteDC(dc: HANDLE) -> i32;
    fn CreateFontW(h: i32, w: i32, esc: i32, ori: i32, weight: i32, it: u32, ul: u32, so: u32, cs: u32, op: u32, cp: u32, q: u32, pf: u32, face: *const u16) -> HANDLE;
    fn SetTextColor(dc: HANDLE, c: u32) -> u32;
    fn SetBkMode(dc: HANDLE, m: i32) -> i32;
    fn TextOutW(dc: HANDLE, x: i32, y: i32, s: *const u16, n: i32) -> i32;
    fn CreateSolidBrush(c: u32) -> HANDLE;
    fn GdiFlush() -> i32;
}
#[link(name = "user32")]
extern "system" {
    fn FillRect(dc: HANDLE, rc: *const [i32; 4], br: HANDLE) -> i32;
}

// ------------------------------------------------------------------------------------------------ shared state

/// Set by the mod loop: Omega is the player's pawn (panel shown unless hidden with Home).
pub static SHOW: AtomicBool = AtomicBool::new(false);
/// Back-buffer height seen by the render thread (the panel scales with it).
static SCREEN_H: AtomicU32 = AtomicU32::new(0);

struct Image { w: u32, h: u32, px: Vec<u32>, ver: u32 }
static IMAGE: Mutex<Option<Image>> = Mutex::new(None);

// ------------------------------------------------------------------------------------------------ hooks

static REAL_CREATE9: AtomicUsize = AtomicUsize::new(0);
static REAL_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static REAL_PRESENT: AtomicUsize = AtomicUsize::new(0);

const IAT_DIRECT3DCREATE9: u32 = 0x1194_88DC; // TWFC.exe import slot d3d9!Direct3DCreate9

unsafe fn vt(obj: P, i: usize) -> usize { *(*(obj as *const *const usize)).add(i) }

/// Writes `hook` into a vtable/IAT slot and returns the previous value.
unsafe fn swap_slot(slot: *mut usize, hook: usize) -> usize {
    let mut old = 0u32;
    VirtualProtect(slot as P, 4, 0x04, &mut old);
    let prev = *slot;
    *slot = hook;
    VirtualProtect(slot as P, 4, old, &mut old);
    prev
}

/// Called from DllMain, before the game creates its device: redirect the exe's Direct3DCreate9 import.
pub unsafe fn install() {
    let d3d9: Vec<u16> = "d3d9.dll\0".encode_utf16().collect();
    let real = GetProcAddress(GetModuleHandleW(d3d9.as_ptr()), b"Direct3DCreate9\0".as_ptr()) as usize;
    let slot = IAT_DIRECT3DCREATE9 as *mut usize;
    if real == 0 || *slot != real {
        crate::log(&format!("hud: Direct3DCreate9 import not found (slot {:08X}, real {real:08X}); no overlay", *slot));
        return;
    }
    REAL_CREATE9.store(real, Relaxed);
    swap_slot(slot, hook_create9 as usize);
}

unsafe extern "system" fn hook_create9(sdk: u32) -> P {
    let f: unsafe extern "system" fn(u32) -> P = std::mem::transmute(REAL_CREATE9.load(Relaxed));
    let d3d = f(sdk);
    if !d3d.is_null() {
        let slot = (*(d3d as *const *mut usize)).add(16); // IDirect3D9::CreateDevice
        if *slot != hook_create_device as usize {
            REAL_CREATE_DEVICE.store(swap_slot(slot, hook_create_device as usize), Relaxed);
        }
    }
    d3d
}

unsafe extern "system" fn hook_create_device(d3d: P, adapter: u32, ty: u32, wnd: HANDLE, flags: u32, pp: P, out: *mut P) -> i32 {
    let f: unsafe extern "system" fn(P, u32, u32, HANDLE, u32, P, *mut P) -> i32 = std::mem::transmute(REAL_CREATE_DEVICE.load(Relaxed));
    let hr = f(d3d, adapter, ty, wnd, flags, pp, out);
    if hr >= 0 && !out.is_null() && !(*out).is_null() {
        let slot = (*(*out as *const *mut usize)).add(17); // IDirect3DDevice9::Present
        if *slot != hook_present as usize {
            REAL_PRESENT.store(swap_slot(slot, hook_present as usize), Relaxed);
        }
        crate::log("hud: game Direct3D device hooked");
    }
    hr
}

unsafe extern "system" fn hook_present(dev: P, src: P, dst: P, wnd: HANDLE, dirty: P) -> i32 {
    if SHOW.load(Relaxed) {
        // never let a panic unwind into the renderer
        let _ = std::panic::catch_unwind(|| draw(dev));
    }
    let f: unsafe extern "system" fn(P, P, P, HANDLE, P) -> i32 = std::mem::transmute(REAL_PRESENT.load(Relaxed));
    f(dev, src, dst, wnd, dirty)
}

// ------------------------------------------------------------------------------------------------ drawing (render thread)

struct Tex { dev: usize, tex: usize, w: u32, h: u32, ver: u32 }
static TEX: Mutex<Option<Tex>> = Mutex::new(None);
static DRAWN: AtomicBool = AtomicBool::new(false);

macro_rules! call {
    ($obj:expr, $i:expr, fn($($t:ty),*) $(, $a:expr)*) => {{
        let f: unsafe extern "system" fn(P $(, $t)*) -> i32 = std::mem::transmute(vt($obj, $i));
        f($obj $(, $a)*)
    }};
}

unsafe fn release(o: P) { if !o.is_null() { call!(o, 2, fn()); } }

unsafe fn draw(dev: P) {
    if call!(dev, 3, fn()) != 0 { return; } // TestCooperativeLevel: device lost
    let mut bb: P = std::ptr::null_mut();
    if call!(dev, 18, fn(u32, u32, u32, *mut P), 0, 0, 0, &mut bb) < 0 || bb.is_null() { return; }
    let mut desc = [0u32; 8];
    call!(bb, 12, fn(*mut u32), desc.as_mut_ptr()); // IDirect3DSurface9::GetDesc
    let (sw, sh) = (desc[6], desc[7]);
    SCREEN_H.store(sh, Relaxed);

    let Ok(mut tg) = TEX.try_lock() else { release(bb); return };
    if let Some(img) = IMAGE.try_lock().ok().as_deref().and_then(|o| o.as_ref()) {
        let stale = match tg.as_ref() { Some(t) => t.dev != dev as usize || t.ver != img.ver, None => true };
        if stale { upload(dev, &mut tg, img); }
    }
    let Some(t) = tg.as_ref().filter(|t| t.dev == dev as usize && t.tex != 0) else { release(bb); return };
    let (tex, tw, th) = (t.tex as P, t.w as f32, t.h as f32);
    drop(tg);

    let mut sb: P = std::ptr::null_mut();
    if call!(dev, 59, fn(u32, *mut P), 1, &mut sb) < 0 || sb.is_null() { release(bb); return; } // CreateStateBlock(ALL)
    let (mut old_rt, mut old_ds): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
    call!(dev, 38, fn(u32, *mut P), 0, &mut old_rt);
    call!(dev, 40, fn(*mut P), &mut old_ds);
    call!(dev, 37, fn(u32, P), 0, bb);
    call!(dev, 39, fn(P), std::ptr::null_mut::<c_void>());

    if call!(dev, 41, fn()) >= 0 {
        for (rs, v) in [(7u32, 0u32), (14, 0), (15, 0), (22, 1), (8, 3), (27, 1), (19, 2), (20, 6), (171, 1), (206, 0),
                        (28, 0), (52, 0), (137, 0), (174, 0), (168, 0xF), (194, 0)] {
            call!(dev, 57, fn(u32, u32), rs, v);
        }
        for (st, v) in [(1u32, 2u32), (2, 2), (4, 2), (5, 2), (11, 0), (24, 0)] { call!(dev, 67, fn(u32, u32, u32), 0, st, v); }
        call!(dev, 67, fn(u32, u32, u32), 1, 1, 1);
        call!(dev, 67, fn(u32, u32, u32), 1, 4, 1);
        for (ss, v) in [(1u32, 3u32), (2, 3), (5, 1), (6, 1), (7, 0), (11, 0)] { call!(dev, 69, fn(u32, u32, u32), 0, ss, v); }
        call!(dev, 92, fn(P), std::ptr::null_mut::<c_void>()); // SetVertexShader
        call!(dev, 107, fn(P), std::ptr::null_mut::<c_void>()); // SetPixelShader
        call!(dev, 102, fn(u32, u32), 0, 1); // SetStreamSourceFreq
        call!(dev, 65, fn(u32, P), 0, tex);
        call!(dev, 89, fn(u32), 0x104); // FVF XYZRHW | TEX1

        let x0 = sw as f32 - tw - (sh as f32 * 0.03).round() - 0.5;
        let y0 = ((sh as f32 - th) * 0.5).round() - 0.5;
        let (x1, y1) = (x0 + tw, y0 + th);
        let v: [f32; 24] = [x0, y0, 0.0, 1.0, 0.0, 0.0, x1, y0, 0.0, 1.0, 1.0, 0.0,
                            x0, y1, 0.0, 1.0, 0.0, 1.0, x1, y1, 0.0, 1.0, 1.0, 1.0];
        let hr = call!(dev, 83, fn(u32, u32, *const f32, u32), 5, 2, v.as_ptr(), 24); // DrawPrimitiveUP(TRIANGLESTRIP)
        if !DRAWN.swap(true, Relaxed) { crate::log(&format!("hud: first panel draw {sw}x{sh}, hr {hr:08X}")); }
        call!(dev, 65, fn(u32, P), 0, std::ptr::null_mut::<c_void>());
        call!(dev, 42, fn());
    }

    call!(dev, 37, fn(u32, P), 0, old_rt);
    call!(dev, 39, fn(P), old_ds);
    release(old_rt);
    release(old_ds);
    call!(sb, 5, fn()); // Apply
    release(sb);
    release(bb);
}

unsafe fn upload(dev: P, tg: &mut Option<Tex>, img: &Image) {
    if let Some(old) = tg.take() {
        if old.dev == dev as usize && old.w == img.w && old.h == img.h {
            *tg = Some(Tex { ver: img.ver, ..old });
        } else {
            release(old.tex as P);
        }
    }
    if tg.is_none() {
        let mut tex: P = std::ptr::null_mut();
        // CreateTexture(w, h, 1 level, usage 0, A8R8G8B8, MANAGED): survives device Reset (alt-tab)
        if call!(dev, 23, fn(u32, u32, u32, u32, u32, u32, *mut P, P), img.w, img.h, 1, 0, 21, 1, &mut tex, std::ptr::null_mut::<c_void>()) < 0 {
            return;
        }
        *tg = Some(Tex { dev: dev as usize, tex: tex as usize, w: img.w, h: img.h, ver: img.ver });
    }
    let tex = tg.as_ref().unwrap().tex as P;
    let mut lr: [usize; 2] = [0; 2]; // D3DLOCKED_RECT { Pitch, pBits }
    if call!(tex, 19, fn(u32, *mut usize, P, u32), 0, lr.as_mut_ptr(), std::ptr::null_mut::<c_void>(), 0) < 0 { return; }
    for y in 0..img.h as usize {
        let row = &img.px[y * img.w as usize..(y + 1) * img.w as usize];
        std::ptr::copy_nonoverlapping(row.as_ptr(), (lr[1] + y * lr[0]) as *mut u32, row.len());
    }
    call!(tex, 20, fn(u32), 0);
}

// ------------------------------------------------------------------------------------------------ panel image (mod thread)

pub const GRAB_TEXT: [(&str, u32); 4] = [
    ("ready", 0x7CFF8C),
    ("pulling in...", 0xFFE070),
    ("CLAW CRUSH", 0xFF9A40),
    ("thrown!", 0xFF5A5A),
];

#[derive(PartialEq, Clone, Copy)]
pub struct View { pub mode: usize, pub grab: usize, pub god: u8, pub ground: bool } // god: 0 unknown, 1 off, 2 on; ground: at the mission spawn (F12)

static LAST: Mutex<Option<(View, u32)>> = Mutex::new(None);

/// Re-renders the panel when its content or the screen height changed. Called from the mod loop.
pub fn update(view: View, modes: &[&str]) {
    let h = SCREEN_H.load(Relaxed);
    if h == 0 { return; }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == Some((view, h)) { return; }
    *last = Some((view, h));
    if let Some(mut img) = unsafe { render(view, modes, h) } {
        let mut g = IMAGE.lock().unwrap_or_else(|e| e.into_inner());
        img.ver = g.as_ref().map_or(1, |o| o.ver.wrapping_add(1));
        *g = Some(img);
    }
}

// colours are 0xRRGGBB; GDI wants 0x00BBGGRR
fn bgr(c: u32) -> u32 { ((c & 0xFF) << 16) | (c & 0xFF00) | (c >> 16) }

unsafe fn render(view: View, modes: &[&str], screen_h: u32) -> Option<Image> {
    let u = (screen_h as f32 / 1080.0).max(0.5); // 1.0 at 1080p
    let s = |v: f32| (v * u).round() as i32;
    let (w, h) = (s(330.0), s(526.0));

    let dc = CreateCompatibleDC(std::ptr::null_mut());
    // BITMAPINFOHEADER: top-down 32-bit
    let bmi: [u32; 10] = [40, w as u32, (-h) as u32, 1 | (32 << 16), 0, 0, 0, 0, 0, 0];
    let mut bits: *mut u32 = std::ptr::null_mut();
    let bmp = CreateDIBSection(dc, bmi.as_ptr(), 0, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() { DeleteDC(dc); return None; }
    let old_bmp = SelectObject(dc, bmp);
    SetBkMode(dc, 1); // TRANSPARENT

    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let font = |px: f32, weight: i32| CreateFontW(-s(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 4 /* ANTIALIASED */, 0, face.as_ptr());
    let (f_title, f_bold, f_text, f_small) = (font(30.0, 800), font(20.0, 700), font(20.0, 400), font(16.0, 400));
    let rect = |x: i32, y: i32, rw: i32, rh: i32, c: u32| {
        let br = CreateSolidBrush(bgr(c));
        FillRect(dc, &[x, y, x + rw, y + rh], br);
        DeleteObject(br);
    };
    let text = |x: i32, y: i32, f: HANDLE, c: u32, t: &str| {
        SelectObject(dc, f);
        SetTextColor(dc, bgr(c));
        let wt: Vec<u16> = t.encode_utf16().collect();
        TextOutW(dc, x, y, wt.as_ptr(), wt.len() as i32);
    };

    const GOLD: u32 = 0xF2B632;
    const WHITE: u32 = 0xF0F0F0;
    const DIM: u32 = 0x9AA0A8;
    let pad = s(18.0);

    rect(0, 0, s(5.0), h, GOLD); // accent bar
    text(pad, s(12.0), f_title, GOLD, "OMEGA SUPREME");
    rect(pad, s(52.0), w - 2 * pad, s(2.0), 0x5A4614);

    // melee modes (Q), T cycles
    let mut y = s(64.0);
    text(pad, y, f_bold, WHITE, "Q  melee");
    text(w - pad - s(118.0), y + s(3.0), f_small, DIM, "T = next mode");
    y += s(32.0);
    for (i, m) in modes.iter().enumerate() {
        let on = i == view.mode;
        if on {
            rect(pad - s(6.0), y - s(2.0), w - 2 * pad + s(12.0), s(30.0), 0x5C4510);
            rect(pad - s(6.0), y - s(2.0), s(4.0), s(30.0), GOLD);
        }
        text(pad + s(6.0), y, if on { f_bold } else { f_text }, if on { 0xFFD860 } else { DIM }, &format!("{}   {m}", i + 1));
        y += s(32.0);
    }

    // other controls
    y += s(10.0);
    rect(pad, y, w - 2 * pad, s(1.0), 0x3A3A3A);
    y += s(12.0);
    let (gt, gc) = GRAB_TEXT[view.grab.min(3)];
    let rows: [(&str, &str, Option<(&str, u32)>); 6] = [
        ("F11", "tractor grab", Some((gt, gc))),
        ("Fire", "homing rockets", None),
        ("F2", "frenzy gun", None),
        ("F8", "god mode", match view.god { 2 => Some(("ON", 0x7CFF8C)), 1 => Some(("off", DIM)), _ => None }),
        ("F12", "sky arena / mission", Some(if view.ground { ("mission spawn", WHITE) } else { ("sky arena", DIM) })),
        ("Home", "hide this guide", None),
    ];
    for (key, what, status) in rows {
        rect(pad, y + s(1.0), s(60.0), s(26.0), 0x34383E); // key cap
        text(pad + s(7.0), y + s(2.0), f_bold, WHITE, key);
        text(pad + s(74.0), y + s(2.0), f_text, WHITE, what);
        y += s(32.0);
        if let Some((t, c)) = status {
            text(pad + s(74.0), y - s(4.0), f_small, c, &format!("> {t}"));
            y += s(24.0);
        }
    }
    GdiFlush();

    // GDI drew on black: brightness is the coverage. Premultiplied ARGB over a translucent black panel.
    let n = (w * h) as usize;
    let src = std::slice::from_raw_parts(bits, n);
    let px = src.iter().map(|&p| {
        let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
        let m = r.max(g).max(b);
        let a = m + (150 * (255 - m)) / 255; // panel background alpha ~0.59
        (a << 24) | (r << 16) | (g << 8) | b
    }).collect();

    for f in [f_title, f_bold, f_text, f_small] { DeleteObject(f); }
    SelectObject(dc, old_bmp);
    DeleteObject(bmp);
    DeleteDC(dc);
    Some(Image { w: w as u32, h: h as u32, px, ver: 0 })
}
