[日本語](README.ja.md)

# Windows Media Bar

A lightweight Windows media control bar that docks to the bottom of your screen. It displays the currently playing track and provides playback controls — all without any visible taskbar footprint.

## Features

- **Always-on-bottom AppBar** — reserves space at the bottom of the screen so windows never overlap it
- **Now playing info** — album art, track title, and artist name
- **Playback controls** — Previous / Play⏸Pause / Next
- **Seek bar** — click or drag to scrub; shows elapsed / total time
- **System accent color** — the 1 px top border follows your Windows accent color
- **Settings** (⚙ gear button, far right) — adjust bar height, font face, and font size; all settings are persisted in the registry
- **Auto-start** — registers itself in `HKCU\...\Run` so it launches with Windows

## Requirements

- Windows 10 version 1809 or later (Windows 11 recommended)
- x86-64 processor
- [Rust](https://rustup.rs/) stable toolchain with the **MSVC** target (`x86_64-pc-windows-msvc`)
- Visual Studio Build Tools (C++ desktop workload) or Visual Studio

## Building

```powershell
cargo build --release
```

The binary will be placed at:

```
target\release\windows-media-bar.exe
```

Run the executable once and it will register itself for auto-start. No installer is needed.

## Settings

Click the **⚙ gear button** at the right end of the bar to open the settings window.

| Setting    | Default                | Range                |
| ---------- | ---------------------- | -------------------- |
| Bar height | 40 px                  | 40-100 px            |
| Font face  | Segoe UI Variable Text | Any installed font   |
| Font size  | 13 pt                  | Any positive integer |

Settings are saved to `HKCU\Software\MediaBar` in the Windows registry and are applied immediately after clicking **OK**.

## Technical notes

- Written entirely in **Rust** using the [`windows`](https://crates.io/crates/windows) crate (v0.58)
- Uses **Win32 AppBar API** (`SHAppBarMessage`) for docked placement
- Reads media metadata via **WinRT SMTC** (`GlobalSystemMediaTransportControlsSessionManager`)
- Album art is decoded with **WIC** (`IWICImagingFactory`) and downscaled with HALFTONE for smooth rendering
- All painting is done with **GDI** double-buffering; no external UI framework
- Settings UI is a plain Win32 window with `EnumFontFamiliesExW` for font enumeration

## License

MIT
