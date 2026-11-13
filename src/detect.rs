//! "Is the screen important right now?"
//!
//! SURE  -> hold the break silently (nothing pops up on a shared screen):
//!   * PowerPoint slideshow, Windows presentation mode, a fullscreen
//!     Slides/Keynote/Canva/Prezi window
//!   * a Zoom meeting window is open
//!   * any app has the CAMERA on (video call)
//!   * a known meeting app (Zoom, Teams, Webex, Slack, Discord, Skype,
//!     WhatsApp...) is using the MICROPHONE
//!   * a browser is using the mic AND a meeting tab is open (Meet, Teams web...)
//! MAYBE -> ask the user (mic used by something unknown)
//!   * a screen recorder / streamer (OBS, Loom...) is using the microphone
//! Games / normal fullscreen video are deliberately NOT important.
//!
//! Desktop apps are double-checked against the running process list, so a
//! crashed app that left a stale "mic in use" entry can't hold breaks forever.
//!
//! Mic/camera use comes from Windows' own privacy log
//! (CapabilityAccessManager): LastUsedTimeStop == 0 means "in use right now".

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Busy {
    pub sure: bool,
    pub presenting: bool,
    pub reason: String,
    /// 1-3 words for the floating button ("Teams call", "Presenting")
    pub short: String,
    /// app behind an unsure (mic) detection, so the user can teach it
    pub app: String,
}

fn sure(reason: impl Into<String>, short: impl Into<String>, presenting: bool) -> Option<Busy> {
    Some(Busy { sure: true, presenting, reason: reason.into(), short: short.into(), app: String::new() })
}

// ---------- learning ----------
// When the user says an unsure app is important, it's remembered here and
// treated as a meeting app from then on. Stored in %APPDATA%\EyeBreak\learned.txt
fn learned_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("EyeBreak").join("learned.txt")
}
fn learned_store() -> &'static std::sync::Mutex<Vec<String>> {
    static L: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let v = std::fs::read_to_string(learned_path())
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        std::sync::Mutex::new(v)
    })
}
pub fn learned() -> Vec<String> {
    learned_store().lock().map(|v| v.clone()).unwrap_or_default()
}
pub fn forget_all() {
    if let Ok(mut v) = learned_store().lock() {
        v.clear();
    }
    let _ = std::fs::remove_file(learned_path());
}
pub fn learn(app: &str) {
    if app.is_empty() {
        return;
    }
    if let Ok(mut v) = learned_store().lock() {
        if !v.iter().any(|a| a == app) {
            v.push(app.to_string());
            let p = learned_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, v.join("
"));
        }
    }
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const MEETING_APPS: &[&str] = &[
    "zoom", "ms-teams", "msteams", "teams", "microsoftteams", "webex", "ciscowebexstart", "atmgr",
    "webexhost", "skype", "slack", "discord", "whatsapp", "5319275a.whatsappdesktop", "telegram",
    "signal", "gotomeeting", "g2mcomm", "ringcentral", "bluejeans", "lark", "viber",
    "microsoft.yourphone", "phoneexperiencehost", "facetime", "googlemeet", "jitsi",
    // recording / streaming: also "don't black out the screen"
    "obs64", "obs32", "obs", "streamlabs", "loom", "camtasia", "camtasiarecorder",
];
const BROWSERS: &[&str] = &[
    "chrome", "msedge", "firefox", "brave", "opera", "vivaldi", "arc", "browser", "yandex", "chromium",
];
const MEETING_TITLES: &[&str] = &[
    "meet -", "google meet", "meet.google", "zoom", "microsoft teams", "| teams", "webex", "jitsi",
    "whereby", "huddle", "skype", "discord", "whatsapp", "meeting", "call with", "gotomeeting",
    "ringcentral",
];
const PRESENT_TITLES: &[&str] = &["slides", "powerpoint", "slide show", "presentation", "keynote", "prezi", "canva"];

pub fn detect() -> Option<Busy> {
    unsafe {
        if visible_class("screenClass") {
            return sure("You're presenting (PowerPoint slideshow)", "Presenting", true);
        }
        let mut st = 0;
        if SHQueryUserNotificationState(&mut st) >= 0 && st == 4 {
            return sure("Windows presentation mode is on", "Presenting", true);
        }
        let fg = GetForegroundWindow();
        if !fg.is_null() && is_fullscreen(fg) {
            let t = title(fg).to_lowercase();
            if PRESENT_TITLES.iter().any(|k| t.contains(k)) {
                return sure("You're presenting", "Presenting", true);
            }
        }
        if visible_class("ZPContentViewWndClass") || visible_class("ConfMultiTabContentWndClass") {
            return sure("You're in a Zoom meeting", "Zoom meeting", false);
        }

        let users = av_users();
        let taught = learned();
        let mut maybe: Option<(String, String)> = None;
        for u in &users {
            if u.camera {
                return sure(format!("{} has your camera on", u.pretty), "Camera on", false);
            }
            if MEETING_APPS.contains(&u.base.as_str()) || taught.contains(&u.base) {
                return sure(format!("You're in a {} call", u.pretty), format!("{} call", u.pretty), false);
            }
            if BROWSERS.contains(&u.base.as_str()) {
                let titles = window_titles();
                if titles.iter().any(|t| MEETING_TITLES.iter().any(|k| t.contains(k))) {
                    return sure(format!("You're in a meeting ({})", u.pretty), "Meeting", false);
                }
            }
            maybe.get_or_insert_with(|| (format!("{} is using your microphone", u.pretty), u.base.clone()));
        }
        maybe.map(|(reason, app)| Busy { sure: false, presenting: false, reason, short: "Mic in use".into(), app })
    }
}

/// Human readable, for the `--detect` self-test.
pub fn describe() -> String {
    match detect() {
        None => "NONE: nothing important detected -> normal break".into(),
        Some(b) if b.sure => format!("SURE: {} (presenting={}) -> break held silently", b.reason, b.presenting),
        Some(b) => format!("MAYBE: {} -> user is asked", b.reason),
    }
}

unsafe fn visible_class(class: &str) -> bool {
    let h = FindWindowW(w(class).as_ptr(), null());
    !h.is_null() && IsWindowVisible(h) != 0
}

unsafe fn title(h: HWND) -> String {
    let mut b = [0u16; 512];
    let n = GetWindowTextW(h, b.as_mut_ptr(), 512);
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

unsafe extern "system" fn enum_cb(h: HWND, lp: LPARAM) -> BOOL {
    if IsWindowVisible(h) != 0 {
        let t = title(h);
        if !t.is_empty() {
            (*(lp as *mut Vec<String>)).push(t.to_lowercase());
        }
    }
    1
}

unsafe fn window_titles() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    EnumWindows(Some(enum_cb), &mut v as *mut Vec<String> as LPARAM);
    v
}

unsafe fn is_fullscreen(h: HWND) -> bool {
    let mut r: RECT = zeroed();
    if GetWindowRect(h, &mut r) == 0 {
        return false;
    }
    let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) == 0 {
        return false;
    }
    let m = mi.rcMonitor;
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

struct AvUser {
    base: String,
    pretty: String,
    camera: bool,
}

unsafe fn av_users() -> Vec<AvUser> {
    let mut out = Vec::new();
    let mut procs: Option<Vec<String>> = None;
    for (cap, camera) in [("webcam", true), ("microphone", false)] {
        let base = format!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{cap}");
        scan(&base, camera, &mut out, &mut procs);
        scan(&format!(r"{base}\NonPackaged"), camera, &mut out, &mut procs);
    }
    out
}

unsafe fn scan(path: &str, camera: bool, out: &mut Vec<AvUser>, procs: &mut Option<Vec<String>>) {
    let mut hk: HKEY = null_mut();
    if RegOpenKeyExW(HKEY_CURRENT_USER, w(path).as_ptr(), 0, KEY_READ, &mut hk) != 0 {
        return;
    }
    let mut i = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = 512u32;
        if RegEnumKeyExW(hk, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) != 0 {
            break;
        }
        i += 1;
        let nm = String::from_utf16_lossy(&name[..len as usize]);
        if nm == "NonPackaged" {
            continue;
        }
        let start = qword(hk, &nm, "LastUsedTimeStart").unwrap_or(0);
        let stop = qword(hk, &nm, "LastUsedTimeStop");
        if start > 0 && stop == Some(0) {
            let (base, pretty) = names(&nm);
            // Desktop app (key is its exe path): only count it if it's really running.
            let alive = !nm.contains('#') || procs.get_or_insert_with(|| running_exes()).iter().any(|p| *p == base);
            if alive && !base.contains("eyebreak") && !out.iter().any(|u| u.base == base && u.camera == camera) {
                out.push(AvUser { base, pretty, camera });
            }
        }
    }
    RegCloseKey(hk);
}

unsafe fn qword(hk: HKEY, sub: &str, val: &str) -> Option<u64> {
    let mut v: u64 = 0;
    let mut sz = 8u32;
    const RRF_RT_QWORD_: u32 = 0x48;
    let r = RegGetValueW(hk, w(sub).as_ptr(), w(val).as_ptr(), RRF_RT_QWORD_, null_mut(), &mut v as *mut u64 as _, &mut sz);
    (r == 0).then_some(v)
}

/// "C:#Program Files#Zoom#bin#Zoom.exe" -> ("zoom", "Zoom")
/// "MSTeams_8wekyb3d8bbwe"              -> ("msteams", "Microsoft Teams")
fn names(key: &str) -> (String, String) {
    let last = key.rsplit('#').next().unwrap_or(key);
    let last = last.split('_').next().unwrap_or(last);
    let raw = last.strip_suffix(".exe").or_else(|| last.strip_suffix(".EXE")).unwrap_or(last);
    let base = raw.to_lowercase();
    let pretty = match base.as_str() {
        "zoom" => "Zoom",
        "ms-teams" | "msteams" | "teams" | "microsoftteams" => "Teams",
        "chrome" => "Chrome",
        "msedge" => "Edge",
        "firefox" => "Firefox",
        "brave" => "Brave",
        "discord" => "Discord",
        "slack" => "Slack",
        "skype" => "Skype",
        "whatsapp" | "5319275a.whatsappdesktop" => "WhatsApp",
        "webex" | "ciscowebexstart" | "atmgr" | "webexhost" => "Webex",
        "microsoft.yourphone" | "phoneexperiencehost" => "Phone Link",
        "obs64" | "obs32" | "obs" => "OBS",
        "loom" => "Loom",
        _ => raw,
    }
    .to_string();
    (base, pretty)
}

/// Lower-case exe names (without ".exe") of every running process.
unsafe fn running_exes() -> Vec<String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut v = Vec::new();
    let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if snap == INVALID_HANDLE_VALUE {
        return v;
    }
    let mut e: PROCESSENTRY32W = zeroed();
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = Process32FirstW(snap, &mut e);
    while ok != 0 {
        let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        let name = String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase();
        v.push(name.strip_suffix(".exe").unwrap_or(&name).to_string());
        ok = Process32NextW(snap, &mut e);
    }
    CloseHandle(snap);
    v
}
