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
const SIDE_PAD: i32 = 8;
const CORNER:   i32 = 6;

const SEEK_W:     i32 = 360;
const SEEK_H:     i32 = 3;
const SEEK_PAD_R: i32 = 12;
const TIME_W:     i32 = 82;
const TIME_GAP:   i32 = 16;

// Dynamic layout (computed from h = bar_h at runtime):
//   art_w   = h
//   group_x = h + SIDE_PAD
//   text_x  = group_x + GROUP_W + 14

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

// ── messages ──────────────────────────────────────────────────────────────────
const WM_APPBAR:       u32 = WM_APP + 1;
const WM_MEDIA_UPDATE: u32 = WM_APP + 2;
const WM_MOUSE_LEAVE:  u32 = 0x02A3;
const REFRESH_MS:      u64 = 1000;

// ── settings window control IDs ───────────────────────────────────────────────
const IDC_BARH_EDIT:     i32 = 101;
const IDC_FONT_EDIT:     i32 = 102;
const IDC_FONTSIZE_EDIT: i32 = 103;

// ── registry keys ─────────────────────────────────────────────────────────────
const STARTUP_KEY:  PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const STARTUP_NAME: PCWSTR = w!("MediaBar");
const SETTINGS_KEY: PCWSTR = w!("Software\\MediaBar");

// ── types ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Default)]
enum Hover { #[default] None, Prev, Play, Next }

#[derive(Clone, Default)]
struct MediaInfo {
    title:          String,
    artist:         String,
    playing:        bool,
    thumbnail:      Option<Arc<Vec<u8>>>,
    position_100ns: i64,
    duration_100ns: i64,
}

struct CachedArt {
    key:  String,
    hbmp: isize,
}

struct Settings {
    bar_h:     i32,
    font_face: String,
    font_size: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_h:     DEFAULT_BAR_H,
            font_face: DEFAULT_FONT_FACE.to_string(),
            font_size: DEFAULT_FONT_SIZE,
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

unsafe fn show_context_menu(hwnd: HWND) {
    let registered = is_startup_registered();
    let menu = CreatePopupMenu().unwrap();

    let _ = AppendMenuW(menu, MF_STRING, 3, w!("設定 (&P)"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(std::ptr::null()));
    let _ = AppendMenuW(menu, MF_STRING, 1, w!("スタートアップ時に起動 (&S)"));
    let check_flag = if registered {
        MF_BYCOMMAND | MF_CHECKED
    } else {
        MF_BYCOMMAND | MF_UNCHECKED
    };
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

    match cmd.0 {
        1 => set_startup(!registered),
        2 => { let _ = DestroyWindow(hwnd); }
        3 => {
            let sp = STATE_PTR.load(Ordering::Relaxed);
            if !sp.is_null() { open_settings(hwnd, sp); }
        }
        _ => {}
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
fn appbar_register(hwnd: HWND, sw: i32, sh: i32, bar_h: i32) -> RECT {
    unsafe {
        let mut d = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            uCallbackMessage: WM_APPBAR,
            uEdge: ABE_BOTTOM,
            rc: RECT { left: 0, top: sh - bar_h, right: sw, bottom: sh },
            lParam: LPARAM(0),
        };
        SHAppBarMessage(ABM_NEW, &mut d);
        d.rc = RECT { left: 0, top: sh - bar_h, right: sw, bottom: sh };
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

fn hit_btn(x: i32, group_x: i32) -> Hover {
    let prev_x = group_x;
    let play_x = group_x + BTN_W;
    let next_x = group_x + BTN_W * 2;
    if      (prev_x..prev_x + BTN_W).contains(&x) { Hover::Prev }
    else if (play_x..play_x + BTN_W).contains(&x) { Hover::Play }
    else if (next_x..next_x + BTN_W).contains(&x) { Hover::Next }
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

unsafe fn decode_thumbnail(bytes: &[u8], size: i32) -> Option<isize> {
    let stream = SHCreateMemStream(Some(bytes))?;
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let decoder = factory
        .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
        .ok()?;
    let frame = decoder.GetFrame(0).ok()?;
    let scaler = factory.CreateBitmapScaler().ok()?;
    scaler
        .Initialize(&frame, size as u32, size as u32, WICBitmapInterpolationModeFant)
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

    let buf = std::slice::from_raw_parts_mut(
        bits as *mut u8,
        (stride * size as u32) as usize,
    );
    converter.CopyPixels(std::ptr::null(), stride, buf).ok()?;
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

    if hover != Hover::None {
        let hx = match hover {
            Hover::Prev => gx,
            Hover::Play => gx + BTN_W,
            Hover::Next => gx + BTN_W * 2,
            Hover::None => unreachable!(),
        };
        let hov_br = CreateSolidBrush(COLORREF(C_BTN_HOV));
        FillRect(dc, &RECT { left: hx, top: gy, right: hx + BTN_W, bottom: gy2 }, hov_br);
        let _ = DeleteObject(hov_br);
    }

    let _ = RestoreDC(dc, saved);
    let _ = DeleteObject(rgn);

    let bdr = if hover != Hover::None { C_BTN_BDR_H } else { C_BTN_BDR };
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

    let mut sep = w16(" - ");
    let mut ssz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &sep, &mut ssz);
    let sep_x = x + title_alloc + 2;
    if sep_x + ssz.cx >= x + avail { SelectObject(dc, of); return; }

    SetTextColor(dc, COLORREF(C_SEP));
    let mut sr = RECT { left: sep_x, top, right: sep_x + ssz.cx, bottom: h };
    DrawTextW(dc, &mut sep, &mut sr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

    let art_x = sep_x + ssz.cx;
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
    let text_x  = group_x + GROUP_W + 14;

    let mdc = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mdc, bmp);

    let bg = CreateSolidBrush(COLORREF(C_BG));
    FillRect(mdc, &rc, bg);
    let _ = DeleteObject(bg);

    let info  = state.info.lock().unwrap().clone();
    let hover = *state.hover.lock().unwrap();

    // Album art
    let art_top = ACCENT_H;
    let art_h   = h - ACCENT_H;
    {
        let mut cache = state.art.lock().unwrap();
        if cache.as_ref().map(|c| c.key.as_str()) != Some(info.title.as_str()) {
            if let Some(old) = cache.take() {
                let _ = DeleteObject(HBITMAP(old.hbmp as *mut _));
            }
            if !info.title.is_empty() {
                if let Some(ref bytes) = info.thumbnail {
                    if let Some(hbmp) = decode_thumbnail(bytes, art_w) {
                        *cache = Some(CachedArt { key: info.title.clone(), hbmp });
                    }
                }
            }
        }
        if let Some(ref cached) = *cache {
            let art_dc = CreateCompatibleDC(mdc);
            let old = SelectObject(art_dc, HBITMAP(cached.hbmp as *mut _));
            let _ = StretchBlt(mdc, 0, art_top, art_w, art_h, art_dc, 0, 0, art_w, art_w, SRCCOPY);
            SelectObject(art_dc, old);
            let _ = DeleteDC(art_dc);
        } else {
            let ph    = RECT { left: 0, top: art_top, right: art_w, bottom: h };
            let ph_br = CreateSolidBrush(COLORREF(C_ART_PH));
            FillRect(mdc, &ph, ph_br);
            let _ = DeleteObject(ph_br);
        }
    }

    let btn_top  = ACCENT_H + (h - ACCENT_H - BTN_H) / 2;
    let font_btn = HFONT(state.font_btn.load(Ordering::Relaxed) as *mut _);
    draw_btn_group(mdc, btn_top, group_x, hover, info.playing, font_btn);

    let has_seek = info.duration_100ns > 0;
    let text_max_x = if has_seek {
        w - SEEK_PAD_R - SEEK_W - TIME_GAP - TIME_W - TIME_GAP
    } else {
        w
    };
    let font = HFONT(state.font.load(Ordering::Relaxed) as *mut _);
    draw_media_text(mdc, &info, font, text_x, text_max_x, h);

    if has_seek {
        let sx      = w - SEEK_PAD_R - SEEK_W;
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
            let fill_br = CreateSolidBrush(COLORREF(state.accent.load(Ordering::Relaxed)));
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

    // Accent border — drawn last so it always renders on top
    let acc_rc = RECT { left: 0, top: 0, right: w, bottom: ACCENT_H };
    let acc = CreateSolidBrush(COLORREF(state.accent.load(Ordering::Relaxed)));
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
    let (bar_h, font_face, font_size) = {
        let s = state.settings.lock().unwrap();
        (s.bar_h, s.font_face.clone(), s.font_size)
    };

    let face_wide: Vec<u16> = font_face.encode_utf16().chain([0u16]).collect();
    let new_font     = make_font(PCWSTR(face_wide.as_ptr()), font_size);
    let new_font_btn = make_font(w!("Segoe MDL2 Assets"), 14);

    let old_font     = state.font.swap(new_font.0 as usize, Ordering::Relaxed);
    let old_font_btn = state.font_btn.swap(new_font_btn.0 as usize, Ordering::Relaxed);
    let _ = DeleteObject(HFONT(old_font as *mut _));
    let _ = DeleteObject(HFONT(old_font_btn as *mut _));

    // Art cache must be invalidated: decoded bitmap size depends on bar_h
    if let Some(old) = state.art.lock().unwrap().take() {
        let _ = DeleteObject(HBITMAP(old.hbmp as *mut _));
    }

    appbar_remove(hwnd_main);
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let rc = appbar_register(hwnd_main, sw, sh, bar_h);
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
    let ww = 340;
    let wh = 196;
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

            // Helper closure — creates a child control and sets its font
            let mk = |class: PCWSTR, text: PCWSTR, style: u32,
                      x: i32, y: i32, cw: i32, ch: i32, id: i32| -> HWND {
                let hw = CreateWindowExW(
                    WINDOW_EX_STYLE(0), class, text,
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                    x, y, cw, ch,
                    hwnd, HMENU(id as usize as *mut _), hi, None,
                ).unwrap_or(HWND(std::ptr::null_mut()));
                SendMessageW(hw, WM_SETFONT, WPARAM(gui_font.0 as usize), LPARAM(1));
                hw
            };

            // Labels
            mk(w!("STATIC"), w!("バーの高さ"),    0, 16, 20, 110, 20, 0);
            mk(w!("STATIC"), w!("px"),              0, 208, 20, 30,  20, 0);
            mk(w!("STATIC"), w!("フォント名"),      0, 16, 54, 110, 20, 0);
            mk(w!("STATIC"), w!("フォントサイズ"), 0, 16, 88, 110, 20, 0);
            mk(w!("STATIC"), w!("px"),              0, 208, 88, 30,  20, 0);

            // Edits
            let barh_edit = mk(w!("EDIT"), w!(""),
                WS_BORDER.0 | WS_TABSTOP.0, 140, 17, 60, 22, IDC_BARH_EDIT);
            let font_edit = mk(w!("EDIT"), w!(""),
                WS_BORDER.0 | WS_TABSTOP.0, 140, 51, 160, 22, IDC_FONT_EDIT);
            let size_edit = mk(w!("EDIT"), w!(""),
                WS_BORDER.0 | WS_TABSTOP.0, 140, 85, 60,  22, IDC_FONTSIZE_EDIT);

            // Buttons (IDOK=1, IDCANCEL=2 — recognised by IsDialogMessageW)
            mk(w!("BUTTON"), w!("OK"),         WS_TABSTOP.0 | 1, 170, 128, 70, 28, 1);
            mk(w!("BUTTON"), w!("キャンセル"), WS_TABSTOP.0,     248, 128, 72, 28, 2);

            // Populate edits with current values
            let t: Vec<u16> = bar_h.to_string().encode_utf16().chain([0u16]).collect();
            let _ = SetWindowTextW(barh_edit, PCWSTR(t.as_ptr()));
            let t: Vec<u16> = font_face.encode_utf16().chain([0u16]).collect();
            let _ = SetWindowTextW(font_edit, PCWSTR(t.as_ptr()));
            let t: Vec<u16> = font_size.to_string().encode_utf16().chain([0u16]).collect();
            let _ = SetWindowTextW(size_edit, PCWSTR(t.as_ptr()));

            LRESULT(0)
        }
        WM_COMMAND => {
            let id  = (wp.0 & 0xFFFF) as i32;
            let ctx = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsCtx;
            if ctx.is_null() { return DefWindowProcW(hwnd, msg, wp, lp); }

            match id {
                1 => { // IDOK — read, validate, apply
                    let mut buf = [0u16; 256];

                    let barh_edit = GetDlgItem(hwnd, IDC_BARH_EDIT)
                        .unwrap_or(HWND(std::ptr::null_mut()));
                    let font_edit = GetDlgItem(hwnd, IDC_FONT_EDIT)
                        .unwrap_or(HWND(std::ptr::null_mut()));
                    let size_edit = GetDlgItem(hwnd, IDC_FONTSIZE_EDIT)
                        .unwrap_or(HWND(std::ptr::null_mut()));

                    let bar_h = {
                        let n = GetWindowTextW(barh_edit, &mut buf) as usize;
                        String::from_utf16_lossy(&buf[..n])
                            .trim().parse::<i32>()
                            .unwrap_or(DEFAULT_BAR_H).clamp(20, 100)
                    };
                    let font_face = {
                        let n = GetWindowTextW(font_edit, &mut buf) as usize;
                        let s = String::from_utf16_lossy(&buf[..n]).trim().to_string();
                        if s.is_empty() { DEFAULT_FONT_FACE.to_string() } else { s }
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let sp = STATE_PTR.load(std::sync::atomic::Ordering::Relaxed);
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
                // rc.bottom = bar height; derive group_x from it
                let group_x = rc.bottom + SIDE_PAD;

                let dragging = {
                    let mut drag = (*sp).seeking.lock().unwrap();
                    if drag.is_some() {
                        let sx = rc.right - SEEK_PAD_R - SEEK_W;
                        *drag = Some(((x - sx) as f64 / SEEK_W as f64).clamp(0.0, 1.0));
                        true
                    } else {
                        false
                    }
                };
                if dragging {
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    let new_hover = hit_btn(x, group_x);
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
            let sx      = rc.right - SEEK_PAD_R - SEEK_W;
            let group_x = rc.bottom + SIDE_PAD;

            if x >= sx && x < sx + SEEK_W && !sp.is_null() {
                let dur = (*sp).info.lock().unwrap().duration_100ns;
                if dur > 0 {
                    let frac = ((x - sx) as f64 / SEEK_W as f64).clamp(0.0, 1.0);
                    *(*sp).seeking.lock().unwrap() = Some(frac);
                    let _ = SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            } else {
                match hit_btn(x, group_x) {
                    Hover::Prev => send_cmd("prev", hw),
                    Hover::Play => send_cmd("play", hw),
                    Hover::Next => send_cmd("next", hw),
                    Hover::None => {}
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
        WM_TIMER => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, 1);
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

        let settings = load_settings();
        let bar_h    = settings.bar_h;

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("MediaBarClass"),
            w!("MediaBar"),
            WS_POPUP | WS_VISIBLE,
            0, sh - bar_h, sw, bar_h,
            None, None, hinstance, None,
        )?;

        let rc = appbar_register(hwnd, sw, sh, bar_h);
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
            // Route Tab/Enter/Escape to the settings window if it's open
            let sh_raw = SETTINGS_HWND.load(Ordering::Relaxed);
            if sh_raw != 0 {
                let sh_hwnd = HWND(sh_raw as *mut _);
                if IsDialogMessageW(sh_hwnd, &msg).as_bool() { continue; }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let ptr = STATE_PTR.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
        if !ptr.is_null() {
            let s = Box::from_raw(ptr);
            if let Some(art) = s.art.lock().unwrap().take() {
                let _ = DeleteObject(HBITMAP(art.hbmp as *mut _));
            }
            let _ = DeleteObject(HFONT(s.font.load(Ordering::Relaxed) as *mut _));
            let _ = DeleteObject(HFONT(s.font_btn.load(Ordering::Relaxed) as *mut _));
        }

        CoUninitialize();
    }
    Ok(())
}
