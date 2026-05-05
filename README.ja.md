[English](README.md)

# Windows Media Bar

画面下部にドッキングする、軽量な Windows 向けメディアコントロールバーです。現在再生中のトラックを表示し、タスクバーを圧迫せずに再生操作を行えます。

## 機能

- **常時最下部固定 (AppBar)** — 画面下端に領域を確保し、他のウィンドウと重ならない
- **再生中トラック情報** — アルバムアート・タイトル・アーティスト名を表示
- **再生コントロール** — 前のトラック / 再生⏸一時停止 / 次のトラック
- **シークバー** — クリック・ドラッグでシーク可能、経過時間 / 総時間を表示
- **システムアクセントカラー対応** — バー上端 1 px のラインが Windows のアクセントカラーに追従
- **設定画面** (右端の ⚙ ボタン) — バーの高さ・フォント・フォントサイズを変更可能。設定はレジストリに保存される
- **自動起動** — `HKCU\...\Run` に登録され、Windows 起動時に自動で立ち上がる

## 動作環境

- Windows 10 バージョン 1809 以降（Windows 11 推奨）
- x86-64 プロセッサ
- [Rust](https://rustup.rs/) stable ツールチェーン（**MSVC** ターゲット: `x86_64-pc-windows-msvc`）
- Visual Studio Build Tools（C++ デスクトップ開発ワークロード）または Visual Studio

## ビルド方法

```powershell
cargo build --release
```

ビルド後のバイナリは以下に出力されます。

```
target\release\windows-media-bar.exe
```

実行ファイルを一度起動すると自動起動が登録されます。インストーラーは不要です。

## 設定

バー右端の **⚙ ボタン** をクリックすると設定ウィンドウが開きます。

| 項目           | デフォルト値           | 範囲                               |
| -------------- | ---------------------- | ---------------------------------- |
| バーの高さ     | 40 px                  | 40 〜 100 px                       |
| フォント       | Segoe UI Variable Text | インストール済みのフォントから選択 |
| フォントサイズ | 13 pt                  | 任意の正の整数                     |

設定は `HKCU\Software\MediaBar` にレジストリ保存され、**OK** ボタン押下後に即時反映されます。

## 技術的な補足

- [`windows`](https://crates.io/crates/windows) クレート (v0.58) を使用した純粋な **Rust** 実装
- **Win32 AppBar API** (`SHAppBarMessage`) によるドッキング配置
- **WinRT SMTC** (`GlobalSystemMediaTransportControlsSessionManager`) によるメディア情報取得
- **WIC** (`IWICImagingFactory`) でアルバムアートをデコードし、HALFTONE でスムーズに縮小
- **GDI** ダブルバッファリングで描画（外部 UI フレームワーク不使用）
- 設定 UI は純粋な Win32 ウィンドウ。フォント一覧は `EnumFontFamiliesExW` で列挙

## ライセンス

MIT
