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
- **Auto-start** — enable during installation or from the system tray menu to launch with Windows

## Requirements

- Windows 10 version 1809 or later (Windows 11 recommended)
- x86-64 processor

## Installation

Download `windows-media-bar-v<version>-x86_64-setup.exe` from [Releases](https://github.com/grassfieldk/windows-media-bar/releases) and run it. Installation does not require administrator privileges. Launch the app from the Start menu and enable auto-start during installation or from the system tray menu.

Uninstall through Windows Settings to remove the app and its auto-start entry. Application settings are preserved.

## Updates

The app checks GitHub Releases for a newer stable version on launch. Automatic updates are disabled by default and can be enabled during installation or in Settings.

With automatic updates enabled, the app downloads and installs the update, then restarts. Otherwise, an Update button appears on the bar; click it to download, install, and restart. Settings also provides a button to check for updates.

Downloaded installers are verified before installation. Settings and auto-start preferences are preserved. If checking or installing fails, retry using the button in the app.

## Build requirements

- [Rust](https://rustup.rs/) stable toolchain with the **MSVC** target (`x86_64-pc-windows-msvc`)
- Visual Studio Build Tools (C++ desktop workload) or Visual Studio

## Building

```powershell
# Activate mise in the shell before running tasks
mise activate pwsh | Out-String | Invoke-Expression
mise install
mise run build
```

The binary will be placed at:

```
target\x86_64-pc-windows-msvc\release\windows-media-bar.exe
```

## Building the installer

Install [Inno Setup 6](https://jrsoftware.org/isdl.php), then run in a mise-enabled shell:

```powershell
mise run installer
# If ISCC.exe is installed in a custom location
powershell -NoProfile -File scripts/package-installer.ps1 -CompilerPath 'C:\Tools\Inno Setup 6\ISCC.exe'
```

The installer is generated at `target\installer\windows-media-bar-v<version>-x86_64-setup.exe`, together with a `.sha256` checksum, using the version from `Cargo.toml`. The Release workflow also builds and attaches these files to GitHub Releases for `v1.2.3` tags and manual runs. The release tag must match the version in `Cargo.toml`.

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
