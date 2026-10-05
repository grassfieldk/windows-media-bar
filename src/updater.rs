use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0, WPARAM},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    UI::WindowsAndMessaging::PostMessageW,
};

pub const MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;
const RELEASES_URL: &str = "https://api.github.com/repos/grassfieldk/windows-media-bar/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/grassfieldk/windows-media-bar/releases/download/";
const MAX_INSTALLER_SIZE: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Checking,
    Current,
    Available(String),
    Downloading,
    Ready(PathBuf),
    CheckFailed(String),
    Failed(String),
}

impl Status {
    pub fn should_auto_update(&self, enabled: bool) -> bool {
        enabled && matches!(self, Self::Available(_))
    }

    pub fn button_text(&self) -> Option<&'static str> {
        match self {
            Self::Available(_) => Some("更新"),
            Self::Downloading | Self::Ready(_) => Some("更新中…"),
            Self::CheckFailed(_) | Self::Failed(_) => Some("更新を再試行"),
            _ => None,
        }
    }

    pub fn bar_button_text(&self) -> Option<&'static str> {
        if matches!(self, Self::CheckFailed(_)) { None } else { self.button_text() }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Checking => "更新を確認中…".into(),
            Self::Current => "最新バージョンです".into(),
            Self::Available(version) => format!("バージョン {version} に更新できます"),
            Self::Downloading => "更新をダウンロード中…".into(),
            Self::Ready(_) => "更新を適用して再起動します…".into(),
            Self::CheckFailed(error) | Self::Failed(error) => error.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct Release {
    version: Version,
    url: String,
    checksum_url: Option<String>,
    digest: Option<String>,
    size: u64,
}

struct State {
    status: Status,
    release: Option<Release>,
}

#[derive(Clone)]
pub struct Updater(Arc<Mutex<State>>);

impl Updater {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(State { status: Status::Current, release: None })))
    }

    pub fn status(&self) -> Status {
        self.0.lock().unwrap().status.clone()
    }

    pub fn fail(&self, message: String) {
        self.0.lock().unwrap().status = Status::Failed(message);
    }

    fn notify(&self, hwnd: isize) {
        unsafe {
            let _ = PostMessageW(HWND(hwnd as *mut _), MESSAGE, WPARAM(0), LPARAM(0));
        }
    }

    pub fn check(&self, hwnd: isize) {
        {
            let mut state = self.0.lock().unwrap();
            if matches!(state.status, Status::Checking | Status::Downloading | Status::Ready(_)) {
                return;
            }
            state.status = Status::Checking;
            state.release = None;
        }
        self.notify(hwnd);
        let updater = self.clone();
        std::thread::spawn(move || {
            let result = check_latest();
            {
                let mut state = updater.0.lock().unwrap();
                match result {
                    Ok(Some(release)) => {
                        state.status = Status::Available(release.version.to_string());
                        state.release = Some(release);
                    }
                    Ok(None) => state.status = Status::Current,
                    Err(error) => state.status = Status::CheckFailed(error),
                }
            }
            updater.notify(hwnd);
        });
    }

    pub fn download(&self, hwnd: isize) {
        let release = {
            let mut state = self.0.lock().unwrap();
            if !matches!(state.status, Status::Available(_) | Status::CheckFailed(_) | Status::Failed(_)) {
                return;
            }
            let Some(release) = state.release.clone() else {
                drop(state);
                self.check(hwnd);
                return;
            };
            state.status = Status::Downloading;
            release
        };
        self.notify(hwnd);
        let updater = self.clone();
        std::thread::spawn(move || {
            let result = download_installer(&release);
            updater.0.lock().unwrap().status = match result {
                Ok(path) => Status::Ready(path),
                Err(error) => Status::Failed(error),
            };
            updater.notify(hwnd);
        });
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("windows-media-bar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .https_only(true)
        .build()
        .map_err(|_| "更新サーバーに接続できません".into())
}

fn select_release(api: ApiRelease, current: &Version) -> Result<Option<Release>, String> {
    let tag = api.tag_name.strip_prefix('v').unwrap_or(&api.tag_name);
    let version = Version::parse(tag).map_err(|_| "更新情報のバージョンが不正です")?;
    if api.draft || api.prerelease || !version.pre.is_empty() || version <= *current {
        return Ok(None);
    }
    let name = format!("windows-media-bar-v{version}-x86_64-setup.exe");
    let asset = api.assets.iter().find(|asset| asset.name == name)
        .ok_or("更新用インストーラが見つかりません")?;
    let expected_url = format!("{DOWNLOAD_PREFIX}{}/{name}", api.tag_name);
    if asset.browser_download_url != expected_url || asset.size == 0 || asset.size > MAX_INSTALLER_SIZE {
        return Err("更新用インストーラの情報が不正です".into());
    }
    let digest = asset.digest.as_deref().and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| valid_digest(value)).map(str::to_owned);
    let checksum_url = api.assets.iter()
        .find(|asset| asset.name == format!("{name}.sha256") && asset.size <= 1024)
        .map(|asset| asset.browser_download_url.clone());
    if let Some(url) = &checksum_url {
        if url != &format!("{expected_url}.sha256") {
            return Err("更新用チェックサムの情報が不正です".into());
        }
    }
    if digest.is_none() && checksum_url.is_none() {
        return Err("更新ファイルの検証情報が見つかりません".into());
    }
    Ok(Some(Release { version, url: expected_url, checksum_url, digest, size: asset.size }))
}

fn check_latest() -> Result<Option<Release>, String> {
    let response = client()?.get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(20))
        .send().map_err(|_| "更新を確認できません。接続を確認して再試行してください")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response.error_for_status().map_err(|_| "更新サーバーが応答できません。後で再試行してください")?;
    let mut bytes = Vec::new();
    response.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|_| "更新情報を読み込めません")?;
    if bytes.len() > 1024 * 1024 { return Err("更新情報が大きすぎます".into()); }
    let api = serde_json::from_slice(&bytes).map_err(|_| "更新情報を読み込めません")?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    select_release(api, &current)
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn checksum(client: &Client, release: &Release) -> Result<String, String> {
    if let Some(digest) = &release.digest { return Ok(digest.clone()); }
    let response = client.get(release.checksum_url.as_ref().ok_or("検証情報が見つかりません")?)
        .send().and_then(|response| response.error_for_status()).map_err(|_| "検証情報を取得できません")?;
    let mut bytes = Vec::new();
    response.take(1025).read_to_end(&mut bytes).map_err(|_| "検証情報を読み込めません")?;
    if bytes.len() > 1024 { return Err("検証情報が不正です".into()); }
    let text = String::from_utf8(bytes).map_err(|_| "検証情報が不正です")?;
    let digest = text.split_whitespace().next().filter(|value| valid_digest(value))
        .ok_or("検証情報が不正です")?;
    Ok(digest.to_owned())
}

fn updates_dir() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("WindowsMediaBar").join("updates"))
        .ok_or_else(|| "更新ファイルの保存先が見つかりません".into())
}

fn download_installer(release: &Release) -> Result<PathBuf, String> {
    let client = client()?;
    let expected = checksum(&client, release)?;
    let root = updates_dir()?;
    fs::create_dir_all(&root).map_err(|_| "更新ファイルの保存先を作成できません")?;
    // Completed helper processes can leave their executable until a later launch
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("job-") && entry.path().is_dir() {
                let old = entry.metadata().and_then(|meta| meta.modified()).ok()
                    .and_then(|time| time.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(86400));
                if old { let _ = fs::remove_dir_all(entry.path()); }
            }
        }
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let job = root.join(format!("job-{}-{nonce}", std::process::id()));
    fs::create_dir(&job).map_err(|_| "更新ファイルの保存先を作成できません")?;
    let path = job.join("setup.exe");
    let result = (|| {
        let response = client.get(&release.url).send().and_then(|response| response.error_for_status())
            .map_err(|_| "更新をダウンロードできません。再試行してください")?;
        let mut reader = response.take(release.size + 1);
        let mut file = File::create(&path).map_err(|_| "更新ファイルを保存できません")?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let read = reader.read(&mut buffer).map_err(|_| "更新のダウンロードが中断されました")?;
            if read == 0 { break; }
            size += read as u64;
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read]).map_err(|_| "更新ファイルを保存できません")?;
        }
        file.sync_all().map_err(|_| "更新ファイルを保存できません")?;
        let actual = format!("{:x}", hasher.finalize());
        verify_download(size, release.size, &actual, &expected)?;
        Ok(path.clone())
    })();
    if result.is_err() { let _ = fs::remove_dir_all(job); }
    result
}

fn verify_download(size: u64, expected_size: u64, digest: &str, expected_digest: &str) -> Result<(), String> {
    if size != expected_size || !digest.eq_ignore_ascii_case(expected_digest) {
        return Err("更新ファイルの検証に失敗しました。再試行してください".into());
    }
    Ok(())
}

pub fn launch_helper(installer: &Path) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "アプリの保存先を取得できません")?;
    let helper = installer.parent().ok_or("更新ファイルの保存先が不正です")?.join("updater.exe");
    fs::copy(&executable, &helper).map_err(|_| "更新用プログラムを準備できません")?;
    Command::new(helper).arg("--apply-update").arg(installer).arg(&executable)
        .arg(std::process::id().to_string()).spawn().map_err(|_| "更新用プログラムを起動できません")?;
    Ok(())
}

fn run_installer(installer: &Path, executable: &Path, parent_id: u32) -> Result<(), String> {
    unsafe {
        // An already exited parent has no process handle to wait on
        if let Ok(parent) = OpenProcess(PROCESS_SYNCHRONIZE, false, parent_id) {
            let result = WaitForSingleObject(parent, 30_000);
            let _ = CloseHandle(parent);
            if result != WAIT_OBJECT_0 { return Err("アプリを終了できませんでした".into()); }
        }
    }
    let directory = executable.parent().ok_or("アプリの保存先が不正です")?;
    let status = Command::new(installer)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/UPDATE=1", "/CLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS"])
        .arg(format!("/DIR={}", directory.display()))
        .arg(format!("/LOG={}", installer.with_file_name("install.log").display()))
        .status().map_err(|_| "更新用インストーラを起動できませんでした")?;
    if !status.success() { return Err(format!("更新を適用できませんでした (コード: {})", status.code().unwrap_or(-1))); }
    Ok(())
}

// Returns true when this process is the temporary update helper
pub fn run_helper_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--apply-update")) { return false; }
    let Some(installer) = args.next().map(PathBuf::from) else { return true; };
    let Some(executable) = args.next().map(PathBuf::from) else { return true; };
    let Some(parent_id) = args.next().and_then(|arg| arg.to_str().and_then(|arg| arg.parse::<u32>().ok())) else { return true; };
    if let Err(error) = run_installer(&installer, &executable, parent_id) {
        let _ = Command::new(&executable).arg("--update-failed").arg(error).spawn();
    }
    let _ = fs::remove_file(installer);
    true
}

pub fn previous_failure() -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--update-failed" { return args.next(); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> ApiRelease {
        let name = format!("windows-media-bar-{tag}-x86_64-setup.exe");
        ApiRelease {
            tag_name: tag.into(), draft: false, prerelease: false,
            assets: vec![ApiAsset {
                browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/{name}"),
                name, size: 42, digest: Some(format!("sha256:{}", "a".repeat(64))),
            }],
        }
    }

    #[test]
    fn only_newer_stable_releases_are_offered() {
        let current = Version::parse("0.9.0").unwrap();
        assert!(select_release(release("v0.9.0"), &current).unwrap().is_none());
        assert!(select_release(release("v0.8.0"), &current).unwrap().is_none());
        assert_eq!(select_release(release("v0.10.0"), &current).unwrap().unwrap().version, Version::parse("0.10.0").unwrap());
        let mut preview = release("v1.0.0");
        preview.prerelease = true;
        assert!(select_release(preview, &current).unwrap().is_none());
    }

    #[test]
    fn untrusted_assets_and_missing_verification_are_rejected() {
        let current = Version::parse("0.1.0").unwrap();
        let mut api = release("v0.2.0");
        api.assets[0].browser_download_url = "https://example.com/setup.exe".into();
        assert!(select_release(api, &current).is_err());
        let mut api = release("v0.2.0");
        api.assets[0].digest = None;
        assert!(select_release(api, &current).is_err());
    }

    #[test]
    fn corrupted_or_incomplete_downloads_are_rejected() {
        let digest = format!("{:x}", Sha256::digest(b"installer"));
        assert!(verify_download(9, 9, &digest, &digest).is_ok());
        assert!(verify_download(8, 9, &digest, &digest).is_err());
        assert!(verify_download(9, 9, &digest, &"0".repeat(64)).is_err());
    }

    #[test]
    fn automatic_installation_requires_the_users_preference() {
        let available = Status::Available("0.2.0".into());
        assert!(!available.should_auto_update(false));
        assert!(available.should_auto_update(true));
        assert!(!Status::Failed("接続失敗".into()).should_auto_update(true));
        assert!(!Status::Downloading.should_auto_update(true));
        assert_eq!(available.button_text(), Some("更新"));
        assert_eq!(Status::Current.button_text(), None);
        let check_failed = Status::CheckFailed("接続失敗".into());
        assert_eq!(check_failed.bar_button_text(), None);
        assert_eq!(check_failed.button_text(), Some("更新を再試行"));
        assert_eq!(available.bar_button_text(), Some("更新"));
    }

    #[test]
    fn checksum_asset_supports_releases_without_an_api_digest() {
        let current = Version::parse("0.1.0").unwrap();
        let mut api = release("v0.2.0");
        api.assets[0].digest = None;
        api.assets.push(ApiAsset {
            name: format!("{}.sha256", api.assets[0].name),
            browser_download_url: format!("{}.sha256", api.assets[0].browser_download_url),
            size: 120, digest: None,
        });
        assert!(select_release(api, &current).unwrap().is_some());
        let mut api = release("v0.2.0");
        api.assets[0].name = "windows-media-bar-v0.2.0-x86_64.exe".into();
        assert!(select_release(api, &current).is_err());
    }
}
