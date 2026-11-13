#![windows_subsystem = "windows"]
//! EyeBreak: every 20 minutes the screen fades to black for a 1-minute
//! guided eye break. Breaks wait while you're in a meeting or presenting,
//! and start a moment after it ends. A small draggable status pill stays on
//! top of everything (fullscreen games too). Background work keeps running:
//! the break is only a black window on top.
//! Pure Win32 + GDI+, single thread, ~0% CPU while waiting.

mod detect;
mod gfx;

use gfx::{argb, fade, Align, Canvas, Weight};
use std::cell::UnsafeCell;
use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows_sys::Win32::System::RemoteDesktop::WTSRegisterSessionNotification;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLS_MAIN: &str = "EyeBreak.Main";
const CLS_PILL: &str = "EyeBreak.Pill";
const CLS_CARD: &str = "EyeBreak.Card";
const CLS_OVER: &str = "EyeBreak.Overlay";
const CLS_PANEL: &str = "EyeBreak.Panel";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_PING: u32 = WM_APP + 2; // another copy was started: wave hello
const WM_WTSSESSION_CHANGE_: u32 = 0x02B1;
const WM_MOUSELEAVE_: u32 = 0x02A3;
const VK_ESCAPE_: u32 = 0x1B;

const IDM_BREAK: usize = 401;
const IDM_RESTART: usize = 402;
const IDM_FORGET: usize = 403;
const IDM_RESETPOS: usize = 404;
const IDM_QUIT: usize = 405;
const IDM_UNINSTALL: usize = 406;

// colours (ARGB)
const TEAL: u32 = argb(255, 46, 196, 176);
const TEAL_SOFT: u32 = argb(255, 140, 226, 212);
const AMBER: u32 = argb(255, 245, 185, 70);
const TEXT: u32 = argb(255, 236, 239, 243);
const SUB: u32 = argb(255, 150, 157, 168);
const BODY: u32 = argb(240, 21, 23, 28);
const BODY_HOVER: u32 = argb(248, 34, 37, 44);
const EDGE: u32 = argb(34, 255, 255, 255);

// sizes (dp)
const BADGE: i32 = 34; // collapsed: just the ring
const EXP_W: i32 = 132; // opened on hover
const PANEL_W: i32 = 256;
const CARD_W: i32 = 264;
const CARD_H: i32 = 120;
const PAD: i32 = 10; // room for the soft shadow around layered windows

const FADE_IN: i64 = 700;
const FADE_OUT: i64 = 500;
const ESC_HOLD: i64 = 3000;

const STAGES: [(&str, &str); 3] = [
    ("Look far away", "Find something at least 6 m (20 ft) away and let your focus go soft."),
    ("Close your eyes", "Breathe slowly with the circle. Relax your forehead, jaw and shoulders."),
    ("Blink and roll", "Blink slowly 10 times, then roll your eyes gently: up, right, down, left."),
];

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Work,
    Warn,
    Break,
}

#[derive(Clone, Copy)]
struct Drag {
    cursor: POINT,
    x: i32,
    y: i32,
    moved: bool,
}

struct App {
    hinst: HINSTANCE,
    test: bool,
    scale: f32,
    main: HWND,
    pill: HWND,
    card: HWND,
    panel: HWND,
    overlays: Vec<HWND>,
    hook: HHOOK,
    icon_big: HICON,
    icon_small: HICON,
    phase: Phase,
    phase_end: i64,
    cycle_start: i64,
    last_tick: i64,
    // meeting / presentation hold
    hold: bool,
    hold_presenting: bool,
    hold_reason: String,
    hold_short: String,
    after_hold: bool,
    clear_since: Option<i64>,
    ask: Option<String>,
    ask_app: String,
    ask_dismissed: bool,
    last_check: i64,
    lock_at: Option<i64>,
    // break
    break_started: i64,
    stage: usize,
    skipped: bool,
    last_top: i64,
    breaks_today: u32,
    day: u32,
    // floating pill
    pill_x: i32,
    pill_y: i32,
    hover: bool,
    drag: Option<Drag>,
    flash_until: i64,
    expand: f32,
    last_anim: i64,
    panel_x: i32,
    panel_y: i32,
    panel_items: Vec<PanelItem>,
    panel_hover: i32,
    panel_closed_at: i64,
    card_hover: i32,
    card_btns: [RECT; 2],
    last_tip: i64,
    timer_ms: u32,
    taskbar_msg: u32,
}

impl App {
    fn work_ms(&self) -> i64 {
        if self.test { 20_000 } else { 19 * 60_000 }
    }
    fn warn_ms(&self) -> i64 {
        if self.test { 15_000 } else { 60_000 }
    }
    fn break_ms(&self) -> i64 {
        if self.test { 15_000 } else { 60_000 }
    }
    /// a meeting must look over for this long before the countdown starts
    fn grace_ms(&self) -> i64 {
        if self.test { 4_000 } else { 10_000 }
    }
}

struct Global(UnsafeCell<Option<App>>);
unsafe impl Sync for Global {}
static APP: Global = Global(UnsafeCell::new(None));
static BREAK_ACTIVE: AtomicBool = AtomicBool::new(false);
static ESC_DOWN_AT: AtomicI64 = AtomicI64::new(0);

fn app() -> &'static mut App {
    unsafe { (*APP.0.get()).as_mut().expect("app state") }
}

// ---------- helpers ----------
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
fn px(v: i32) -> i32 {
    (v as f32 * app().scale).round() as i32
}
fn mmss(secs: i64) -> String {
    let s = secs.max(0);
    format!("{}:{:02}", s / 60, s % 60)
}
fn secs_left(end: i64) -> i64 {
    ((end - now_ms()).max(0) + 999) / 1000
}
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32)
}
fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("EyeBreak")
}
fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("EyeBreak")
}
fn today() -> u32 {
    unsafe {
        let mut t: SYSTEMTIME = zeroed();
        GetLocalTime(&mut t);
        t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32
    }
}

/// Double-buffered WM_PAINT (no flicker).
unsafe fn paint(h: HWND, f: impl FnOnce(HDC, &RECT)) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(h, &mut ps);
    let mut r: RECT = zeroed();
    GetClientRect(h, &mut r);
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, r.right.max(1), r.bottom.max(1));
    let old = SelectObject(mem, bmp as _);
    f(mem, &r);
    BitBlt(hdc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(bmp as _);
    DeleteDC(mem);
    EndPaint(h, &ps);
}

unsafe fn work_area() -> RECT {
    let mut r: RECT = zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as _, 0);
    r
}

unsafe fn monitor_work(pt: POINT) -> RECT {
    let m = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(m, &mut mi) == 0 {
        return work_area();
    }
    mi.rcWork
}

unsafe fn reg_class(name: &str, proc_: WNDPROC, bg: HBRUSH) {
    let a = app();
    let n = w(name);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: proc_,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: a.hinst,
        hIcon: a.icon_big,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: bg,
        lpszMenuName: null(),
        lpszClassName: n.as_ptr(),
        hIconSm: a.icon_small,
    };
    RegisterClassExW(&wc);
}

// ---------- keyboard lock during a break ----------
unsafe extern "system" fn kb_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Every key press is swallowed during the break (releases pass, so no
    // modifier gets stuck). Holding Esc for 3 s skips the break.
    if code >= 0 && BREAK_ACTIVE.load(Ordering::Relaxed) {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        let m = wp as u32;
        let down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
        if k.vkCode == VK_ESCAPE_ {
            if down {
                if ESC_DOWN_AT.load(Ordering::Relaxed) == 0 {
                    ESC_DOWN_AT.store(now_ms(), Ordering::Relaxed);
                }
            } else {
                ESC_DOWN_AT.store(0, Ordering::Relaxed);
            }
        }
        if down {
            return 1;
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

unsafe fn force_fg(h: HWND) {
    let fg = GetForegroundWindow();
    let me = GetCurrentThreadId();
    let other = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, null_mut()) };
    let attach = other != 0 && other != me;
    if attach {
        AttachThreadInput(me, other, 1);
    }
    BringWindowToTop(h);
    SetForegroundWindow(h);
    if attach {
        AttachThreadInput(me, other, 0);
    }
}

// ---------- phase machine ----------
unsafe fn start_work() {
    let a = app();
    let now = now_ms();
    a.phase = Phase::Work;
    a.phase_end = now + a.work_ms();
    a.cycle_start = now;
    a.hold = false;
    a.after_hold = false;
    a.ask = None;
    a.ask_dismissed = false;
    a.clear_since = None;
    update_card();
    tray(NIM_MODIFY);
    render_pill();
}

unsafe fn start_warn() {
    let a = app();
    a.phase = Phase::Warn;
    a.phase_end = now_ms() + a.warn_ms();
    a.after_hold = a.hold;
    a.hold = false;
    a.ask_dismissed = false;
    a.clear_since = None;
    MessageBeep(MB_ICONASTERISK);
    update_card();
    tray(NIM_MODIFY);
}

/// Silently hold the break while something important is on.
unsafe fn start_hold(reason: String, short: String, presenting: bool) {
    let a = app();
    a.phase = Phase::Work;
    a.phase_end = now_ms();
    a.hold = true;
    a.hold_presenting = presenting;
    a.hold_reason = reason;
    a.hold_short = short;
    a.ask = None;
    a.clear_since = None;
    a.last_check = now_ms();
    update_card();
    tray(NIM_MODIFY);
}

unsafe extern "system" fn mon_cb(_m: HMONITOR, _dc: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
    (*(data as *mut Vec<RECT>)).push(*r);
    1
}

unsafe fn start_break() {
    let a = app();
    let now = now_ms();
    a.phase = Phase::Break;
    a.phase_end = now + a.break_ms();
    a.break_started = now;
    a.hold = false;
    a.stage = 0;
    a.skipped = false;
    a.ask = None;
    update_card();
    hide_panel();
    ShowWindow(a.pill, SW_HIDE);

    let mut rects: Vec<RECT> = Vec::new();
    EnumDisplayMonitors(null_mut(), null(), Some(mon_cb), &mut rects as *mut Vec<RECT> as LPARAM);
    if rects.is_empty() {
        rects.push(RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) });
    }
    for r in rects {
        // Just a black window on top: every app, download and cloud task
        // underneath keeps running exactly as before.
        let h = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            w(CLS_OVER).as_ptr(),
            w("EyeBreak").as_ptr(),
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            null_mut(),
            null_mut(),
            a.hinst,
            null(),
        );
        if !h.is_null() {
            SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
            ShowWindow(h, SW_SHOWNOACTIVATE);
            a.overlays.push(h);
        }
    }
    ESC_DOWN_AT.store(0, Ordering::Relaxed);
    BREAK_ACTIVE.store(true, Ordering::Relaxed);
    if a.hook.is_null() {
        a.hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), a.hinst, 0);
    }
    if let Some(&o) = a.overlays.first() {
        force_fg(o);
    }
    set_timer(40);
    tray(NIM_MODIFY);
}

unsafe fn end_break() {
    let a = app();
    BREAK_ACTIVE.store(false, Ordering::Relaxed);
    ESC_DOWN_AT.store(0, Ordering::Relaxed);
    if !a.hook.is_null() {
        UnhookWindowsHookEx(a.hook);
        a.hook = null_mut();
    }
    for o in std::mem::take(&mut a.overlays) {
        DestroyWindow(o);
    }
    if !a.skipped {
        let d = today();
        if d != a.day {
            a.day = d;
            a.breaks_today = 0;
        }
        a.breaks_today += 1;
    }
    start_work();
}

unsafe fn keep_on_top() {
    let a = app();
    for &o in &a.overlays {
        SetWindowPos(o, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    let fg = GetForegroundWindow();
    if !a.overlays.contains(&fg) {
        if let Some(&o) = a.overlays.first() {
            force_fg(o);
        }
    }
}

unsafe fn set_timer(ms: u32) {
    let a = app();
    if a.timer_ms != ms {
        a.timer_ms = ms;
        SetTimer(a.main, 1, ms, None);
    }
}

unsafe fn check_busy() {
    let a = app();
    match detect::detect() {
        Some(b) if b.sure => start_hold(b.reason, b.short, b.presenting),
        other => {
            a.ask_app = other.as_ref().map(|b| b.app.clone()).unwrap_or_default();
            let ask = other.map(|b| b.reason);
            if ask != a.ask {
                a.ask = ask;
                update_card();
            }
        }
    }
}

unsafe fn tick() {
    let a = app();
    let now = now_ms();
    let gap = now - a.last_tick;
    a.last_tick = now;
    // PC was asleep: your eyes rested, so start a fresh 20 minutes.
    if gap > 90_000 && a.phase != Phase::Break {
        start_work();
    }
    match a.phase {
        Phase::Work if now >= a.phase_end => {
            if a.hold {
                // Holding for a meeting/presentation: is it still going?
                if now - a.last_check >= 3000 {
                    a.last_check = now;
                    match detect::detect() {
                        Some(b) => {
                            a.hold_reason = b.reason;
                            a.hold_short = b.short;
                            a.hold_presenting = b.presenting;
                            a.clear_since = None;
                        }
                        None => {
                            let since = *a.clear_since.get_or_insert(now);
                            if now - since >= a.grace_ms() {
                                start_warn();
                            }
                        }
                    }
                }
            } else {
                a.last_check = now;
                match detect::detect() {
                    Some(b) if b.sure => start_hold(b.reason, b.short, b.presenting),
                    other => {
                        a.ask_app = other.as_ref().map(|b| b.app.clone()).unwrap_or_default();
                        a.ask = other.map(|b| b.reason);
                        start_warn();
                    }
                }
            }
        }
        Phase::Work => {}
        Phase::Warn => {
            if now >= a.phase_end {
                start_break();
            } else if now - a.last_check >= 3000 {
                a.last_check = now;
                check_busy();
            }
        }
        Phase::Break => {
            let esc = ESC_DOWN_AT.load(Ordering::Relaxed);
            if esc > 0 && now - esc >= ESC_HOLD && now < a.phase_end {
                a.phase_end = now; // skip: fade out now
                a.skipped = true;
            }
            if now >= a.phase_end + FADE_OUT {
                end_break();
            } else {
                let k = if now >= a.phase_end {
                    1.0 - (now - a.phase_end) as f32 / FADE_OUT as f32
                } else {
                    ((now - a.break_started) as f32 / FADE_IN as f32).min(1.0)
                };
                let alpha = (ease(k) * 255.0) as u8;
                let total = a.break_ms().max(1);
                let stage = (((now - a.break_started) * 3 / total) as usize).min(2);
                if stage != a.stage {
                    a.stage = stage;
                    if stage == 2 {
                        MessageBeep(MB_OK); // "open your eyes"
                    }
                }
                if now - a.last_top >= 250 {
                    a.last_top = now;
                    keep_on_top();
                }
                for &o in &a.overlays {
                    SetLayeredWindowAttributes(o, 0, alpha, LWA_ALPHA);
                    InvalidateRect(o, null(), 0);
                }
            }
        }
    }

    // Floating pill: always on top of everything (games included). Hidden
    // during the break itself and while presenting, so the audience never
    // sees it.
    let a = app();
    let want_pill = a.phase != Phase::Break && !(a.hold && a.hold_presenting);
    if want_pill != (IsWindowVisible(a.pill) != 0) {
        if want_pill {
            render_pill();
        }
        ShowWindow(a.pill, if want_pill { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
    let mut moving = false;
    if want_pill {
        moving = step_expand(now);
        if a.drag.is_none() {
            render_pill();
            if now - a.last_top >= 2000 {
                a.last_top = now;
                SetWindowPos(a.pill, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                if IsWindowVisible(a.card) != 0 {
                    SetWindowPos(a.card, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
    }
    if IsWindowVisible(a.panel) != 0 && !moving {
        render_panel();
    }
    if now - a.last_tip > 15_000 {
        a.last_tip = now;
        tray(NIM_MODIFY);
    }
    let animating = a.phase == Phase::Warn || (a.hold && a.clear_since.is_some()) || now < a.flash_until;
    set_timer(match a.phase {
        Phase::Break => 40,
        _ if moving => 15,
        _ if animating => 60,
        _ => 1000,
    });
}

// ---------- status text ----------
fn status_line() -> String {
    let a = app();
    match a.phase {
        Phase::Break => "Eye break in progress".into(),
        Phase::Warn => format!("Eye break in {}", mmss(secs_left(a.phase_end))),
        Phase::Work if a.hold && a.clear_since.is_some() => "Meeting looks over: eye break in a moment".into(),
        Phase::Work if a.hold => format!("On hold: {}", a.hold_reason),
        Phase::Work => {
            let mins = (secs_left(a.phase_end + a.warn_ms()) + 59) / 60;
            format!("Next eye break in {mins} min")
        }
    }
}

// ---------- tray ----------
unsafe fn tray(msg: u32) {
    let a = app();
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = a.main;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    nid.hIcon = a.icon_small;
    let tip = format!("EyeBreak - {}", status_line());
    let t: Vec<u16> = tip.encode_utf16().take(127).collect();
    nid.szTip[..t.len()].copy_from_slice(&t);
    if Shell_NotifyIconW(msg, &nid) == 0 && msg == NIM_MODIFY {
        // Explorer wasn't ready at login, or restarted: (re)add the icon.
        Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

// ---------- actions (panel rows) ----------
unsafe fn run_cmd(id: usize) {
    let a = app();
    match id {
        IDM_BREAK => start_break(),
        IDM_RESTART => start_work(),
        IDM_FORGET => detect::forget_all(),
        IDM_RESETPOS => {
            let (x, y) = default_pill_pos();
            a.pill_x = x;
            a.pill_y = y;
            let _ = std::fs::remove_file(data_dir().join("position.txt"));
            render_pill();
            update_card();
        }
        IDM_QUIT => {
            DestroyWindow(a.main);
        }
        IDM_UNINSTALL => uninstall(),
        _ => {}
    }
}

// ---------- floating badge ----------
// Collapsed: a 34 px ring showing the minutes left. Hover (or the last
// minute before a break) slides it open into a small pill with details.
unsafe fn default_pill_pos() -> (i32, i32) {
    let wa = work_area();
    (wa.right - px(BADGE) - px(10), wa.top + (wa.bottom - wa.top) * 42 / 100)
}

fn load_pos() -> Option<(i32, i32)> {
    let s = std::fs::read_to_string(data_dir().join("position.txt")).ok()?;
    let mut it = s.split_whitespace().map(|v| v.parse::<i32>());
    Some((it.next()?.ok()?, it.next()?.ok()?))
}

fn save_pos() {
    let a = app();
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(data_dir().join("position.txt"), format!("{} {}", a.pill_x, a.pill_y));
}

/// Keep the badge fully on a screen; with `snap`, stick it to a nearby edge.
unsafe fn clamp_pill(snap: bool) {
    let a = app();
    let b = px(BADGE);
    let wa = monitor_work(POINT { x: a.pill_x + b / 2, y: a.pill_y + b / 2 });
    let m = px(6);
    let (minx, maxx) = (wa.left + m, (wa.right - b - m).max(wa.left + m));
    let (miny, maxy) = (wa.top + m, (wa.bottom - b - m).max(wa.top + m));
    let mut x = a.pill_x.clamp(minx, maxx);
    let mut y = a.pill_y.clamp(miny, maxy);
    if snap {
        let s = px(28);
        if x - minx < s {
            x = minx;
        } else if maxx - x < s {
            x = maxx;
        }
        if y - miny < s {
            y = miny;
        } else if maxy - y < s {
            y = maxy;
        }
    }
    a.pill_x = x;
    a.pill_y = y;
}

/// The badge opens (and the panel appears) towards the middle of the screen.
unsafe fn opens_left() -> bool {
    let a = app();
    let b = px(BADGE);
    let wa = monitor_work(POINT { x: a.pill_x + b / 2, y: a.pill_y + b / 2 });
    a.pill_x + b / 2 > (wa.left + wa.right) / 2
}

/// (big line, small line, colour, ring fill 0..1, pulsing)
fn pill_state(now: i64) -> (String, String, u32, f32, bool) {
    let a = app();
    match a.phase {
        Phase::Warn => {
            let f = 1.0 - (a.phase_end - now) as f32 / a.warn_ms() as f32;
            let sub = if a.after_hold { "meeting over" } else { "eye break soon" };
            (mmss(secs_left(a.phase_end)), sub.into(), TEAL, f, true)
        }
        Phase::Work if a.hold && a.clear_since.is_some() => ("Wrapping up".into(), "break in a moment".into(), AMBER, 1.0, true),
        Phase::Work if a.hold => ("On hold".into(), a.hold_short.clone(), AMBER, 1.0, false),
        Phase::Work => {
            let left = secs_left(a.phase_end + a.warn_ms());
            let big = if left > 60 { format!("{} min", (left + 59) / 60) } else { mmss(left) };
            let f = (now - a.cycle_start) as f32 / (a.work_ms() + a.warn_ms()) as f32;
            (big, "until eye break".into(), TEAL, f, false)
        }
        Phase::Break => ("Break".into(), String::new(), TEAL, 1.0, false),
    }
}

fn expand_target(now: i64) -> bool {
    let a = app();
    let dragging = a.drag.map_or(false, |d| d.moved);
    !dragging && (a.hover || a.phase == Phase::Warn || now < a.flash_until)
}

/// Advance the open/close slide; true while still moving.
fn step_expand(now: i64) -> bool {
    let a = app();
    let dt = (now - a.last_anim).clamp(0, 60) as f32;
    a.last_anim = now;
    let target = if expand_target(now) { 1.0 } else { 0.0 };
    if a.expand < target {
        a.expand = (a.expand + dt / 160.0).min(target);
    } else if a.expand > target {
        a.expand = (a.expand - dt / 160.0).max(target);
    }
    a.expand != target
}

fn badge_width() -> i32 {
    let a = app();
    let b = px(BADGE);
    b + ((px(EXP_W) - b) as f32 * ease(a.expand)).round() as i32
}

unsafe fn render_pill() {
    let a = app();
    let pad = px(PAD);
    let b = px(BADGE);
    let bw = badge_width();
    let left = opens_left();
    let body_x = if left { a.pill_x + b - bw } else { a.pill_x };
    gfx::present_layered(a.pill, body_x - pad, a.pill_y - pad, bw + 2 * pad, b + 2 * pad, |c| draw_pill(c, now_ms(), bw, left));
}

fn draw_pill(c: &Canvas, now: i64, bw: i32, left: bool) {
    let a = app();
    let s = a.scale;
    let p = px(PAD) as f32;
    let b = px(BADGE) as f32;
    let bwf = bw as f32;
    let r = b / 2.0;
    let (big, small, col, frac, pulse) = pill_state(now);
    let lit = a.hover || a.drag.map_or(false, |d| d.moved) || now < a.flash_until;
    let k = if pulse { 0.5 + 0.5 * ((now as f32 / 1000.0 * std::f32::consts::PI * 1.4).sin() * 0.5 + 0.5) } else { 1.0 };

    c.shadow(p, p, bwf, b, r, 5.0 * s, 0.32);
    c.fill_round(p, p, bwf, b, r, if lit { BODY_HOVER } else { BODY });
    c.stroke_round(p + 0.5, p + 0.5, bwf - 1.0, b - 1.0, r - 0.5, 1.0, if lit { fade(col, 0.45) } else { EDGE });

    // ring + what's inside it
    let (cx, cy) = (if left { p + bwf - r } else { p + r }, p + r);
    let (rr, sw) = (r - 5.0 * s, 2.4 * s);
    c.ring(cx, cy, rr, sw, argb(26, 255, 255, 255));
    c.arc(cx, cy, rr, sw, -90.0, 360.0 * frac.clamp(0.0, 1.0), fade(col, k));
    if a.hold && a.phase == Phase::Work {
        let (bw2, bh2) = (2.4 * s, 8.5 * s);
        for dx in [-2.6 * s, 2.6 * s] {
            c.fill_round(cx + dx - bw2 / 2.0, cy - bh2 / 2.0, bw2, bh2, bw2 / 2.0, fade(AMBER, k));
        }
    } else {
        let num = match a.phase {
            Phase::Warn => secs_left(a.phase_end).to_string(),
            _ => {
                let left_s = secs_left(a.phase_end + a.warn_ms());
                if left_s > 60 { ((left_s + 59) / 60).to_string() } else { left_s.to_string() }
            }
        };
        let nc = if a.phase == Phase::Warn { TEAL_SOFT } else { TEXT };
        c.text(&num, cx - r, cy - r + 0.5 * s, b, b, 11.5 * s, Weight::Semibold, nc, Align::Center, false);
    }

    // details, faded in as it opens
    let t = ((a.expand - 0.45) / 0.55).clamp(0.0, 1.0);
    if t > 0.0 {
        let tw = bwf - b - 12.0 * s;
        let tx = if left { p + 12.0 * s } else { p + b + 2.0 * s };
        let small_col = if a.hold { AMBER } else { SUB };
        c.text(&big, tx, p + 3.0 * s, tw, 15.0 * s, 12.5 * s, Weight::Semibold, fade(TEXT, t), Align::Left, false);
        c.text(&small, tx, p + 16.5 * s, tw, 13.0 * s, 10.0 * s, Weight::Regular, fade(small_col, t), Align::Left, false);
    }
}

unsafe fn kick_anim() {
    let a = app();
    a.last_anim = now_ms();
    set_timer(15);
}

unsafe extern "system" fn pill_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match m {
        WM_LBUTTONDOWN => {
            SetCapture(h);
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            a.drag = Some(Drag { cursor: pt, x: a.pill_x, y: a.pill_y, moved: false });
            0
        }
        WM_MOUSEMOVE => {
            if let Some(d) = a.drag.as_mut() {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                let (dx, dy) = (pt.x - d.cursor.x, pt.y - d.cursor.y);
                if !d.moved && (dx.abs() > px(4) || dy.abs() > px(4)) {
                    d.moved = true;
                    a.expand = 0.0; // drag the small badge, not the open pill
                    ShowWindow(a.card, SW_HIDE);
                    hide_panel();
                }
                if d.moved {
                    a.pill_x = d.x + dx;
                    a.pill_y = d.y + dy;
                    render_pill();
                }
            } else if !a.hover {
                a.hover = true;
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
                kick_anim();
            }
            0
        }
        WM_MOUSELEAVE_ => {
            a.hover = false;
            kick_anim();
            0
        }
        WM_LBUTTONUP => {
            let d = a.drag.take();
            ReleaseCapture();
            match d {
                Some(d) if d.moved => finish_drag(),
                Some(_) => toggle_panel(),
                None => {}
            }
            0
        }
        WM_CAPTURECHANGED => {
            if let Some(d) = a.drag.take() {
                if d.moved {
                    finish_drag();
                }
            }
            0
        }
        WM_RBUTTONUP => {
            toggle_panel();
            0
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(null_mut(), if a.drag.map_or(false, |d| d.moved) { IDC_SIZEALL } else { IDC_HAND }));
            1
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_CLOSE => 0,
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

unsafe fn finish_drag() {
    clamp_pill(true);
    save_pos();
    kick_anim();
    render_pill();
    update_card();
}

// ---------- panel (click the badge or the tray icon) ----------
struct PanelItem {
    id: usize,
    rect: RECT,
    label: &'static str,
    footer: bool,
}

/// Lay the panel out in window coordinates; returns (window w, window h).
fn build_panel() -> (i32, i32) {
    let a = app();
    let pad = px(PAD);
    let wd = px(PANEL_W);
    let mut items = Vec::new();
    let mut y = pad + px(76); // header + progress bar
    let mut rows: Vec<(usize, &'static str)> = vec![(IDM_BREAK, "Take an eye break now"), (IDM_RESTART, "Restart the 20-min timer")];
    if !detect::learned().is_empty() {
        rows.push((IDM_FORGET, "Forget learned meeting apps"));
    }
    rows.push((IDM_RESETPOS, "Move back to the edge"));
    for (id, label) in rows {
        items.push(PanelItem { id, rect: RECT { left: pad + px(6), top: y, right: pad + wd - px(6), bottom: y + px(34) }, label, footer: false });
        y += px(34);
    }
    y += px(13); // divider
    let half = (wd - px(12)) / 2;
    items.push(PanelItem { id: IDM_QUIT, rect: RECT { left: pad + px(6), top: y, right: pad + px(6) + half, bottom: y + px(30) }, label: "Quit", footer: true });
    items.push(PanelItem { id: IDM_UNINSTALL, rect: RECT { left: pad + px(6) + half, top: y, right: pad + wd - px(6), bottom: y + px(30) }, label: "Uninstall", footer: true });
    y += px(30) + px(6);
    a.panel_items = items;
    (wd + 2 * pad, y + pad)
}

unsafe fn toggle_panel() {
    let a = app();
    if IsWindowVisible(a.panel) != 0 {
        hide_panel();
    } else if now_ms() - a.panel_closed_at > 300 {
        open_panel(false);
    }
}

unsafe fn open_panel(from_tray: bool) {
    let a = app();
    if a.phase == Phase::Break {
        return;
    }
    let d = today();
    if d != a.day {
        a.day = d;
        a.breaks_today = 0;
    }
    let pad = px(PAD);
    let (ww, wh) = build_panel();
    let (bw, bh) = (ww - 2 * pad, wh - 2 * pad);
    let (x, y);
    if from_tray {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let wa = monitor_work(pt);
        x = (pt.x - bw).clamp(wa.left + px(8), (wa.right - bw - px(8)).max(wa.left));
        y = (pt.y - bh - px(8)).clamp(wa.top + px(8), (wa.bottom - bh - px(8)).max(wa.top));
    } else {
        let b = px(BADGE);
        let wa = monitor_work(POINT { x: a.pill_x + b / 2, y: a.pill_y + b / 2 });
        let bx = if opens_left() { a.pill_x - bw - px(8) } else { a.pill_x + b + px(8) };
        x = bx.clamp(wa.left + px(4), (wa.right - bw - px(4)).max(wa.left));
        y = (a.pill_y + b / 2 - px(38)).clamp(wa.top + px(8), (wa.bottom - bh - px(8)).max(wa.top));
    }
    a.panel_x = x - pad;
    a.panel_y = y - pad;
    a.panel_hover = -1;
    render_panel();
    ShowWindow(a.panel, SW_SHOW);
    force_fg(a.panel);
}

unsafe fn hide_panel() {
    let a = app();
    if IsWindowVisible(a.panel) != 0 {
        ShowWindow(a.panel, SW_HIDE);
        a.panel_closed_at = now_ms();
    }
}

unsafe fn render_panel() {
    let a = app();
    let pad = px(PAD);
    let wd = px(PANEL_W) + 2 * pad;
    let ht = a.panel_items.last().map_or(px(200), |i| i.rect.bottom + px(6) + pad);
    gfx::present_layered(a.panel, a.panel_x, a.panel_y, wd, ht, |c| draw_panel(c, now_ms(), ht));
}

fn draw_panel(c: &Canvas, now: i64, win_h: i32) {
    let a = app();
    let s = a.scale;
    let p = px(PAD) as f32;
    let wd = px(PANEL_W) as f32;
    let ht = win_h as f32 - 2.0 * p;
    let (big, _, col, frac, _) = pill_state(now);
    let sub = match a.phase {
        Phase::Warn => "until your eye break starts".to_string(),
        Phase::Work if a.hold && a.clear_since.is_some() => "Meeting looks over, break in a moment".to_string(),
        Phase::Work if a.hold => format!("{}. The break waits.", a.hold_reason),
        _ => "until your next eye break".to_string(),
    };

    c.shadow(p, p, wd, ht, 14.0 * s, 10.0 * s, 0.5);
    c.fill_round(p, p, wd, ht, 14.0 * s, argb(252, 22, 24, 29));
    c.stroke_round(p + 0.5, p + 0.5, wd - 1.0, ht - 1.0, 14.0 * s, 1.0, EDGE);

    // header
    let x0 = p + 16.0 * s;
    c.text(&big, x0, p + 12.0 * s, wd - 110.0 * s, 28.0 * s, 21.0 * s, Weight::Semibold, TEXT, Align::Left, false);
    c.text(&sub, x0, p + 39.0 * s, wd - 32.0 * s, 16.0 * s, 11.5 * s, Weight::Regular, if a.hold { AMBER } else { SUB }, Align::Left, false);
    let chip = match a.breaks_today {
        1 => "1 break today".to_string(),
        n => format!("{n} breaks today"),
    };
    let (chw, chh) = (88.0 * s, 20.0 * s);
    let chx = p + wd - 14.0 * s - chw;
    c.fill_round(chx, p + 16.0 * s, chw, chh, chh / 2.0, argb(20, 255, 255, 255));
    c.text(&chip, chx, p + 16.0 * s, chw, chh, 10.5 * s, Weight::Regular, SUB, Align::Center, false);
    // progress
    let (bx, by, bw, bh) = (x0, p + 62.0 * s, wd - 32.0 * s, 4.0 * s);
    c.fill_round(bx, by, bw, bh, bh / 2.0, argb(22, 255, 255, 255));
    if frac > 0.01 {
        c.fill_round(bx, by, (bw * frac.clamp(0.0, 1.0)).max(bh), bh, bh / 2.0, col);
    }

    // rows
    let mut divider = 0.0;
    for (i, it) in a.panel_items.iter().enumerate() {
        let r = &it.rect;
        let (rx, ry, rw, rh) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
        let hot = a.panel_hover == i as i32;
        if it.footer {
            divider = ry - 7.0 * s;
            c.text(it.label, rx, ry, rw, rh, 11.5 * s, Weight::Regular, if hot { TEXT } else { argb(255, 120, 126, 136) }, Align::Center, false);
            continue;
        }
        if hot {
            c.fill_round(rx, ry + 1.0 * s, rw, rh - 2.0 * s, 8.0 * s, argb(18, 255, 255, 255));
        }
        let (ix, iy) = (rx + 18.0 * s, ry + rh / 2.0);
        let ic = if hot { TEAL_SOFT } else { TEAL };
        match it.id {
            IDM_BREAK => c.eye(ix, iy, 13.0 * s, 1.4 * s, ic, None),
            IDM_RESTART => {
                c.arc(ix, iy, 5.5 * s, 1.6 * s, -50.0, 300.0, ic);
            }
            IDM_FORGET => {
                c.ring(ix, iy, 5.5 * s, 1.5 * s, ic);
            }
            _ => {
                c.ring(ix, iy, 5.5 * s, 1.5 * s, ic);
                c.fill_circle(ix, iy, 2.0 * s, ic);
            }
        }
        c.text(it.label, rx + 34.0 * s, ry, rw - 40.0 * s, rh, 12.5 * s, Weight::Regular, TEXT, Align::Left, false);
    }
    if divider > 0.0 {
        c.fill_round(p + 14.0 * s, divider, wd - 28.0 * s, 1.0, 0.5, argb(20, 255, 255, 255));
    }
}

fn panel_hit(x: i32, y: i32) -> i32 {
    let a = app();
    a.panel_items
        .iter()
        .position(|i| x >= i.rect.left && x < i.rect.right && y >= i.rect.top && y < i.rect.bottom)
        .map_or(-1, |i| i as i32)
}

unsafe extern "system" fn panel_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match m {
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            let hit = panel_hit(x, y);
            if hit != a.panel_hover {
                a.panel_hover = hit;
                render_panel();
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            0
        }
        WM_MOUSELEAVE_ => {
            if a.panel_hover != -1 {
                a.panel_hover = -1;
                render_panel();
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            let hit = panel_hit(x, y);
            if hit >= 0 {
                let id = a.panel_items[hit as usize].id;
                hide_panel();
                run_cmd(id);
            }
            0
        }
        WM_ACTIVATE => {
            if wp & 0xFFFF == 0 {
                hide_panel(); // clicked somewhere else
            }
            0
        }
        WM_KEYDOWN if wp as u32 == VK_ESCAPE_ => {
            hide_panel();
            0
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(null_mut(), if a.panel_hover >= 0 { IDC_HAND } else { IDC_ARROW }));
            1
        }
        WM_CLOSE => {
            hide_panel();
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

// ---------- "is this important?" card (only when unsure) ----------
unsafe fn update_card() {
    let a = app();
    let want = a.phase == Phase::Warn && a.ask.is_some() && !a.ask_dismissed;
    if !want {
        if IsWindowVisible(a.card) != 0 {
            ShowWindow(a.card, SW_HIDE);
        }
        return;
    }
    render_card();
    ShowWindow(a.card, SW_SHOWNOACTIVATE);
    SetWindowPos(a.card, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}

unsafe fn render_card() {
    let a = app();
    let pad = px(PAD);
    let (cw, ch) = (px(CARD_W), px(CARD_H));
    let (pw, ph) = (px(BADGE), px(BADGE));
    let wa = monitor_work(POINT { x: a.pill_x + pw / 2, y: a.pill_y + ph / 2 });
    // beside the (opened) badge, on the side it opens towards
    let x = if opens_left() { a.pill_x + pw - px(EXP_W) - cw - px(10) } else { a.pill_x + px(EXP_W) + px(10) };
    let y = (a.pill_y + ph / 2 - ch / 2).clamp(wa.top + px(8), (wa.bottom - ch - px(8)).max(wa.top));
    let reason = a.ask.clone().unwrap_or_default();
    let hover = a.card_hover;
    // buttons, in window coordinates
    let by = pad + px(80);
    a.card_btns = [
        RECT { left: pad + px(16), top: by, right: pad + px(16 + 150), bottom: by + px(28) },
        RECT { left: pad + px(16 + 150 + 8), top: by, right: pad + cw - px(16), bottom: by + px(28) },
    ];
    gfx::present_layered(a.card, x - pad, y - pad, cw + 2 * pad, ch + 2 * pad, |c| draw_card(c, &reason, hover));
}

fn draw_card(c: &Canvas, reason: &str, hover: i32) {
    let a = app();
    let s = a.scale;
    let pad = px(PAD);
    let (cw, ch) = (px(CARD_W), px(CARD_H));
    let btns = a.card_btns;
    {
        let p = pad as f32;
        let (cw, ch) = (cw as f32, ch as f32);
        c.shadow(p, p, cw, ch, 12.0 * s, 10.0 * s, 0.55);
        c.fill_round(p, p, cw, ch, 12.0 * s, argb(250, 24, 26, 32));
        c.stroke_round(p + 0.5, p + 0.5, cw - 1.0, ch - 1.0, 12.0 * s, 1.0, EDGE);
        c.fill_circle(p + 21.0 * s, p + 25.0 * s, 4.0 * s, AMBER);
        c.text("Is this a meeting?", p + 32.0 * s, p + 13.0 * s, cw - 48.0 * s, 24.0 * s, 14.0 * s, Weight::Semibold, TEXT, Align::Left, false);
        c.text(&format!("{reason}. If yes, the break waits until it ends."), p + 16.0 * s, p + 38.0 * s, cw - 32.0 * s, 36.0 * s, 12.0 * s, Weight::Regular, SUB, Align::Left, true);
        let rf = |r: &RECT| (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
        let (x0, y0, w0, h0) = rf(&btns[0]);
        c.fill_round(x0, y0, w0, h0, 8.0 * s, if hover == 0 { argb(255, 255, 204, 102) } else { AMBER });
        c.text("Yes, hold the break", x0, y0, w0, h0, 12.5 * s, Weight::Semibold, argb(255, 40, 28, 4), Align::Center, false);
        let (x1, y1, w1, h1) = rf(&btns[1]);
        c.fill_round(x1, y1, w1, h1, 8.0 * s, if hover == 1 { argb(40, 255, 255, 255) } else { argb(18, 255, 255, 255) });
        c.text("No", x1, y1, w1, h1, 12.5 * s, Weight::Semibold, TEXT, Align::Center, false);
    }
}

fn card_hit(x: i32, y: i32) -> i32 {
    let a = app();
    a.card_btns.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map_or(-1, |i| i as i32)
}

unsafe extern "system" fn card_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    match m {
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            let hit = card_hit(x, y);
            if hit != a.card_hover {
                a.card_hover = hit;
                render_card();
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            0
        }
        WM_MOUSELEAVE_ => {
            if a.card_hover != -1 {
                a.card_hover = -1;
                render_card();
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            match card_hit(x, y) {
                0 => {
                    let reason = a.ask.clone().unwrap_or_default();
                    detect::learn(&a.ask_app); // remember: this app means "meeting"
                    start_hold(reason, "Meeting".into(), false);
                }
                1 => {
                    a.ask_dismissed = true;
                    update_card();
                }
                _ => {}
            }
            0
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(null_mut(), if a.card_hover >= 0 { IDC_HAND } else { IDC_ARROW }));
            1
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_CLOSE => 0,
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

// ---------- full-screen break ----------
unsafe fn paint_overlay(h: HWND) {
    paint(h, |dc, r| {
        let c = Canvas::from_hdc(dc);
        draw_overlay(&c, r.right as f32, r.bottom as f32, now_ms());
    });
}

fn draw_overlay(c: &Canvas, wd: f32, ht: f32, now: i64) {
    let a = app();
    {
        c.clear(argb(255, 0, 0, 0));
        let u = ht.min(wd * 0.6);
        let total = a.break_ms().max(1) as f32;
        let left_ms = (a.phase_end - now).max(0) as f32;
        let frac = ((total - left_ms) / total).clamp(0.0, 1.0);
        let (cx, cy) = (wd / 2.0, ht * 0.40);

        // breathing circle: 4 s in, 6 s out (6 calm breaths a minute)
        let t = ((now - a.break_started).max(0) % 10_000) as f32 / 1000.0;
        let inhale = t < 4.0;
        let b = if inhale { ease(t / 4.0) } else { 1.0 - ease((t - 4.0) / 6.0) };
        let br = u * (0.085 + 0.045 * b);
        for i in 1..=8 {
            c.fill_circle(cx, cy, br + i as f32 * u * 0.009, argb(5, 46, 196, 176));
        }
        c.fill_circle(cx, cy, br, argb(40, 46, 196, 176));
        c.ring(cx, cy, br, (u * 0.0018).max(1.0), argb(80, 46, 196, 176));

        // time-left ring
        let (rr, sw) = (u * 0.175, (u * 0.0045).max(2.0));
        c.ring(cx, cy, rr, sw, argb(255, 26, 28, 33));
        c.arc(cx, cy, rr, sw, -90.0, 360.0 * (1.0 - frac), argb(210, 46, 196, 176));

        c.text("EYE BREAK", 0.0, cy - rr - u * 0.085, wd, u * 0.04, u * 0.021, Weight::Semibold, fade(TEAL, 0.85), Align::Center, false);
        c.text(&mmss(secs_left(a.phase_end)), cx - rr, cy - u * 0.072, rr * 2.0, u * 0.11, u * 0.085, Weight::Light, argb(255, 220, 224, 228), Align::Center, false);
        c.text(if inhale { "breathe in" } else { "breathe out" }, cx - rr, cy + u * 0.035, rr * 2.0, u * 0.04, u * 0.022, Weight::Regular, fade(TEAL_SOFT, 0.7), Align::Center, false);

        let stage = ((frac * 3.0) as usize).min(2);
        let (t1, t2) = STAGES[stage];
        let ty = cy + rr + u * 0.06;
        c.text(t1, 0.0, ty, wd, u * 0.06, u * 0.042, Weight::Semibold, argb(255, 226, 229, 233), Align::Center, false);
        c.text(t2, wd * 0.18, ty + u * 0.062, wd * 0.64, u * 0.075, u * 0.024, Weight::Regular, argb(255, 146, 152, 162), Align::Center, true);
        let dy = ty + u * 0.165;
        for i in 0..3 {
            let dx = cx + (i as f32 - 1.0) * u * 0.03;
            let col = if i <= stage { TEAL } else { argb(255, 50, 54, 60) };
            c.fill_circle(dx, dy, u * if i == stage { 0.0055 } else { 0.004 }, col);
        }

        let esc = ESC_DOWN_AT.load(Ordering::Relaxed);
        let (ft, fc) = if esc > 0 && now < a.phase_end {
            let left = (ESC_HOLD - (now - esc)).max(0);
            (format!("Keep holding Esc to skip... {}", (left + 999) / 1000), AMBER)
        } else {
            ("Hold Esc for 3 seconds to skip   \u{00B7}   Your apps, downloads and cloud tasks keep running".to_string(), argb(255, 84, 88, 96))
        };
        c.text(&ft, 0.0, ht - u * 0.10, wd, u * 0.04, u * 0.019, Weight::Regular, fc, Align::Center, false);
    }
}

/// `--preview <dir>`: render every screen to PNG (design checks, website art).
unsafe fn preview(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let a = app();
    a.scale = 2.0;
    let now = now_ms();
    let pad = px(PAD);
    for sz in [16, 24, 32, 48, 64, 256] {
        gfx::render_png(&dir.join(format!("icon-{sz}.png")), sz, sz, |c| gfx::draw_icon(c, sz as f32, TEAL));
    }
    let b = px(BADGE);
    let pill = |name: &str, open: f32| {
        let a = app();
        a.expand = open;
        let bw = badge_width();
        gfx::render_png(&dir.join(name), bw + 2 * pad, b + 2 * pad, |c| draw_pill(c, now, bw, false));
    };
    a.phase = Phase::Work;
    a.cycle_start = now - 3 * 60_000;
    a.phase_end = now + a.work_ms() - 3 * 60_000;
    pill("pill-work.png", 0.0);
    a.hover = true;
    pill("pill-hover.png", 1.0);
    a.hover = false;
    a.hold = true;
    a.hold_short = "Teams call".into();
    a.hold_reason = "You're in a Teams call".into();
    pill("pill-hold.png", 0.0);
    pill("pill-hold-open.png", 1.0);
    a.hold = false;
    a.breaks_today = 3;
    let (pw, ph) = build_panel();
    gfx::render_png(&dir.join("panel.png"), pw, ph, |c| draw_panel(c, now, ph));
    a.phase = Phase::Warn;
    a.phase_end = now + 42_000;
    pill("pill-warn.png", 1.0);

    let (cw, ch) = (px(CARD_W), px(CARD_H));
    let by = pad + px(80);
    a.card_btns = [
        RECT { left: pad + px(16), top: by, right: pad + px(16 + 150), bottom: by + px(28) },
        RECT { left: pad + px(16 + 150 + 8), top: by, right: pad + cw - px(16), bottom: by + px(28) },
    ];
    gfx::render_png(&dir.join("card.png"), cw + 2 * pad, ch + 2 * pad, |c| draw_card(c, "Chrome is using your microphone", -1));

    a.phase = Phase::Break;
    for (i, el) in [(0, 9_000), (1, 27_000), (2, 47_000)] {
        a.break_started = now - el - 1_500;
        a.phase_end = a.break_started + 1_500 + a.break_ms();
        gfx::render_png(&dir.join(format!("break-{i}.png")), 1920, 1080, |c| draw_overlay(c, 1920.0, 1080.0, now));
    }
}

// ---------- window procedures ----------
unsafe extern "system" fn main_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tb = app().taskbar_msg;
    if tb != 0 && m == tb {
        tray(NIM_ADD);
        return 0;
    }
    match m {
        WM_TIMER => {
            tick();
            0
        }
        WM_APP_TRAY => {
            let ev = (lp & 0xFFFF) as u32;
            if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU || ev == WM_LBUTTONUP {
                open_panel(true);
            }
            0
        }
        WM_APP_PING => {
            let a = app();
            clamp_pill(false);
            a.flash_until = now_ms() + 2500;
            kick_anim();
            0
        }
        WM_WTSSESSION_CHANGE_ => {
            let a = app();
            match wp as u32 {
                0x7 => a.lock_at = Some(now_ms()),
                0x8 => {
                    // Away (locked) for 1+ minute counts as a rest.
                    if let Some(t) = a.lock_at.take() {
                        if now_ms() - t >= 60_000 && a.phase != Phase::Break {
                            start_work();
                        }
                    }
                }
                _ => {}
            }
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            clamp_pill(false);
            render_pill();
            update_card();
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_DESTROY => {
            let a = app();
            BREAK_ACTIVE.store(false, Ordering::Relaxed);
            if !a.hook.is_null() {
                UnhookWindowsHookEx(a.hook);
                a.hook = null_mut();
            }
            tray(NIM_DELETE);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

unsafe extern "system" fn over_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            paint_overlay(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            SetCursor(null_mut());
            1
        }
        WM_CLOSE => 0,
        WM_SYSCOMMAND if (wp & 0xFFF0) as u32 == SC_CLOSE => 0,
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

// ---------- install / autostart / uninstall ----------
unsafe fn register_autostart() {
    let Ok(exe) = std::env::current_exe() else { return };
    let val = w(&format!("\"{}\"", exe.display()));
    RegSetKeyValueW(HKEY_CURRENT_USER, w(RUN_KEY).as_ptr(), w("EyeBreak").as_ptr(), REG_SZ, val.as_ptr() as _, (val.len() * 2) as u32);
}

/// Downloaded EyeBreak.exe run from anywhere: copy it to
/// %LOCALAPPDATA%\Programs\EyeBreak (replacing an older running copy) and
/// start it from there. Returns true when the installed copy took over.
unsafe fn self_install() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let dir = install_dir();
    let target = dir.join("EyeBreak.exe");
    if dir.as_os_str().is_empty() || exe.to_string_lossy().to_lowercase() == target.to_string_lossy().to_lowercase() {
        return false;
    }
    let old = FindWindowW(w(CLS_MAIN).as_ptr(), null());
    if !old.is_null() {
        PostMessageW(old, WM_CLOSE, 0, 0);
    }
    let _ = std::fs::create_dir_all(&dir);
    for _ in 0..60 {
        // a running copy keeps its exe locked, so this succeeds once it has quit
        if std::fs::copy(&exe, &target).is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(300));
            return std::process::Command::new(&target).spawn().is_ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

unsafe fn uninstall() {
    let r = MessageBoxW(
        null_mut(),
        w("Remove EyeBreak from this PC?\n\nIt stops now and won't start with Windows anymore.").as_ptr(),
        w("EyeBreak").as_ptr(),
        MB_YESNO | MB_ICONQUESTION | MB_TOPMOST,
    );
    if r != IDYES {
        return;
    }
    RegDeleteKeyValueW(HKEY_CURRENT_USER, w(RUN_KEY).as_ptr(), w("EyeBreak").as_ptr());
    let _ = std::fs::remove_dir_all(data_dir());
    if let Some(start_menu) = std::env::var_os("APPDATA") {
        let _ = std::fs::remove_file(PathBuf::from(start_menu).join(r"Microsoft\Windows\Start Menu\Programs\EyeBreak.lnk"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.to_string_lossy().to_lowercase() == install_dir().to_string_lossy().to_lowercase() {
                // remove the program folder a moment after we exit
                let _ = std::process::Command::new("cmd")
                    .raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"", dir.display()))
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .spawn();
            }
        }
    }
    DestroyWindow(app().main);
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("crash.log"), format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--detect") {
        let _ = std::fs::write(std::env::temp_dir().join("eyebreak-detect.txt"), detect::describe());
        return;
    }
    let test = args.iter().any(|a| a == "--test");
    let portable = test || args.iter().any(|a| a == "--portable" || a == "--preview");
    unsafe {
        if !portable && self_install() {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        gfx::startup();

        let now = now_ms();
        *APP.0.get() = Some(App {
            hinst: GetModuleHandleW(null()),
            test,
            scale: GetDpiForSystem() as f32 / 96.0,
            main: null_mut(),
            pill: null_mut(),
            card: null_mut(),
            panel: null_mut(),
            overlays: Vec::new(),
            hook: null_mut(),
            icon_big: null_mut(),
            icon_small: null_mut(),
            phase: Phase::Work,
            phase_end: 0,
            cycle_start: now,
            last_tick: now,
            hold: false,
            hold_presenting: false,
            hold_reason: String::new(),
            hold_short: String::new(),
            after_hold: false,
            clear_since: None,
            ask: None,
            ask_app: String::new(),
            ask_dismissed: false,
            last_check: 0,
            lock_at: None,
            break_started: 0,
            stage: 0,
            skipped: false,
            last_top: 0,
            breaks_today: 0,
            day: today(),
            pill_x: 0,
            pill_y: 0,
            hover: false,
            drag: None,
            flash_until: 0,
            expand: 0.0,
            last_anim: now,
            panel_x: 0,
            panel_y: 0,
            panel_items: Vec::new(),
            panel_hover: -1,
            panel_closed_at: 0,
            card_hover: -1,
            card_btns: [RECT { left: 0, top: 0, right: 0, bottom: 0 }; 2],
            last_tip: 0,
            timer_ms: 0,
            taskbar_msg: RegisterWindowMessageW(w("TaskbarCreated").as_ptr()),
        });
        let a = app();
        a.phase_end = now + a.work_ms();
        a.icon_big = gfx::make_icon(px(32).max(32), TEAL);
        a.icon_small = gfx::make_icon(GetSystemMetrics(SM_CXSMICON).max(16), TEAL);
        if let Some(i) = args.iter().position(|a| a == "--preview") {
            preview(&args.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("preview")));
            return;
        }
        // Only one copy ever runs; a second launch makes the pill wave.
        let _mutex = CreateMutexW(null(), 0, w("Local\\EyeBreak.SingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let h = FindWindowW(w(CLS_MAIN).as_ptr(), null());
            if !h.is_null() {
                PostMessageW(h, WM_APP_PING, 0, 0);
            }
            return;
        }

        reg_class(CLS_MAIN, Some(main_proc), null_mut());
        reg_class(CLS_PILL, Some(pill_proc), null_mut());
        reg_class(CLS_CARD, Some(card_proc), null_mut());
        reg_class(CLS_OVER, Some(over_proc), GetStockObject(BLACK_BRUSH) as HBRUSH);
        reg_class(CLS_PANEL, Some(panel_proc), null_mut());
        a.main = CreateWindowExW(0, w(CLS_MAIN).as_ptr(), w("EyeBreak").as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, null_mut(), null_mut(), a.hinst, null());
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
        a.pill = CreateWindowExW(ex, w(CLS_PILL).as_ptr(), w("EyeBreak").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), a.hinst, null());
        a.card = CreateWindowExW(ex, w(CLS_CARD).as_ptr(), w("EyeBreak").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), a.hinst, null());
        a.panel = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED, w(CLS_PANEL).as_ptr(), w("EyeBreak").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), a.hinst, null());

        let (x, y) = load_pos().unwrap_or_else(|| default_pill_pos());
        a.pill_x = x;
        a.pill_y = y;
        clamp_pill(false);
        render_pill();
        ShowWindow(a.pill, SW_SHOWNOACTIVATE);

        tray(NIM_ADD);
        WTSRegisterSessionNotification(a.main, 0);
        set_timer(1000);
        if !test {
            register_autostart();
        }

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
