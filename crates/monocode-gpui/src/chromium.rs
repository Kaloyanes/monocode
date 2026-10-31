//! Chromium for drafts: the Chromium Embedded Framework, downloaded when the
//! person picks it in Settings › Visualizations › Page engine rather than
//! shipped with the app. One version is pinned and its archive is checked
//! against a known hash before anything in it runs. WebKit stays the default
//! and takes over whenever Chromium is missing.
//!
//! The framework runs in `monocode-cef-host`, which ships beside the app's
//! executable. It runs inside a small app bundle laid out here around the
//! download ("MonoCode CEF.app"), because CEF needs a bundle with its helper
//! app and framework in fixed places. The app itself never loads CEF.
use gpui::{App, BorrowAppContext, Global, Task};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// The CEF release drafts run in, which `monocode-cef-host` is built against.
pub(crate) const VERSION: &str = "156.0.14";
/// The Chromium inside it.
pub(crate) const CHROMIUM: &str = "156.0.8078.48";

/// One platform's minimal CEF distribution.
struct Build {
    archive: &'static str,
    sha256: &'static str,
    bytes: u64,
}

#[cfg(target_arch = "aarch64")]
const BUILD: Build = Build {
    archive: "cef_binary_156.0.14+g4fa4ab5+chromium-156.0.8078.48_macosarm64_minimal",
    sha256: "ed994b907ad596ca609a0542a1e2b22aa97e93e98b8141fed4c24aef39b9ccf6",
    bytes: 134_887_274,
};
#[cfg(not(target_arch = "aarch64"))]
const BUILD: Build = Build {
    archive: "cef_binary_156.0.14+g4fa4ab5+chromium-156.0.8078.48_macosx64_minimal",
    sha256: "e584ade521802f8c2779d24996b32ddc183e8502743e5e3910b7247bb08f24f1",
    bytes: 141_687_370,
};

const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
const HOST: &str = "monocode-cef-host";
const HELPER: &str = "MonoCode CEF Helper";

/// Where every downloaded version lives, beside the app's database.
fn root() -> PathBuf {
    monocode_core::store::monocode_database().with_file_name("cef")
}

/// The headless shell earlier builds downloaded, for cleaning up.
fn legacy_root() -> PathBuf {
    monocode_core::store::monocode_database().with_file_name("chromium")
}

/// The bundle the host runs in. `MONOCODE_CEF_APP` names another, for
/// development (see `scripts/cef-host-bundle.sh`).
fn bundle() -> PathBuf {
    if let Some(path) = std::env::var_os("MONOCODE_CEF_APP") {
        return path.into();
    }
    root().join(VERSION).join("MonoCode CEF.app")
}

/// The host binary this build of the app ships. `MONOCODE_CEF_HOST` names
/// another, for development.
fn shipped_host() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MONOCODE_CEF_HOST") {
        return Some(path.into());
    }
    let path = std::env::current_exe().ok()?.parent()?.join(HOST);
    path.is_file().then_some(path)
}

/// The host to start, its bundle made ready: the bundle holds copies of the
/// host this app ships, replaced whenever the app's own copy changes.
pub(crate) fn host() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let bundle = bundle();
    let contents = bundle.join("Contents");
    if !contents.join("Frameworks").join(FRAMEWORK).exists() {
        return None;
    }
    if std::env::var_os("MONOCODE_CEF_APP").is_some() {
        let host = contents.join("MacOS").join(HOST);
        return host.is_file().then_some(host);
    }
    let shipped = shipped_host()?;
    let helper = contents
        .join("Frameworks")
        .join(format!("{HELPER}.app/Contents"));
    // Copies, not links: Chromium starts each helper by its resolved path and
    // the helper finds the framework next to where it is, so a linked helper
    // looks beside the app instead and quits.
    let shipped_bytes = std::fs::read(&shipped).ok()?;
    for copy in [
        contents.join("MacOS").join(HOST),
        helper.join("MacOS").join(HELPER),
    ] {
        let current = std::fs::symlink_metadata(&copy)
            .is_ok_and(|meta| meta.is_file())
            .then(|| std::fs::read(&copy).ok())
            .flatten();
        if current.as_deref() != Some(shipped_bytes.as_slice()) {
            std::fs::create_dir_all(copy.parent()?).ok()?;
            let partial = copy.with_extension("partial");
            std::fs::copy(&shipped, &partial).ok()?;
            std::fs::rename(&partial, &copy).ok()?;
        }
    }
    write_plist(&contents.join("Info.plist"), HOST, "dev.monocode.cef").ok()?;
    write_plist(&helper.join("Info.plist"), HELPER, "dev.monocode.cef.helper").ok()?;
    Some(contents.join("MacOS").join(HOST))
}

fn write_plist(path: &Path, executable: &str, identifier: &str) -> std::io::Result<()> {
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>{executable}</string>
<key>CFBundleIdentifier</key><string>{identifier}</string>
<key>CFBundleName</key><string>{executable}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
"#
    );
    if std::fs::read_to_string(path).is_ok_and(|current| current == plist) {
        return Ok(());
    }
    std::fs::write(path, plist)
}

/// Whether drafts can run in Chromium on this Mac now.
pub(crate) fn installed() -> bool {
    #[cfg(test)]
    if INSTALLED_IN_TEST.get() {
        return true;
    }
    bundle()
        .join("Contents/Frameworks")
        .join(FRAMEWORK)
        .exists()
        && (std::env::var_os("MONOCODE_CEF_APP").is_some() || shipped_host().is_some())
}

/// Why Chromium last gave up on a draft, until a page opens in it again.
static FAILURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records why drafts went back to WebKit, or that Chromium works again.
pub(crate) fn report_failure(reason: Option<String>) {
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = reason;
    }
}

/// Why Chromium could not run the last draft that asked for it, for Settings.
pub(crate) fn failure() -> Option<String> {
    FAILURE.lock().ok()?.clone()
}

#[cfg(test)]
thread_local! {
    /// Lets a test choose Chromium without a browser on disk.
    pub(crate) static INSTALLED_IN_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What the download is doing, for Settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Install {
    #[default]
    Idle,
    Downloading {
        received: u64,
        total: u64,
    },
    Unpacking,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct Installer {
    state: Install,
    _task: Option<Task<()>>,
}
impl Global for Installer {}

pub(crate) fn state(cx: &App) -> Install {
    cx.try_global::<Installer>()
        .map(|installer| installer.state.clone())
        .unwrap_or_default()
}

fn set(state: Install, cx: &mut App) {
    cx.default_global::<Installer>().state = state;
    // Observers of the global (Settings) redraw.
    cx.update_global::<Installer, _>(|_, _| ());
}

/// The archive's size, for saying what picking Chromium costs.
pub(crate) fn download_size() -> u64 {
    BUILD.bytes
}

/// Starts the download unless Chromium is here or already on its way.
pub(crate) fn install(cx: &mut App) {
    if installed() || matches!(state(cx), Install::Downloading { .. } | Install::Unpacking) {
        return;
    }
    if !cfg!(target_os = "macos") {
        set(
            Install::Failed("Chromium drafts need the macOS app.".into()),
            cx,
        );
        return;
    }
    if shipped_host().is_none() {
        set(
            Install::Failed("This build of MonoCode has no Chromium host.".into()),
            cx,
        );
        return;
    }
    let received = Arc::new(AtomicU64::new(0));
    let unpacking = Arc::new(AtomicBool::new(false));
    let (done, result) = async_channel::bounded(1);
    std::thread::spawn({
        let received = received.clone();
        let unpacking = unpacking.clone();
        move || {
            let outcome = download(&received, &unpacking);
            let _ = done.send_blocking(outcome);
        }
    });
    set(
        Install::Downloading {
            received: 0,
            total: BUILD.bytes,
        },
        cx,
    );
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let state = match result.try_recv() {
                Ok(Ok(())) => Install::Idle,
                Ok(Err(error)) => Install::Failed(error),
                Err(async_channel::TryRecvError::Closed) => {
                    Install::Failed("The download stopped unexpectedly.".into())
                }
                Err(async_channel::TryRecvError::Empty) if unpacking.load(Ordering::Relaxed) => {
                    Install::Unpacking
                }
                Err(async_channel::TryRecvError::Empty) => Install::Downloading {
                    received: received.load(Ordering::Relaxed),
                    total: BUILD.bytes,
                },
            };
            let finished = matches!(state, Install::Idle | Install::Failed(_));
            cx.update(|cx| set(state, cx));
            if finished {
                return;
            }
        }
    });
    cx.default_global::<Installer>()._task = Some(task);
}

/// Deletes every downloaded version.
pub(crate) fn remove(cx: &mut App) -> Result<(), String> {
    if matches!(state(cx), Install::Downloading { .. } | Install::Unpacking) {
        return Err("Chromium is still downloading.".into());
    }
    for root in [root(), legacy_root()] {
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        }
    }
    set(Install::Idle, cx);
    Ok(())
}

/// How much the downloaded Chromium takes on disk.
pub(crate) fn disk_size() -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return 0;
        };
        if !meta.is_dir() {
            return meta.len();
        }
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| walk(&entry.path()))
            .sum()
    }
    walk(&root())
}

/// Downloads the pinned archive, checks it, and unpacks its framework in
/// place of any earlier version. Runs on its own thread.
fn download(received: &AtomicU64, unpacking: &AtomicBool) -> Result<(), String> {
    let root = root();
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let url = format!(
        "https://cef-builds.spotifycdn.com/{}.tar.bz2",
        BUILD.archive.replace('+', "%2B")
    );
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .build();
    let response = agent
        .get(&url)
        .call()
        .map_err(|error| format!("Chromium could not be downloaded: {error}"))?;
    let archive = root.join(format!("{VERSION}.tar.bz2.part"));
    let mut file = std::fs::File::create(&archive).map_err(|error| error.to_string())?;
    let mut reader = response.into_reader();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|error| format!("The download was interrupted: {error}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|error| error.to_string())?;
        hasher.update(&buffer[..n]);
        received.fetch_add(n as u64, Ordering::Relaxed);
    }
    drop(file);
    if format!("{:x}", hasher.finalize()) != BUILD.sha256 {
        let _ = std::fs::remove_file(&archive);
        return Err("The download did not match the expected Chromium build.".into());
    }
    unpacking.store(true, Ordering::Relaxed);
    let staging = root.join(format!("{VERSION}.unpacking"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    // Only the framework is needed; the rest is headers and build files.
    let unpacked = std::process::Command::new("/usr/bin/tar")
        .arg("-xjf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .arg(format!("{}/Release/{FRAMEWORK}", BUILD.archive))
        .status()
        .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&archive);
    let framework = staging.join(BUILD.archive).join("Release").join(FRAMEWORK);
    if !unpacked.success() || !framework.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("Chromium could not be unpacked.".into());
    }
    let frameworks = staging.join("MonoCode CEF.app/Contents/Frameworks");
    std::fs::create_dir_all(&frameworks).map_err(|error| error.to_string())?;
    std::fs::rename(&framework, frameworks.join(FRAMEWORK)).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(staging.join(BUILD.archive));
    let home = root.join(VERSION);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::rename(&staging, &home).map_err(|error| error.to_string())?;
    // Earlier versions, and the headless shell earlier builds used, are not
    // used again.
    for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        if entry.file_name() != VERSION {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
    let _ = std::fs::remove_dir_all(legacy_root());
    Ok(())
}
