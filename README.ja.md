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
- **自動起動** — インストール時またはタスクトレイのメニューで有効にすると、Windows 起動時に自動で立ち上がる

## 動作環境

- Windows 10 バージョン 1809 以降（Windows 11 推奨）
- x86-64 プロセッサ

## インストール

[Releases](https://github.com/grassfieldk/windows-media-bar/releases) から `windows-media-bar-v<バージョン>-x86_64-setup.exe` をダウンロードして実行します。管理者権限は不要です。スタートメニューから起動でき、自動起動はインストール時またはタスクトレイのメニューから設定できます。

Windows の「インストールされているアプリ」からアンインストールすると、自動起動の登録も削除されます。アプリの設定は保持されます。

## アップデート

起動時に GitHub Releases の最新版を確認します。自動更新は初期状態では無効で、インストール時または設定画面の「起動時に自動更新する」で選択できます。

有効の場合は、新しいバージョンをダウンロードして適用し、アプリを再起動します。無効の場合は、バーに表示される「更新」ボタンを押すと更新して再起動します。設定画面から更新を再確認できます。

ダウンロードしたファイルを検証してから更新します。更新後も設定と自動起動の状態は引き継がれます。接続や更新に失敗した場合は、アプリ内のボタンから再試行できます。

## ビルド環境

- [Rust](https://rustup.rs/) stable ツールチェーン（**MSVC** ターゲット: `x86_64-pc-windows-msvc`）
- Visual Studio Build Tools（C++ デスクトップ開発ワークロード）または Visual Studio

## ビルド方法

```powershell
# mise をシェルで有効にしてから実行
mise activate pwsh | Out-String | Invoke-Expression
mise install
mise run build
```

ビルド後のバイナリは以下に出力されます。

```
target\x86_64-pc-windows-msvc\release\windows-media-bar.exe
```

## インストーラの作成

[Inno Setup 6](https://jrsoftware.org/isdl.php) を導入し、mise を有効にしたシェルで実行します。

```powershell
mise run installer
# ISCC.exe が標準の場所にない場合
powershell -NoProfile -File scripts/package-installer.ps1 -CompilerPath 'C:\Tools\Inno Setup 6\ISCC.exe'
```

`target\installer\windows-media-bar-v<バージョン>-x86_64-setup.exe` と、ファイル検証用の `.sha256` が生成されます。バージョンは `Cargo.toml` から取得します。`Cargo.toml` のバージョンと一致する `v1.2.3` 形式のタグによるリリース、または Release ワークフローの手動実行でもインストーラが生成され、GitHub Releases に添付されます。

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
