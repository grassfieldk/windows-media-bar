#![windows_subsystem = "windows"]

use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering},
    Mutex,
};
use windows::{
    core::*,
    Media::Control::*,
    Storage::Streams::*,
    UI::ViewManagement::{UIColorType, UISettings},
    Win32::Foundation::*,
    Win32::Graphics::Gdi::*,
    Win32::Graphics::Imaging::*,
    Win32::System::Com::*,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::System::Registry::*,
    Win32::UI::Input::KeyboardAndMouse::*,
    Win32::UI::Shell::*,
    Win32::UI::WindowsAndMessaging::*,
};

// ── layout defaults ───────────────────────────────────────────────────────────
const DEFAULT_BAR_H:    i32 = 40;
const DEFAULT_FONT_SIZE: i32 = 13;
const DEFAULT_FONT_FACE: &str = "Segoe UI Variable Text";

// ── fixed layout constants (do not depend on bar height) ─────────────────────
const ACCENT_H: i32 = 1;
const BTN_W:    i32 = 36;
const BTN_H:    i32 = 26;
const GROUP_W:  i32 = BTN_W * 3; // = 108
const SIDE_PAD: i32 = 16;
const CORNER:   i32 = 6;

const SEEK_W:     i32 = 360;
const SEEK_H:     i32 = 3;
const SEEK_PAD_R: i32 = 24;
const TIME_W:     i32 = 82;
const TIME_GAP:   i32 = 16;

// Dynamic layout (computed from h = bar_h at runtime):
//   art_w   = h
//   group_x = h + SIDE_PAD
//   shr_x   = group_x + GROUP_W + SHR_GAP   (start of shuffle/repeat group)
//   rep_x   = shr_x + SHR_W                 (repeat button within that group)
//   text_x  = shr_x + SHR_W * 2 + 10

// ── colours (COLORREF = 0x00_BB_GG_RR) ───────────────────────────────────────
const C_BG:           u32 = 0x00_1C_1C_1C;
const C_ART_PH:       u32 = 0x00_28_28_28;
const C_BTN:          u32 = 0x00_2D_2D_2D;
const C_BTN_HOV:      u32 = 0x00_3D_3D_3D;
const C_BTN_BDR:      u32 = 0x00_48_48_48;
const C_BTN_BDR_H:    u32 = 0x00_68_68_68;
const C_ICON:         u32 = 0x00_CC_CC_CC;
const C_TITLE:        u32 = 0x00_F2_F2_F2;
const C_ARTIST:       u32 = 0x00_88_88_88;
const C_SEP:          u32 = 0x00_55_55_55;
const C_ACCENT_FALLBACK: u32 = 0x00_D4_78_00;

// ── Segoe MDL2 Assets codepoints ─────────────────────────────────────────────
const ICON_PREV:  &str = "\u{E892}";
const ICON_PLAY:  &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_NEXT:  &str = "\u{E893}";
const ICON_GEAR:       &str = "\u{E713}"; // Settings gear (Segoe MDL2 Assets)
const ICON_SHUFFLE:    &str = "\u{E8B1}"; // Shuffle
const ICON_REPEAT_ALL: &str = "\u{E8EE}"; // Repeat all
const ICON_REPEAT_ONE: &str = "\u{E8ED}"; // Repeat one

// ── shuffle/repeat group ──────────────────────────────────────────────────────
const SHR_W:   i32 = 28; // each shuffle/repeat button width
const SHR_GAP: i32 = 8;  // gap from play-group to the shuffle/repeat group

// ── gear button ───────────────────────────────────────────────────────────────
const GEAR_W:   i32 = 32; // gear button width
const GEAR_PAD: i32 = 8;  // gap between gear button and right window edge

// ── messages ──────────────────────────────────────────────────────────────────
const WM_APPBAR:       u32 = WM_APP + 1;
const WM_MEDIA_UPDATE: u32 = WM_APP + 2;
const WM_TRAYICON:     u32 = WM_APP + 3;
const WM_MOUSE_LEAVE:  u32 = 0x02A3;
const TRAY_UID:        u32 = 1;
const REFRESH_MS:      u64 = 1000;

// ── settings window control IDs ───────────────────────────────────────────────
const IDC_BARH_EDIT:     i32 = 101;
const IDC_FONT_COMBO:    i32 = 102;
const IDC_FONTSIZE_EDIT: i32 = 103;

// ── registry keys ─────────────────────────────────────────────────────────────
const STARTUP_KEY:  PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const STARTUP_NAME: PCWSTR = w!("MediaBar");
const SETTINGS_KEY: PCWSTR = w!("Software\\MediaBar");

// ── types ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Default)]
enum Hover { #[default] None, Prev, Play, Next, Shuffle, Repeat, Gear }

struct ShrGroupState {
    hover:           Hover,
    shuffle_active:  bool,
    shuffle_enabled: bool,
    repeat_active:   bool,
    repeat_enabled:  bool,
    rep_icon:        &'static str,
    font:            HFONT,
    accent:          u32,
}

#[derive(Clone, Default)]
struct MediaInfo {
    title:           String,
    artist:          String,
    playing:         bool,
    thumbnail:       Option<Arc<Vec<u8>>>,
    position_100ns:  i64,
    duration_100ns:  i64,
    shuffle:         bool,
    repeat:          Option<windows::Media::MediaPlaybackAutoRepeatMode>,
    shuffle_enabled: bool,
    repeat_enabled:  bool,
}

struct CachedArt {
    key:     String,
    hbmp:    isize, // small (art_w×art_w) — left thumbnail
    hbmp_bg: isize, // large (BG_ART_SIZE×BG_ART_SIZE) — blurred background
}

const BG_ART_SIZE: i32 = 300;

struct MonitorInfo {
    rect:    RECT,
    primary: bool,
}

struct Settings {
    bar_h:       i32,
    font_face:   String,
    font_size:   i32,
    monitor_idx: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_h:       DEFAULT_BAR_H,
            font_face:   DEFAULT_FONT_FACE.to_string(),
            font_size:   DEFAULT_FONT_SIZE,
            monitor_idx: 0,
        }
    }
}

struct AppState {
    info:          Mutex<MediaInfo>,
    hover:         Mutex<Hover>,
    accent:        AtomicU32,
    art:           Mutex<Option<CachedArt>>,
    seeking:       Mutex<Option<f64>>,
    effective_pos: AtomicI64,
    pos_sampled:   Mutex<std::time::Instant>,
    last_smtc_pos: AtomicI64,
    font:          AtomicUsize,
    font_btn:      AtomicUsize,
    settings:      Mutex<Settings>,
}

struct SettingsCtx {
    hwnd_main: isize,
    state_ptr: *mut AppState,
}
unsafe impl Send for SettingsCtx {}

// ── system accent color ───────────────────────────────────────────────────────
fn get_accent_color() -> u32 {
    (|| -> Option<u32> {
        let settings = UISettings::new().ok()?;
        let c = settings.GetColorValue(UIColorType::Accent).ok()?;
        Some((c.B as u32) << 16 | (c.G as u32) << 8 | c.R as u32)
    })()
    .unwrap_or(C_ACCENT_FALLBACK)
}

// ── settings persistence ──────────────────────────────────────────────────────
fn load_settings() -> Settings {
    let mut s = Settings::default();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, SETTINGS_KEY, 0, KEY_QUERY_VALUE, &mut key)
            != ERROR_SUCCESS
        {
            return s;
        }
        let mut ty = REG_VALUE_TYPE(0);

        let mut val = 0u32;
        let mut sz  = 4u32;
        if RegQueryValueExW(key, w!("BarH"), None, Some(&mut ty),
            Some(&mut val as *mut u32 as *mut u8), Some(&mut sz)) == ERROR_SUCCESS
            && ty == REG_DWORD
        {
            s.bar_h = (val as i32).clamp(20, 100);
        }

        let mut val = 0u32;
        let mut sz  = 4u32;
        if RegQueryValueExW(key, w!("FontSize"), None, Some(&mut ty),
            Some(&mut val as *mut u32 as *mut u8), Some(&mut sz)) == ERROR_SUCCESS
            && ty == REG_DWORD
        {
            s.font_size = (val as i32).clamp(8, 32);
        }

        let mut buf = [0u16; 256];
        let mut sz  = (buf.len() * 2) as u32;
        if RegQueryValueExW(key, w!("FontFace"), None, Some(&mut ty),
            Some(buf.as_mut_ptr() as *mut u8), Some(&mut sz)) == ERROR_SUCCESS
            && ty == REG_SZ
        {
            let chars = (sz / 2) as usize;
            let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
            if end > 0 {
                s.font_face = String::from_utf16_lossy(&buf[..end]);
            }
        }

        let mut val = 0u32;
        let mut sz  = 4u32;
        if RegQueryValueExW(key, w!("MonitorIdx"), None, Some(&mut ty),
            Some(&mut val as *mut u32 as *mut u8), Some(&mut sz)) == ERROR_SUCCESS
            && ty == REG_DWORD
        {
            s.monitor_idx = val;
        }

        let _ = RegCloseKey(key);
    }
    s
}

fn save_settings(s: &Settings) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyW(HKEY_CURRENT_USER, SETTINGS_KEY, &mut key) != ERROR_SUCCESS {
            return;
        }

        let v = s.bar_h as u32;
        let _ = RegSetValueExW(key, w!("BarH"), 0, REG_DWORD,
            Some(std::slice::from_raw_parts(&v as *const u32 as *const u8, 4)));

        let v = s.font_size as u32;
        let _ = RegSetValueExW(key, w!("FontSize"), 0, REG_DWORD,
            Some(std::slice::from_raw_parts(&v as *const u32 as *const u8, 4)));

        let wide: Vec<u16> = s.font_face.encode_utf16().chain([0u16]).collect();
        let _ = RegSetValueExW(key, w!("FontFace"), 0, REG_SZ,
            Some(std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2)));

        let v = s.monitor_idx;
        let _ = RegSetValueExW(key, w!("MonitorIdx"), 0, REG_DWORD,
            Some(std::slice::from_raw_parts(&v as *const u32 as *const u8, 4)));

        let _ = RegCloseKey(key);
    }
}

// ── startup registration ──────────────────────────────────────────────────────
fn is_startup_registered() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, STARTUP_KEY, 0, KEY_QUERY_VALUE, &mut key)
            != ERROR_SUCCESS
        {
            return false;
        }
        let found =
            RegQueryValueExW(key, STARTUP_NAME, None, None, None, None) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        found
    }
}

fn set_startup(enable: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, STARTUP_KEY, 0, KEY_SET_VALUE, &mut key)
            != ERROR_SUCCESS
        {
            return;
        }
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let s = format!("\"{}\"", exe.display());
                let wide: Vec<u16> = s.encode_utf16().chain([0u16]).collect();
                let bytes = std::slice::from_raw_parts(
                    wide.as_ptr() as *const u8,
                    wide.len() * 2,
                );
                let _ = RegSetValueExW(key, STARTUP_NAME, 0, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, STARTUP_NAME);
        }
        let _ = RegCloseKey(key);
    }
}

unsafe extern "system" fn monitor_enum_proc(
    hmon: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        list.push(MonitorInfo { rect: mi.rcMonitor, primary: mi.dwFlags & 1 != 0 });
    }
    BOOL(1)
}

fn enum_monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(), None,
            Some(monitor_enum_proc),
            LPARAM(&mut list as *mut Vec<MonitorInfo> as isize),
        );
    }
    list.sort_by_key(|m| (m.rect.left, m.rect.top));
    list
}

unsafe fn tray_icon_add(hwnd: HWND) {
    let hinstance = GetModuleHandleW(None).unwrap_or_default();
    let hicon = LoadIconW(hinstance, PCWSTR(std::ptr::with_exposed_provenance(1)))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default());

    let mut tip = [0u16; 128];
    let s: Vec<u16> = "MediaBar".encode_utf16().collect();
    tip[..s.len().min(127)].copy_from_slice(&s[..s.len().min(127)]);

    let nid = NOTIFYICONDATAW {
        cbSize:          std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd:            hwnd,
        uID:             TRAY_UID,
        uFlags:          NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon:           hicon,
        szTip:           tip,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn tray_icon_remove(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd:   hwnd,
        uID:    TRAY_UID,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn show_context_menu(hwnd: HWND) {
    let registered = is_startup_registered();
    let sp         = STATE_PTR.load(Ordering::Relaxed);
    let monitors   = enum_monitors();

    let current_idx = if !sp.is_null() {
        (*sp).settings.lock().unwrap().monitor_idx as usize
    } else { 0 };
    let effective_idx = current_idx.min(monitors.len().saturating_sub(1));

    let menu = CreatePopupMenu().unwrap();
    let _ = AppendMenuW(menu, MF_STRING, 3, w!("設定 (&P)"));

    if monitors.len() > 1 {
        let sub = CreatePopupMenu().unwrap();
        for (i, mon) in monitors.iter().enumerate() {
            let label = if mon.primary {
                format!("ディスプレイ {} (プライマリ)", i + 1)
            } else {
                format!("ディスプレイ {}", i + 1)
            };
            let wide: Vec<u16> = label.encode_utf16().chain([0u16]).collect();
            let flags = if i == effective_idx { MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(sub, flags, 100 + i, PCWSTR(wide.as_ptr()));
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("表示画面 (&D)"));
    }

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(std::ptr::null()));
    let _ = AppendMenuW(menu, MF_STRING, 1, w!("スタートアップ時に起動 (&S)"));
    let check_flag = if registered { MF_BYCOMMAND | MF_CHECKED } else { MF_BYCOMMAND | MF_UNCHECKED };
    let _ = CheckMenuItem(menu, 1, check_flag.0);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(std::ptr::null()));
    let _ = AppendMenuW(menu, MF_STRING, 2, w!("終了 (&X)"));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x, pt.y, 0, hwnd, None,
    );
    let _ = DestroyMenu(menu);
    // Required after tray-icon popup so the menu dismisses correctly
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));

    let cmd_val = cmd.0 as usize;
    if cmd_val >= 100 && cmd_val < 100 + monitors.len() {
        let new_idx = (cmd_val - 100) as u32;
        if !sp.is_null() {
            {
                let mut s = (*sp).settings.lock().unwrap();
                s.monitor_idx = new_idx;
                save_settings(&s);
            }
            apply_settings(hwnd, &*sp);
        }
    } else {
        match cmd.0 {
            1 => set_startup(!registered),
            2 => { let _ = DestroyWindow(hwnd); }
            3 => { if !sp.is_null() { open_settings(hwnd, sp); } }
            _ => {}
        }
    }
}

// ── SMTC ──────────────────────────────────────────────────────────────────────
fn get_thumbnail_bytes(thumb_ref: &IRandomAccessStreamReference) -> Option<Arc<Vec<u8>>> {
    let stream = thumb_ref.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 { return None; }
    let input: IInputStream = stream.cast().ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(Arc::new(bytes))
}

fn fetch_media_info(prev_title: &str, prev_thumb: Option<Arc<Vec<u8>>>) -> Option<MediaInfo> {
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .ok()?.get().ok()?;
    let s = mgr.GetCurrentSession().ok()?;
    let p = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let pb = s.GetPlaybackInfo().ok()?;
    if pb.PlaybackType().ok().and_then(|r| r.Value().ok())
        == Some(windows::Media::MediaPlaybackType::Video)
    {
        return None;
    }
    let controls       = pb.Controls().ok();
    let shuffle_enabled = controls.as_ref().and_then(|c| c.IsShuffleEnabled().ok()).unwrap_or(false);
    let repeat_enabled  = controls.as_ref().and_then(|c| c.IsRepeatEnabled().ok()).unwrap_or(false);
    let shuffle = pb.IsShuffleActive().ok().and_then(|r| r.Value().ok()).unwrap_or(false);
    let repeat  = pb.AutoRepeatMode().ok().and_then(|r| r.Value().ok());

    let title = p.Title().ok().map(|s| s.to_string()).unwrap_or_default();
    let thumbnail = if title == prev_title {
        prev_thumb
    } else {
        p.Thumbnail().ok().and_then(|r| get_thumbnail_bytes(&r))
    };
    let (position_100ns, duration_100ns) = (|| -> Option<(i64, i64)> {
        let tl = s.GetTimelineProperties().ok()?;
        let pos   = tl.Position().ok()?.Duration;
        let start = tl.StartTime().ok()?.Duration;
        let end   = tl.EndTime().ok()?.Duration;
        let dur = end - start;
        if dur <= 0 { return None; }
        Some((pos - start, dur))
    })().unwrap_or((0, 0));
    Some(MediaInfo {
        title,
        artist:  p.Artist().ok().map(|s| s.to_string()).unwrap_or_default(),
        playing: pb.PlaybackStatus().ok()
            == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing),
        thumbnail,
        position_100ns,
        duration_100ns,
        shuffle,
        repeat,
        shuffle_enabled,
        repeat_enabled,
    })
}

fn send_cmd(cmd: &str, hwnd_raw: isize) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = (|| -> Option<()> {
            let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .ok()?.get().ok()?;
            let s = mgr.GetCurrentSession().ok()?;
            match cmd.as_str() {
                "prev" => { let _ = s.TrySkipPreviousAsync().and_then(|a| a.get()); }
                "play" => { let _ = s.TryTogglePlayPauseAsync().and_then(|a| a.get()); }
                "next" => { let _ = s.TrySkipNextAsync().and_then(|a| a.get()); }
                "shuffle" => {
                    let pb  = s.GetPlaybackInfo().ok()?;
                    let cur = pb.IsShuffleActive().ok()
                        .and_then(|r| r.Value().ok())
                        .unwrap_or(false);
                    let _ = s.TryChangeShuffleActiveAsync(!cur).and_then(|a| a.get());
                }
                "repeat" => {
                    use windows::Media::MediaPlaybackAutoRepeatMode as Mode;
                    let pb  = s.GetPlaybackInfo().ok()?;
                    let cur = pb.AutoRepeatMode().ok().and_then(|r| r.Value().ok());
                    let next = match cur {
                        None | Some(Mode::None) => Mode::List,
                        Some(Mode::List)        => Mode::Track,
                        _                       => Mode::None,
                    };
                    let _ = s.TryChangeAutoRepeatModeAsync(next).and_then(|a| a.get());
                }
                _ => {}
            }
            Some(())
        })();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let ptr = STATE_PTR.load(Ordering::Relaxed);
        if !ptr.is_null() {
            if let Some(info) = fetch_media_info("", None) {
                (*ptr).last_smtc_pos.store(info.position_100ns, Ordering::Relaxed);
                (*ptr).effective_pos.store(info.position_100ns, Ordering::Relaxed);
                *(*ptr).pos_sampled.lock().unwrap() = std::time::Instant::now();
                (*ptr).info.lock().unwrap().clone_from(&info);
            }
            let _ = PostMessageW(
                HWND(hwnd_raw as *mut _),
                WM_MEDIA_UPDATE, WPARAM(0), LPARAM(0),
            );
        }
        CoUninitialize();
    });
}

fn seek_to(pos_100ns: i64, hwnd_raw: isize) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = (|| -> Option<()> {
            let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .ok()?.get().ok()?;
            let s = mgr.GetCurrentSession().ok()?;
            let _ = s.TryChangePlaybackPositionAsync(pos_100ns).and_then(|a| a.get());
            Some(())
        })();
        let ptr = STATE_PTR.load(Ordering::Relaxed);
        if !ptr.is_null() {
            (*ptr).last_smtc_pos.store(pos_100ns, Ordering::Relaxed);
            (*ptr).effective_pos.store(pos_100ns, Ordering::Relaxed);
            *(*ptr).pos_sampled.lock().unwrap() = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let ptr = STATE_PTR.load(Ordering::Relaxed);
        if !ptr.is_null() {
            let (pt, pth) = {
                let info = (*ptr).info.lock().unwrap();
                (info.title.clone(), info.thumbnail.clone())
            };
            if let Some(info) = fetch_media_info(&pt, pth) {
                (*ptr).last_smtc_pos.store(info.position_100ns, Ordering::Relaxed);
                (*ptr).effective_pos.store(info.position_100ns, Ordering::Relaxed);
                *(*ptr).pos_sampled.lock().unwrap() = std::time::Instant::now();
                (*ptr).info.lock().unwrap().clone_from(&info);
            }
            let _ = PostMessageW(
                HWND(hwnd_raw as *mut _),
                WM_MEDIA_UPDATE, WPARAM(0), LPARAM(0),
            );
        }
        CoUninitialize();
    });
}

// ── AppBar ────────────────────────────────────────────────────────────────────
fn appbar_register(hwnd: HWND, mon_rect: RECT, bar_h: i32) -> RECT {
    unsafe {
        let mut d = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            uCallbackMessage: WM_APPBAR,
            uEdge: ABE_BOTTOM,
            rc: RECT {
                left:   mon_rect.left,
                top:    mon_rect.bottom - bar_h,
                right:  mon_rect.right,
                bottom: mon_rect.bottom,
            },
            lParam: LPARAM(0),
        };
        SHAppBarMessage(ABM_NEW, &mut d);
        d.rc = RECT {
            left:   mon_rect.left,
            top:    mon_rect.bottom - bar_h,
            right:  mon_rect.right,
            bottom: mon_rect.bottom,
        };
        SHAppBarMessage(ABM_QUERYPOS, &mut d);
        d.rc.top = d.rc.bottom - bar_h;
        SHAppBarMessage(ABM_SETPOS, &mut d);
        d.rc
    }
}

fn appbar_remove(hwnd: HWND) {
    unsafe {
        let mut d = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            ..Default::default()
        };
        SHAppBarMessage(ABM_REMOVE, &mut d);
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────
fn fmt_time(t_100ns: i64) -> String {
    let s = (t_100ns / 10_000_000) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn hit_btn(x: i32, group_x: i32, shr_x: i32, rep_x: i32, gear_x: i32) -> Hover {
    let prev_x = group_x;
    let play_x = group_x + BTN_W;
    let next_x = group_x + BTN_W * 2;
    if      (prev_x..prev_x + BTN_W).contains(&x)  { Hover::Prev }
    else if (play_x..play_x + BTN_W).contains(&x)  { Hover::Play }
    else if (next_x..next_x + BTN_W).contains(&x)  { Hover::Next }
    else if (shr_x..shr_x + SHR_W).contains(&x)    { Hover::Shuffle }
    else if (rep_x..rep_x + SHR_W).contains(&x)    { Hover::Repeat }
    else if (gear_x..gear_x + GEAR_W).contains(&x) { Hover::Gear }
    else                                            { Hover::None }
}

fn w16(s: &str) -> Vec<u16> { s.encode_utf16().collect() }

unsafe fn make_font(face: PCWSTR, px: i32) -> HFONT {
    CreateFontW(
        -px, 0, 0, 0,
        FW_NORMAL.0 as i32,
        0, 0, 0,
        DEFAULT_CHARSET.0.into(),
        OUT_DEFAULT_PRECIS.0.into(),
        CLIP_DEFAULT_PRECIS.0.into(),
        CLEARTYPE_QUALITY.0.into(),
        (DEFAULT_PITCH.0 | FF_DONTCARE.0).into(),
        face,
    )
}

// Separable box blur on a top-down 32bpp BGRA pixel buffer.
// Two passes (H then V) applied twice gives a smooth Gaussian-like result.
fn box_blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let cnt = (2 * r + 1) as u32;
    for y in 0..h {
        let base = y * w * 4;
        let mut sums = [0u32; 3];
        // Init window centered at x=0: offsets -r..+r, left side clamps to pixel 0
        for i in 0..=(2 * r) {
            let sx = if i < r { 0 } else { (i - r).min(w - 1) };
            for c in 0..3 { sums[c] += src[base + sx * 4 + c] as u32; }
        }
        for x in 0..w {
            for c in 0..3 { dst[base + x * 4 + c] = (sums[c] / cnt) as u8; }
            dst[base + x * 4 + 3] = src[base + x * 4 + 3];
            let add_x = (x + r + 1).min(w - 1);
            let rem_x = x.saturating_sub(r);
            for c in 0..3 {
                sums[c] += src[base + add_x * 4 + c] as u32;
                sums[c] -= src[base + rem_x * 4 + c] as u32;
            }
        }
    }
}

fn box_blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let cnt = (2 * r + 1) as u32;
    for x in 0..w {
        let mut sums = [0u32; 3];
        // Init window centered at y=0: offsets -r..+r, top clamps to row 0
        for i in 0..=(2 * r) {
            let sy = if i < r { 0 } else { (i - r).min(h - 1) };
            for c in 0..3 { sums[c] += src[sy * w * 4 + x * 4 + c] as u32; }
        }
        for y in 0..h {
            for c in 0..3 { dst[y * w * 4 + x * 4 + c] = (sums[c] / cnt) as u8; }
            dst[y * w * 4 + x * 4 + 3] = src[y * w * 4 + x * 4 + 3];
            let add_y = (y + r + 1).min(h - 1);
            let rem_y = y.saturating_sub(r);
            for c in 0..3 {
                sums[c] += src[add_y * w * 4 + x * 4 + c] as u32;
                sums[c] -= src[rem_y * w * 4 + x * 4 + c] as u32;
            }
        }
    }
}

fn apply_blur(pixels: &mut [u8], w: usize, h: usize, radius: usize) {
    let mut tmp = vec![0u8; pixels.len()];
    for _ in 0..2 {
        box_blur_h(pixels, &mut tmp, w, h, radius);
        box_blur_v(&tmp, pixels, w, h, radius);
    }
}

unsafe fn decode_thumbnail(bytes: &[u8], size: i32) -> Option<isize> {
    let stream = SHCreateMemStream(Some(bytes))?;
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let decoder = factory
        .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
        .ok()?;
    let frame = decoder.GetFrame(0).ok()?;

    // Fit within size×size while preserving aspect ratio
    let mut orig_w = 0u32;
    let mut orig_h = 0u32;
    frame.GetSize(&mut orig_w, &mut orig_h).ok()?;
    let scale = (size as f64 / orig_w as f64).min(size as f64 / orig_h as f64);
    let dst_w = ((orig_w as f64 * scale) as i32).max(1);
    let dst_h = ((orig_h as f64 * scale) as i32).max(1);
    let x_off = (size - dst_w) / 2;
    let y_off = (size - dst_h) / 2;

    let scaler = factory.CreateBitmapScaler().ok()?;
    scaler
        .Initialize(&frame, dst_w as u32, dst_h as u32, WICBitmapInterpolationModeFant)
        .ok()?;
    let converter = factory.CreateFormatConverter().ok()?;
    converter
        .Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .ok()?;

    // Allocate size×size DIB — zero-initialised (= black)
    let stride = (size * 4) as u32;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default()],
    };
    let mut bits = std::ptr::null_mut::<std::ffi::c_void>();
    let hbmp = CreateDIBSection(
        HDC(std::ptr::null_mut()), &bmi, DIB_RGB_COLORS, &mut bits, None, 0,
    ).ok()?;

    // Decode scaled pixels into a temporary buffer, then copy into the centred position
    let img_stride = (dst_w * 4) as u32;
    let mut img_buf = vec![0u8; (img_stride * dst_h as u32) as usize];
    converter.CopyPixels(std::ptr::null(), img_stride, &mut img_buf).ok()?;

    let dst_pixels = std::slice::from_raw_parts_mut(
        bits as *mut u8,
        (stride * size as u32) as usize,
    );
    for row in 0..dst_h as usize {
        let src = row * img_stride as usize;
        let dst = (y_off as usize + row) * stride as usize + x_off as usize * 4;
        dst_pixels[dst..dst + img_stride as usize]
            .copy_from_slice(&img_buf[src..src + img_stride as usize]);
    }

    Some(hbmp.0 as isize)
}

// ── drawing ───────────────────────────────────────────────────────────────────

unsafe fn draw_btn_group(
    dc: HDC, btn_top: i32, group_x: i32, hover: Hover, playing: bool, font: HFONT,
) {
    let gx  = group_x;
    let gy  = btn_top;
    let gx2 = gx + GROUP_W;
    let gy2 = gy + BTN_H;

    let saved = SaveDC(dc);
    let rgn = CreateRoundRectRgn(gx, gy, gx2 + 1, gy2 + 1, CORNER, CORNER);
    SelectClipRgn(dc, rgn);

    let bg_br = CreateSolidBrush(COLORREF(C_BTN));
    FillRect(dc, &RECT { left: gx, top: gy, right: gx2, bottom: gy2 }, bg_br);
    let _ = DeleteObject(bg_br);

    let hx_opt = match hover {
        Hover::Prev => Some(gx),
        Hover::Play => Some(gx + BTN_W),
        Hover::Next => Some(gx + BTN_W * 2),
        _           => None,
    };
    if let Some(hx) = hx_opt {
        let hov_br = CreateSolidBrush(COLORREF(C_BTN_HOV));
        FillRect(dc, &RECT { left: hx, top: gy, right: hx + BTN_W, bottom: gy2 }, hov_br);
        let _ = DeleteObject(hov_br);
    }

    let _ = RestoreDC(dc, saved);
    let _ = DeleteObject(rgn);

    let bdr = if hx_opt.is_some() { C_BTN_BDR_H } else { C_BTN_BDR };
    let pn = CreatePen(PS_SOLID, 1, COLORREF(bdr));
    let op = SelectObject(dc, pn);
    let ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
    let _ = RoundRect(dc, gx, gy, gx2, gy2, CORNER, CORNER);
    SelectObject(dc, op);
    SelectObject(dc, ob);
    let _ = DeleteObject(pn);

    let div_pn = CreatePen(PS_SOLID, 1, COLORREF(C_BTN_BDR));
    let op = SelectObject(dc, div_pn);
    let margin = 4;
    let _ = MoveToEx(dc, gx + BTN_W,     gy + margin, None);
    let _ = LineTo  (dc, gx + BTN_W,     gy2 - margin);
    let _ = MoveToEx(dc, gx + BTN_W * 2, gy + margin, None);
    let _ = LineTo  (dc, gx + BTN_W * 2, gy2 - margin);
    SelectObject(dc, op);
    let _ = DeleteObject(div_pn);

    let of = SelectObject(dc, font);
    SetTextColor(dc, COLORREF(C_ICON));
    SetBkMode(dc, TRANSPARENT);
    let icons = [
        (gx,              ICON_PREV),
        (gx + BTN_W,     if playing { ICON_PAUSE } else { ICON_PLAY }),
        (gx + BTN_W * 2, ICON_NEXT),
    ];
    for (ix, icon) in icons {
        let mut wide = w16(icon);
        let mut r = RECT { left: ix, top: gy, right: ix + BTN_W, bottom: gy2 };
        DrawTextW(dc, &mut wide, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }
    SelectObject(dc, of);
}

unsafe fn draw_gear_btn(dc: HDC, gx: i32, gy: i32, hovered: bool, font: HFONT) {
    let gx2 = gx + GEAR_W;
    let gy2 = gy + BTN_H;

    // Background — clipped to rounded rect
    let saved = SaveDC(dc);
    let rgn   = CreateRoundRectRgn(gx, gy, gx2 + 1, gy2 + 1, CORNER, CORNER);
    SelectClipRgn(dc, rgn);
    let bg_br = CreateSolidBrush(COLORREF(if hovered { C_BTN_HOV } else { C_BTN }));
    FillRect(dc, &RECT { left: gx, top: gy, right: gx2, bottom: gy2 }, bg_br);
    let _ = DeleteObject(bg_br);
    let _ = RestoreDC(dc, saved);
    let _ = DeleteObject(rgn);

    // Border
    let pn = CreatePen(PS_SOLID, 1, COLORREF(if hovered { C_BTN_BDR_H } else { C_BTN_BDR }));
    let op = SelectObject(dc, pn);
    let ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
    let _ = RoundRect(dc, gx, gy, gx2, gy2, CORNER, CORNER);
    SelectObject(dc, op);
    SelectObject(dc, ob);
    let _ = DeleteObject(pn);

    // Gear icon
    let of = SelectObject(dc, font);
    SetTextColor(dc, COLORREF(C_ICON));
    SetBkMode(dc, TRANSPARENT);
    let mut wide = w16(ICON_GEAR);
    let mut r = RECT { left: gx, top: gy, right: gx2, bottom: gy2 };
    DrawTextW(dc, &mut wide, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(dc, of);
}

unsafe fn draw_shr_group(dc: HDC, gx: i32, gy: i32, state: &ShrGroupState) {
    let ShrGroupState {
        hover, shuffle_active, shuffle_enabled, repeat_active, repeat_enabled,
        rep_icon, font, accent,
    } = *state;
    let total_w = SHR_W * 2;
    let gx2 = gx + total_w;
    let gy2 = gy + BTN_H;

    // Background — clipped to rounded rect
    let saved = SaveDC(dc);
    let rgn = CreateRoundRectRgn(gx, gy, gx2 + 1, gy2 + 1, CORNER, CORNER);
    SelectClipRgn(dc, rgn);
    let bg_br = CreateSolidBrush(COLORREF(C_BTN));
    FillRect(dc, &RECT { left: gx, top: gy, right: gx2, bottom: gy2 }, bg_br);
    let _ = DeleteObject(bg_br);

    // Hover highlight on the hovered half
    let hx_opt = match hover {
        Hover::Shuffle => Some(gx),
        Hover::Repeat  => Some(gx + SHR_W),
        _              => None,
    };
    if let Some(hx) = hx_opt {
        let hov_br = CreateSolidBrush(COLORREF(C_BTN_HOV));
        FillRect(dc, &RECT { left: hx, top: gy, right: hx + SHR_W, bottom: gy2 }, hov_br);
        let _ = DeleteObject(hov_br);
    }
    let _ = RestoreDC(dc, saved);
    let _ = DeleteObject(rgn);

    // Border
    let bdr = if hx_opt.is_some() { C_BTN_BDR_H } else { C_BTN_BDR };
    let pn = CreatePen(PS_SOLID, 1, COLORREF(bdr));
    let op = SelectObject(dc, pn);
    let ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
    let _ = RoundRect(dc, gx, gy, gx2, gy2, CORNER, CORNER);
    SelectObject(dc, op);
    SelectObject(dc, ob);
    let _ = DeleteObject(pn);

    // Divider
    let div_pn = CreatePen(PS_SOLID, 1, COLORREF(C_BTN_BDR));
    let op = SelectObject(dc, div_pn);
    let margin = 4;
    let _ = MoveToEx(dc, gx + SHR_W, gy + margin, None);
    let _ = LineTo  (dc, gx + SHR_W, gy2 - margin);
    SelectObject(dc, op);
    let _ = DeleteObject(div_pn);

    // Icons — accent color when active, dim when not enabled, normal otherwise
    let icons: [(&str, bool, bool); 2] = [
        (ICON_SHUFFLE, shuffle_active, shuffle_enabled),
        (rep_icon,     repeat_active,  repeat_enabled),
    ];
    let of = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT);
    for (i, (icon, active, enabled)) in icons.iter().enumerate() {
        let ix = gx + i as i32 * SHR_W;
        let icon_color = if !enabled { 0x00_55_55_55u32 }
                         else if *active { accent }
                         else { C_ICON };
        SetTextColor(dc, COLORREF(icon_color));
        let mut wide = w16(icon);
        let mut r = RECT { left: ix, top: gy, right: ix + SHR_W, bottom: gy2 };
        DrawTextW(dc, &mut wide, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }
    SelectObject(dc, of);
}

unsafe fn draw_media_text(dc: HDC, info: &MediaInfo, font: HFONT, x: i32, max_x: i32, h: i32) {
    let of = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT);
    let avail = max_x - x - SIDE_PAD;
    let top = ACCENT_H;
    if avail <= 0 { SelectObject(dc, of); return; }

    if info.title.is_empty() {
        SetTextColor(dc, COLORREF(C_ARTIST));
        let mut w = w16("No media playing");
        let mut r = RECT { left: x, top, right: x + avail, bottom: h };
        DrawTextW(dc, &mut w, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        SelectObject(dc, of);
        return;
    }

    let title_u16: Vec<u16> = info.title.encode_utf16().collect();
    let mut tsz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &title_u16, &mut tsz);
    let title_alloc = tsz.cx.min(avail * 55 / 100);
    let needs_ellipsis = tsz.cx > title_alloc;

    SetTextColor(dc, COLORREF(C_TITLE));
    let mut tw = title_u16.clone();
    let mut tr = RECT { left: x, top, right: x + title_alloc, bottom: h };
    let tf = DT_LEFT | DT_VCENTER | DT_SINGLELINE;
    DrawTextW(dc, &mut tw, &mut tr, if needs_ellipsis { tf | DT_END_ELLIPSIS } else { tf });

    if info.artist.is_empty() { SelectObject(dc, of); return; }

    let mut sep = w16("-");
    let mut ssz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &sep, &mut ssz);
    let sep_x = x + title_alloc + 6;
    if sep_x + ssz.cx + 6 >= x + avail { SelectObject(dc, of); return; }

    SetTextColor(dc, COLORREF(C_SEP));
    let mut sr = RECT { left: sep_x, top, right: sep_x + ssz.cx, bottom: h };
    DrawTextW(dc, &mut sep, &mut sr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

    let art_x = sep_x + ssz.cx + 6;
    let art_w = x + avail - art_x;
    if art_w > 20 {
        SetTextColor(dc, COLORREF(C_ARTIST));
        let mut aw = w16(&info.artist);
        let mut ar = RECT { left: art_x, top, right: art_x + art_w, bottom: h };
        DrawTextW(dc, &mut aw, &mut ar, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    }
    SelectObject(dc, of);
}

unsafe fn on_paint(hwnd: HWND, state: &AppState) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);

    // Layout derived from the actual window height
    let art_w   = h;
    let group_x = h + SIDE_PAD;
    let shr_x   = group_x + GROUP_W + SHR_GAP;
    let text_x  = shr_x + SHR_W * 2 + 20;

    let mdc = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mdc, bmp);

    let info  = state.info.lock().unwrap().clone();
    let hover = *state.hover.lock().unwrap();

    // Album art — also drives the blurred bar background
    let art_top = ACCENT_H;
    let art_h   = h - ACCENT_H;
    {
        let mut cache = state.art.lock().unwrap();
        if cache.as_ref().map(|c| c.key.as_str()) != Some(info.title.as_str()) {
            if let Some(old) = cache.take() {
                let _ = DeleteObject(HBITMAP(old.hbmp    as *mut _));
                let _ = DeleteObject(HBITMAP(old.hbmp_bg as *mut _));
            }
            if !info.title.is_empty() {
                if let Some(ref bytes) = info.thumbnail {
                    if let (Some(hbmp), Some(hbmp_bg)) = (
                        decode_thumbnail(bytes, art_w),
                        decode_thumbnail(bytes, BG_ART_SIZE),
                    ) {
                        *cache = Some(CachedArt { key: info.title.clone(), hbmp, hbmp_bg });
                    }
                }
            }
        }

        if let Some(ref cached) = *cache {
            // ── blurred background ────────────────────────────────────────────
            // 1. StretchBlt the center crop of hbmp_bg into a full-bar-size DIB.
            // 2. Apply a software separable box blur directly on the pixel data.
            // This gives a real blur at full resolution — no downscale artefacts.
            let src_dc = CreateCompatibleDC(mdc);
            let old_src = SelectObject(src_dc, HBITMAP(cached.hbmp_bg as *mut _));

            let bg_sz   = BG_ART_SIZE;
            let src_h_f = bg_sz as f64 * h as f64 / w as f64;
            let src_h_i = (src_h_f.ceil() as i32).max(1).min(bg_sz);
            let src_y   = ((bg_sz - src_h_i) / 2).max(0);

            let mut bg_bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bg_bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize:        std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth:       w,
                    biHeight:      -h, // top-down
                    biPlanes:      1,
                    biBitCount:    32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                bmiColors: [RGBQUAD::default()],
            };
            if let Ok(bg_dib) = CreateDIBSection(
                HDC(std::ptr::null_mut()), &bg_bmi, DIB_RGB_COLORS, &mut bg_bits, None, 0,
            ) {
                let bg_dc  = CreateCompatibleDC(mdc);
                let old_bg = SelectObject(bg_dc, bg_dib);
                SetStretchBltMode(bg_dc, HALFTONE);
                let _ = StretchBlt(bg_dc, 0, 0, w, h,
                                   src_dc, 0, src_y, bg_sz, src_h_i, SRCCOPY);

                // Apply blur directly on the DIB pixel buffer
                let pixel_count = (w * h * 4) as usize;
                let pixels = std::slice::from_raw_parts_mut(bg_bits as *mut u8, pixel_count);
                apply_blur(pixels, w as usize, h as usize, 12);

                // Copy blurred result to the back buffer
                let _ = BitBlt(mdc, 0, 0, w, h, bg_dc, 0, 0, SRCCOPY);

                SelectObject(bg_dc, old_bg);
                let _ = DeleteDC(bg_dc);
                let _ = DeleteObject(bg_dib);
            }
            SelectObject(src_dc, old_src);
            let _ = DeleteDC(src_dc);

            // ── dark overlay ──────────────────────────────────────────────────
            let ov_dc  = CreateCompatibleDC(mdc);
            let ov_bmp = CreateCompatibleBitmap(mdc, 1, 1);
            let old_ov = SelectObject(ov_dc, ov_bmp);
            let blk_br = CreateSolidBrush(COLORREF(0));
            FillRect(ov_dc, &RECT { left: 0, top: 0, right: 1, bottom: 1 }, blk_br);
            let _ = DeleteObject(blk_br);
            let bf = BLENDFUNCTION {
                BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 170, AlphaFormat: 0,
            };
            let _ = AlphaBlend(mdc, 0, 0, w, h, ov_dc, 0, 0, 1, 1, bf);
            SelectObject(ov_dc, old_ov);
            let _ = DeleteObject(ov_bmp);
            let _ = DeleteDC(ov_dc);

            // ── album art thumbnail (small, crisp, on top of blurred BG) ─────
            let art_dc = CreateCompatibleDC(mdc);
            let old_a  = SelectObject(art_dc, HBITMAP(cached.hbmp as *mut _));
            let _ = BitBlt(mdc, 0, art_top, art_w, art_h, art_dc, 0, 0, SRCCOPY);
            SelectObject(art_dc, old_a);
            let _ = DeleteDC(art_dc);
        } else {
            // No art: solid dark background + placeholder square
            let bg = CreateSolidBrush(COLORREF(C_BG));
            FillRect(mdc, &rc, bg);
            let _ = DeleteObject(bg);
            let ph_br = CreateSolidBrush(COLORREF(C_ART_PH));
            FillRect(mdc, &RECT { left: 0, top: art_top, right: art_w, bottom: h }, ph_br);
            let _ = DeleteObject(ph_br);
        }
    }

    let btn_top  = ACCENT_H + (h - ACCENT_H - BTN_H) / 2;
    let font_btn = HFONT(state.font_btn.load(Ordering::Relaxed) as *mut _);
    let accent   = state.accent.load(Ordering::Relaxed);
    draw_btn_group(mdc, btn_top, group_x, hover, info.playing, font_btn);

    // Shuffle/repeat group — grouped button pair to the right of the play group
    {
        use windows::Media::MediaPlaybackAutoRepeatMode as Mode;
        let rep_icon    = if info.repeat == Some(Mode::Track) { ICON_REPEAT_ONE } else { ICON_REPEAT_ALL };
        let repeat_active = info.repeat.map(|r| r != Mode::None).unwrap_or(false);
        let group_state = ShrGroupState {
            hover,
            shuffle_active: info.shuffle,
            shuffle_enabled: info.shuffle_enabled,
            repeat_active,
            repeat_enabled: info.repeat_enabled,
            rep_icon,
            font: font_btn,
            accent,
        };
        draw_shr_group(mdc, shr_x, btn_top, &group_state);
    }

    // Gear button sits at the far right; everything else is pushed left of it.
    let gear_x   = w - GEAR_PAD - GEAR_W;
    let has_seek = info.duration_100ns > 0;
    let text_max_x = if has_seek {
        gear_x - SEEK_PAD_R - SEEK_W - TIME_GAP - TIME_W - TIME_GAP
    } else {
        gear_x
    };
    let font = HFONT(state.font.load(Ordering::Relaxed) as *mut _);
    draw_media_text(mdc, &info, font, text_x, text_max_x, h);

    if has_seek {
        let sx      = gear_x - SEEK_PAD_R - SEEK_W;
        let seek_cy = ACCENT_H + (h - ACCENT_H) / 2;
        let sy      = seek_cy - SEEK_H / 2;

        let elapsed_100ns = if info.playing {
            state.pos_sampled.lock().unwrap().elapsed().as_nanos() as i64 / 100
        } else { 0 };
        let eff         = state.effective_pos.load(Ordering::Relaxed);
        let display_pos = (eff + elapsed_100ns).min(info.duration_100ns);

        let time_rx = sx - TIME_GAP;
        let time_lx = time_rx - TIME_W;
        let of = SelectObject(mdc, font);
        SetTextColor(mdc, COLORREF(C_ARTIST));
        SetBkMode(mdc, TRANSPARENT);
        let time_str = format!("{} / {}", fmt_time(display_pos), fmt_time(info.duration_100ns));
        let mut tw = w16(&time_str);
        let mut tr = RECT { left: time_lx, top: ACCENT_H, right: time_rx, bottom: h };
        DrawTextW(mdc, &mut tw, &mut tr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
        SelectObject(mdc, of);

        let track_br = CreateSolidBrush(COLORREF(C_BTN_BDR));
        let track_rc = RECT { left: sx, top: sy, right: sx + SEEK_W, bottom: sy + SEEK_H };
        FillRect(mdc, &track_rc, track_br);
        let _ = DeleteObject(track_br);

        let frac = state.seeking.lock().unwrap()
            .unwrap_or(display_pos as f64 / info.duration_100ns as f64);
        let fill_w = (SEEK_W as f64 * frac) as i32;
        if fill_w > 0 {
            let fill_br = CreateSolidBrush(COLORREF(accent));
            let fill_rc = RECT { left: sx, top: sy, right: sx + fill_w, bottom: sy + SEEK_H };
            FillRect(mdc, &fill_rc, fill_br);
            let _ = DeleteObject(fill_br);
        }

        let thumb_cx = sx + fill_w;
        let thumb_r  = 5i32;
        let scale    = 3i32;
        let dst_x = thumb_cx - thumb_r;
        let dst_y = seek_cy  - thumb_r;
        let dst_d = thumb_r * 2;
        let src_d = dst_d * scale;

        let tmp_dc  = CreateCompatibleDC(mdc);
        let tmp_bmp = CreateCompatibleBitmap(mdc, src_d, src_d);
        let old_tmp = SelectObject(tmp_dc, tmp_bmp);

        SetStretchBltMode(tmp_dc, HALFTONE);
        let _ = StretchBlt(tmp_dc, 0, 0, src_d, src_d, mdc, dst_x, dst_y, dst_d, dst_d, SRCCOPY);

        let thumb_br = CreateSolidBrush(COLORREF(C_TITLE));
        let ob = SelectObject(tmp_dc, thumb_br);
        let op = SelectObject(tmp_dc, GetStockObject(NULL_PEN));
        let _ = Ellipse(tmp_dc, 0, 0, src_d + 1, src_d + 1);
        SelectObject(tmp_dc, ob);
        SelectObject(tmp_dc, op);
        let _ = DeleteObject(thumb_br);

        SetStretchBltMode(mdc, HALFTONE);
        let _ = StretchBlt(mdc, dst_x, dst_y, dst_d, dst_d, tmp_dc, 0, 0, src_d, src_d, SRCCOPY);

        SelectObject(tmp_dc, old_tmp);
        let _ = DeleteObject(tmp_bmp);
        let _ = DeleteDC(tmp_dc);
    }

    // Gear button — always visible, vertically centred with the control buttons
    let gear_top = ACCENT_H + (h - ACCENT_H - BTN_H) / 2;
    draw_gear_btn(mdc, gear_x, gear_top, hover == Hover::Gear, font_btn);

    // Accent border — drawn last so it always renders on top
    let acc_rc = RECT { left: 0, top: 0, right: w, bottom: ACCENT_H };
    let acc = CreateSolidBrush(COLORREF(accent));
    FillRect(mdc, &acc_rc, acc);
    let _ = DeleteObject(acc);

    let _ = BitBlt(hdc, 0, 0, w, h, mdc, 0, 0, SRCCOPY);
    SelectObject(mdc, old_bmp);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mdc);
    let _ = EndPaint(hwnd, &ps);
}

// ── settings window ───────────────────────────────────────────────────────────
static SETTINGS_HWND: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

unsafe fn apply_settings(hwnd_main: HWND, state: &AppState) {
    let (bar_h, font_face, font_size, monitor_idx) = {
        let s = state.settings.lock().unwrap();
        (s.bar_h, s.font_face.clone(), s.font_size, s.monitor_idx as usize)
    };

    let face_wide: Vec<u16> = font_face.encode_utf16().chain([0u16]).collect();
    let new_font     = make_font(PCWSTR(face_wide.as_ptr()), font_size);
    let new_font_btn = make_font(w!("Segoe MDL2 Assets"), 14);

    let old_font     = state.font.swap(new_font.0 as usize, Ordering::Relaxed);
    let old_font_btn = state.font_btn.swap(new_font_btn.0 as usize, Ordering::Relaxed);
    let _ = DeleteObject(HFONT(old_font as *mut _));
    let _ = DeleteObject(HFONT(old_font_btn as *mut _));

    // Art cache must be invalidated: hbmp size depends on bar_h
    if let Some(old) = state.art.lock().unwrap().take() {
        let _ = DeleteObject(HBITMAP(old.hbmp    as *mut _));
        let _ = DeleteObject(HBITMAP(old.hbmp_bg as *mut _));
    }

    appbar_remove(hwnd_main);
    let monitors = enum_monitors();
    let mon_rect = if monitors.is_empty() {
        RECT {
            left:   0,
            top:    0,
            right:  GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    } else {
        monitors[monitor_idx.min(monitors.len() - 1)].rect
    };
    let rc = appbar_register(hwnd_main, mon_rect, bar_h);
    let _ = SetWindowPos(
        hwnd_main, HWND_TOP,
        rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
        SWP_SHOWWINDOW,
    );
    let _ = InvalidateRect(hwnd_main, None, true);
}

unsafe fn open_settings(hwnd_main: HWND, state_ptr: *mut AppState) {
    let existing = SETTINGS_HWND.load(Ordering::Relaxed);
    if existing != 0 {
        let _ = SetForegroundWindow(HWND(existing as *mut _));
        return;
    }
    let hi: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let ctx = Box::into_raw(Box::new(SettingsCtx {
        hwnd_main: hwnd_main.0 as isize,
        state_ptr,
    }));
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let ww = 376;
    let wh = 204;
    match CreateWindowExW(
        WS_EX_DLGMODALFRAME | WS_EX_APPWINDOW,
        w!("MediaBarSettings"),
        w!("MediaBar 設定"),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        (sw - ww) / 2, (sh - wh) / 2, ww, wh,
        hwnd_main, None, hi, Some(ctx as *mut _),
    ) {
        Ok(hw) => {
            SETTINGS_HWND.store(hw.0 as isize, Ordering::Relaxed);
            let _ = ShowWindow(hw, SW_SHOW);
        }
        Err(_) => { let _ = Box::from_raw(ctx); }
    }
}

// Callback for EnumFontFamiliesExW — appends each unique face name to a Vec<String>.
unsafe extern "system" fn enum_font_proc(
    lpelfe: *const LOGFONTW,
    _lpntme: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    if lpelfe.is_null() { return 1; }
    let lf = &*lpelfe;
    // Skip @-prefixed vertical-layout variants
    if lf.lfFaceName[0] == b'@' as u16 { return 1; }
    let end = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
    if end == 0 { return 1; }
    let name = String::from_utf16_lossy(&lf.lfFaceName[..end]);
    let list = &mut *(lparam.0 as *mut Vec<String>);
    list.push(name);
    1 // continue enumeration
}

unsafe extern "system" fn settings_wndproc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs  = &*(lp.0 as *const CREATESTRUCTW);
            let ctx = cs.lpCreateParams as *mut SettingsCtx;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx as isize);

            let hi: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let state = &*(*ctx).state_ptr;
            let (bar_h, font_face, font_size) = {
                let s = state.settings.lock().unwrap();
                (s.bar_h, s.font_face.clone(), s.font_size)
            };

            let gui_font = GetStockObject(DEFAULT_GUI_FONT);

            // Helper — creates a child control, sets the system GUI font on it.
            // ex: WS_EX_CLIENTEDGE (0x200) for edits; 0 for everything else.
            let mk = |ex: u32, class: PCWSTR, text: PCWSTR, style: u32,
                      x: i32, y: i32, cw: i32, ch: i32, id: i32| -> HWND {
                let hw = CreateWindowExW(
                    WINDOW_EX_STYLE(ex), class, text,
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                    x, y, cw, ch,
                    hwnd, HMENU(id as usize as *mut _), hi, None,
                ).unwrap_or(HWND(std::ptr::null_mut()));
                SendMessageW(hw, WM_SETFONT, WPARAM(gui_font.0 as usize), LPARAM(1));
                hw
            };

            // ── layout ────────────────────────────────────────────────────────
            // Client area: ~356 × 162 px
            // Columns:  label x=20 w=112 | control x=140 w=196
            // Rows:     y=20 (barh), y=55 (font), y=91 (size)
            // Buttons:  y=129, right-aligned to x=336
            let (lx, cx, cw, rh) = (20i32, 140i32, 196i32, 22i32);

            mk(0, w!("STATIC"), w!("バーの高さ"),    0, lx, 22, 112, rh, 0);
            mk(0, w!("STATIC"), w!("px"),              0, cx+68, 22, 28, rh, 0);
            mk(0, w!("STATIC"), w!("フォント"),       0, lx, 58, 112, rh, 0);
            mk(0, w!("STATIC"), w!("フォントサイズ"), 0, lx, 94, 112, rh, 0);
            mk(0, w!("STATIC"), w!("px"),              0, cx+68, 94, 28, rh, 0);

            // Edits — WS_EX_CLIENTEDGE gives the modern sunken look
            let barh_edit = mk(0x200, w!("EDIT"), w!(""),
                WS_TABSTOP.0, cx, 20, 60, rh, IDC_BARH_EDIT);
            let size_edit = mk(0x200, w!("EDIT"), w!(""),
                WS_TABSTOP.0, cx, 92, 60, rh, IDC_FONTSIZE_EDIT);

            // Font combobox — CBS_DROPDOWNLIST=0x3, WS_TABSTOP, height=200 for dropdown
            let font_combo = mk(0, w!("COMBOBOX"), w!(""),
                0x0003 | WS_TABSTOP.0, cx, 55, cw, 200, IDC_FONT_COMBO);

            // Buttons — right-aligned, OK on the far right
            let (bw, by, bh) = (72i32, 129i32, 26i32);
            let br = cx + cw; // right edge = 336
            mk(0, w!("BUTTON"), w!("OK"),         WS_TABSTOP.0 | 1, br - bw,      by, bw, bh, 1);
            mk(0, w!("BUTTON"), w!("キャンセル"), WS_TABSTOP.0,     br - bw*2 - 8, by, bw, bh, 2);

            // ── populate values ───────────────────────────────────────────────
            let t: Vec<u16> = bar_h.to_string().encode_utf16().chain([0u16]).collect();
            let _ = SetWindowTextW(barh_edit, PCWSTR(t.as_ptr()));
            let t: Vec<u16> = font_size.to_string().encode_utf16().chain([0u16]).collect();
            let _ = SetWindowTextW(size_edit, PCWSTR(t.as_ptr()));

            // Enumerate all installed font families → fill combobox
            let hdc = GetDC(hwnd);
            let mut font_list: Vec<String> = Vec::new();
            let logfont = LOGFONTW { lfCharSet: DEFAULT_CHARSET, ..Default::default() };
            EnumFontFamiliesExW(
                hdc, &logfont, Some(enum_font_proc),
                LPARAM(&mut font_list as *mut Vec<String> as isize), 0,
            );
            ReleaseDC(hwnd, hdc);
            font_list.sort_unstable();
            font_list.dedup();
            for name in &font_list {
                let wide: Vec<u16> = name.encode_utf16().chain([0u16]).collect();
                SendMessageW(font_combo, 0x0143 /*CB_ADDSTRING*/, WPARAM(0),
                    LPARAM(wide.as_ptr() as isize));
            }
            // Select the currently configured font face (exact match)
            let cur: Vec<u16> = font_face.encode_utf16().chain([0u16]).collect();
            let idx = SendMessageW(font_combo, 0x0158 /*CB_FINDSTRINGEXACT*/,
                WPARAM(usize::MAX), LPARAM(cur.as_ptr() as isize));
            if idx.0 >= 0 {
                SendMessageW(font_combo, 0x014E /*CB_SETCURSEL*/,
                    WPARAM(idx.0 as usize), LPARAM(0));
            }

            LRESULT(0)
        }
        WM_COMMAND => {
            let id  = (wp.0 & 0xFFFF) as i32;
            let ctx = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsCtx;
            if ctx.is_null() { return DefWindowProcW(hwnd, msg, wp, lp); }

            match id {
                1 => { // IDOK — read, validate, apply
                    let mut buf = [0u16; 256];

                    let barh_edit  = GetDlgItem(hwnd, IDC_BARH_EDIT)
                        .unwrap_or(HWND(std::ptr::null_mut()));
                    let font_combo = GetDlgItem(hwnd, IDC_FONT_COMBO)
                        .unwrap_or(HWND(std::ptr::null_mut()));
                    let size_edit  = GetDlgItem(hwnd, IDC_FONTSIZE_EDIT)
                        .unwrap_or(HWND(std::ptr::null_mut()));

                    let bar_h = {
                        let n = GetWindowTextW(barh_edit, &mut buf) as usize;
                        String::from_utf16_lossy(&buf[..n])
                            .trim().parse::<i32>()
                            .unwrap_or(DEFAULT_BAR_H).clamp(40, 100)
                    };
                    // Read selected font from the combobox
                    let font_face = {
                        let idx = SendMessageW(font_combo,
                            0x0147 /*CB_GETCURSEL*/, WPARAM(0), LPARAM(0));
                        if idx.0 >= 0 {
                            let len = SendMessageW(font_combo,
                                0x0149 /*CB_GETLBTEXTLEN*/,
                                WPARAM(idx.0 as usize), LPARAM(0));
                            if len.0 > 0 {
                                let mut fbuf = vec![0u16; len.0 as usize + 1];
                                SendMessageW(font_combo, 0x0148 /*CB_GETLBTEXT*/,
                                    WPARAM(idx.0 as usize),
                                    LPARAM(fbuf.as_mut_ptr() as isize));
                                String::from_utf16_lossy(&fbuf[..len.0 as usize])
                            } else { DEFAULT_FONT_FACE.to_string() }
                        } else { DEFAULT_FONT_FACE.to_string() }
                    };
                    let font_size = {
                        let n = GetWindowTextW(size_edit, &mut buf) as usize;
                        String::from_utf16_lossy(&buf[..n])
                            .trim().parse::<i32>()
                            .unwrap_or(DEFAULT_FONT_SIZE).clamp(8, 32)
                    };

                    {
                        let mut s = (*(*ctx).state_ptr).settings.lock().unwrap();
                        s.bar_h     = bar_h;
                        s.font_face = font_face;
                        s.font_size = font_size;
                        save_settings(&s);
                    }

                    apply_settings(
                        HWND((*ctx).hwnd_main as *mut _),
                        &*(*ctx).state_ptr,
                    );
                    let _ = DestroyWindow(hwnd);
                }
                2 => { let _ = DestroyWindow(hwnd); } // IDCANCEL
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ctx = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsCtx;
            if !ctx.is_null() { let _ = Box::from_raw(ctx); }
            SETTINGS_HWND.store(0, Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ── window procedure ──────────────────────────────────────────────────────────
static STATE_PTR: std::sync::atomic::AtomicPtr<AppState> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let sp = STATE_PTR.load(std::sync::atomic::Ordering::Relaxed);
    if msg == TASKBAR_CREATED.load(Ordering::Relaxed) && msg != 0 {
        tray_icon_add(hwnd);
        return LRESULT(0);
    }
    match msg {
        WM_PAINT => {
            if !sp.is_null() { on_paint(hwnd, &*sp); }
            LRESULT(0)
        }
        WM_MEDIA_UPDATE => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !sp.is_null() {
                let x = (lp.0 & 0xFFFF) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let group_x = rc.bottom + SIDE_PAD;
                let shr_x   = group_x + GROUP_W + SHR_GAP;
                let rep_x   = shr_x + SHR_W;
                let gear_x  = rc.right - GEAR_PAD - GEAR_W;

                let dragging = {
                    let mut drag = (*sp).seeking.lock().unwrap();
                    if drag.is_some() {
                        let sx = gear_x - SEEK_PAD_R - SEEK_W;
                        *drag = Some(((x - sx) as f64 / SEEK_W as f64).clamp(0.0, 1.0));
                        true
                    } else {
                        false
                    }
                };
                if dragging {
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    let new_hover = hit_btn(x, group_x, shr_x, rep_x, gear_x);
                    let mut hov = (*sp).hover.lock().unwrap();
                    if *hov != new_hover {
                        *hov = new_hover;
                        drop(hov);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
            }
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
            LRESULT(0)
        }
        WM_MOUSE_LEAVE => {
            if !sp.is_null() {
                *(*sp).hover.lock().unwrap() = Hover::None;
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if !sp.is_null() {
                let frac = (*sp).seeking.lock().unwrap().take();
                let _ = ReleaseCapture();
                if let Some(f) = frac {
                    let dur = (*sp).info.lock().unwrap().duration_100ns;
                    if dur > 0 {
                        seek_to((dur as f64 * f) as i64, hwnd.0 as isize);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x  = (lp.0 & 0xFFFF) as i16 as i32;
            let hw = hwnd.0 as isize;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let gear_x  = rc.right - GEAR_PAD - GEAR_W;
            let sx      = gear_x - SEEK_PAD_R - SEEK_W;
            let group_x = rc.bottom + SIDE_PAD;
            let shr_x   = group_x + GROUP_W + SHR_GAP;
            let rep_x   = shr_x + SHR_W;

            if x >= sx && x < sx + SEEK_W && !sp.is_null() {
                let dur = (*sp).info.lock().unwrap().duration_100ns;
                if dur > 0 {
                    let frac = ((x - sx) as f64 / SEEK_W as f64).clamp(0.0, 1.0);
                    *(*sp).seeking.lock().unwrap() = Some(frac);
                    let _ = SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            } else {
                match hit_btn(x, group_x, shr_x, rep_x, gear_x) {
                    Hover::Prev    => send_cmd("prev", hw),
                    Hover::Play    => send_cmd("play", hw),
                    Hover::Next    => send_cmd("next", hw),
                    Hover::Shuffle => send_cmd("shuffle", hw),
                    Hover::Repeat  => send_cmd("repeat", hw),
                    Hover::Gear    => { if !sp.is_null() { open_settings(hwnd, sp); } }
                    Hover::None    => {}
                }
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if !sp.is_null() {
                (*sp).accent.store(get_accent_color(), Ordering::Relaxed);
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_TRAYICON => {
            match lp.0 as u32 {
                // Right-click or context-menu key → show menu
                w if w == WM_RBUTTONUP || w == WM_CONTEXTMENU => {
                    show_context_menu(hwnd);
                }
                // Double-click → open settings
                w if w == WM_LBUTTONDBLCLK => {
                    if !sp.is_null() { open_settings(hwnd, sp); }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, 1);
            tray_icon_remove(hwnd);
            appbar_remove(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ── entry point ───────────────────────────────────────────────────────────────
fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        if FindWindowW(w!("MediaBarClass"), PCWSTR(std::ptr::null())).is_ok() {
            return Ok(());
        }

        let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();

        // Register main window class
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
            lpszClassName: w!("MediaBarClass"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        // Register settings window class
        let sc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(settings_wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_3DFACE.0 + 1) as *mut _),
            lpszClassName: w!("MediaBarSettings"),
            ..Default::default()
        };
        RegisterClassExW(&sc);

        let settings    = load_settings();
        let bar_h       = settings.bar_h;
        let monitors    = enum_monitors();
        let mon_idx     = (settings.monitor_idx as usize).min(monitors.len().saturating_sub(1));
        let mon_rect    = if monitors.is_empty() {
            RECT {
                left:   0,
                top:    0,
                right:  GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        } else {
            monitors[mon_idx].rect
        };

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("MediaBarClass"),
            w!("MediaBar"),
            WS_POPUP | WS_VISIBLE,
            mon_rect.left, mon_rect.bottom - bar_h,
            mon_rect.right - mon_rect.left, bar_h,
            None, None, hinstance, None,
        )?;

        tray_icon_add(hwnd);

        let rc = appbar_register(hwnd, mon_rect, bar_h);
        SetWindowPos(
            hwnd, HWND_TOP,
            rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
            SWP_SHOWWINDOW,
        )?;
        SetTimer(hwnd, 1, 500, None);

        let face_wide: Vec<u16> = settings.font_face.encode_utf16().chain([0u16]).collect();
        let font     = make_font(PCWSTR(face_wide.as_ptr()), settings.font_size);
        let font_btn = make_font(w!("Segoe MDL2 Assets"), 14);

        let state = Box::new(AppState {
            info:          Mutex::new(MediaInfo::default()),
            hover:         Mutex::new(Hover::None),
            accent:        AtomicU32::new(get_accent_color()),
            art:           Mutex::new(None),
            seeking:       Mutex::new(None),
            effective_pos: AtomicI64::new(0),
            pos_sampled:   Mutex::new(std::time::Instant::now()),
            last_smtc_pos: AtomicI64::new(i64::MIN),
            font:          AtomicUsize::new(font.0 as usize),
            font_btn:      AtomicUsize::new(font_btn.0 as usize),
            settings:      Mutex::new(settings),
        });
        STATE_PTR.store(Box::into_raw(state), std::sync::atomic::Ordering::Relaxed);

        let hwnd_raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            loop {
                let (prev_title, prev_thumb) = {
                    let info = (*STATE_PTR.load(Ordering::Relaxed)).info.lock().unwrap();
                    (info.title.clone(), info.thumbnail.clone())
                };
                let fetched = fetch_media_info(&prev_title, prev_thumb);
                let ptr = STATE_PTR.load(Ordering::Relaxed);
                if ptr.is_null() { break; }
                if let Some(ref ni) = fetched {
                    if ni.duration_100ns > 0 {
                        let last = (*ptr).last_smtc_pos.load(Ordering::Relaxed);
                        let eff  = (*ptr).effective_pos.load(Ordering::Relaxed);
                        let interp_end = {
                            let playing = (*ptr).info.lock().unwrap().playing;
                            let elapsed = if playing {
                                (*ptr).pos_sampled.lock().unwrap().elapsed().as_nanos() as i64 / 100
                            } else { 0 };
                            (eff + elapsed).min(ni.duration_100ns)
                        };
                        let near_end = interp_end >= ni.duration_100ns - 5_000_000;
                        if ni.title != prev_title
                            || ni.position_100ns != last
                            || near_end
                        {
                            (*ptr).last_smtc_pos.store(ni.position_100ns, Ordering::Relaxed);
                            (*ptr).effective_pos.store(ni.position_100ns, Ordering::Relaxed);
                            *(*ptr).pos_sampled.lock().unwrap() = std::time::Instant::now();
                        }
                    }
                }
                (*ptr).info.lock().unwrap().clone_from(&fetched.unwrap_or_default());
                let _ = PostMessageW(
                    HWND(hwnd_raw as *mut _),
                    WM_MEDIA_UPDATE, WPARAM(0), LPARAM(0),
                );
                std::thread::sleep(std::time::Duration::from_millis(REFRESH_MS));
            }
            CoUninitialize();
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let sh_raw = SETTINGS_HWND.load(Ordering::Relaxed);
            if sh_raw != 0 {
                let sh_hwnd = HWND(sh_raw as *mut _);
                // Forward WM_MOUSEWHEEL to whatever window is under the cursor so the
                // font combobox dropdown scrolls even when it doesn't have focus.
                if msg.message == 0x020A /*WM_MOUSEWHEEL*/ {
                    let lp32 = msg.lParam.0 as i32;
                    let pt   = POINT {
                        x: (lp32 & 0xFFFF) as i16 as i32,
                        y: (lp32 >> 16)    as i16 as i32,
                    };
                    let target = WindowFromPoint(pt);
                    if !target.0.is_null() {
                        SendMessageW(target, msg.message, msg.wParam, msg.lParam);
                        continue;
                    }
                }
                // Route Tab / Enter / Escape to the settings window.
                if IsDialogMessageW(sh_hwnd, &msg).as_bool() { continue; }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let ptr = STATE_PTR.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
        if !ptr.is_null() {
            let s = Box::from_raw(ptr);
            if let Some(art) = s.art.lock().unwrap().take() {
                let _ = DeleteObject(HBITMAP(art.hbmp    as *mut _));
                let _ = DeleteObject(HBITMAP(art.hbmp_bg as *mut _));
            }
            let _ = DeleteObject(HFONT(s.font.load(Ordering::Relaxed) as *mut _));
            let _ = DeleteObject(HFONT(s.font_btn.load(Ordering::Relaxed) as *mut _));
        }

        CoUninitialize();
    }
    Ok(())
}
