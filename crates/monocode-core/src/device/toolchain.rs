//! The device tools MonoCode runs with the user's Node: expo-device-hub
//! streams simulators and emulators, agent-device lets agents drive them.
//! Each is npm-installed at a pinned version, into a staging directory that is
//! moved into place only once it is complete.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Hub,
    AgentDevice,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Hub, Tool::AgentDevice];
    pub fn package(self) -> &'static str {
        match self {
            Self::Hub => "expo-device-hub",
            Self::AgentDevice => "agent-device",
        }
    }
    /// Pinned: the hub's wire formats are not a stable API, and
    /// `protocol.rs` speaks this version's.
    pub fn pinned(self) -> &'static str {
        match self {
            Self::Hub => "0.22.1",
            Self::AgentDevice => "0.21.31",
        }
    }
    /// The version in use. The hub's is its pin. agent-device's CLI shares
    /// no wire format with MonoCode, so it also takes newer patch releases of
    /// its pin, once one has installed and answered `--version`.
    pub fn version(self) -> String {
        match self {
            Self::Hub => self.pinned().to_owned(),
            Self::AgentDevice => {
                newest_patch(self).unwrap_or_else(|| self.pinned().to_owned())
            }
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Hub => "Device Hub",
            Self::AgentDevice => "agent-device",
        }
    }
    fn entry(self) -> &'static [&'static str] {
        match self {
            Self::Hub => &["dist", "server", "cli.mjs"],
            Self::AgentDevice => &["bin", "agent-device.mjs"],
        }
    }
    /// The folder holding one folder per installed version.
    pub fn versions_dir(self) -> PathBuf {
        root().join("tools").join(self.package())
    }
    pub fn dir_for(self, version: &str) -> PathBuf {
        self.versions_dir().join(version)
    }
    pub fn entry_for(self, version: &str) -> PathBuf {
        let mut path = self
            .dir_for(version)
            .join("node_modules")
            .join(self.package());
        for part in self.entry() {
            path.push(part);
        }
        path
    }
    pub fn install_dir(self) -> PathBuf {
        self.dir_for(&self.version())
    }
    pub fn entry_path(self) -> PathBuf {
        self.entry_for(&self.version())
    }
    pub fn installed(self) -> bool {
        complete(self, &self.version())
    }
}

/// Whether `version` of `tool` finished installing.
fn complete(tool: Tool, version: &str) -> bool {
    tool.entry_for(version).is_file()
        && std::fs::read_to_string(tool.dir_for(version).join(SENTINEL))
            .is_ok_and(|text| text.trim() == version)
}

/// Whether `version` has run here: the hub started and listed devices, or
/// agent-device answered `--version`.
pub fn verified(tool: Tool, version: &str) -> bool {
    complete(tool, version) && tool.dir_for(version).join(VERIFIED).is_file()
}

/// Every complete version on disk, newest first.
fn versions(tool: Tool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(tool.versions_dir()) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && complete(tool, name))
        .collect();
    found.sort_by_key(|name| std::cmp::Reverse(loose_triple(name)));
    found
}

/// The version to run when the current one won't: the newest other one
/// installed, which an older MonoCode ran.
pub fn fallback(tool: Tool) -> Option<String> {
    let current = tool.version();
    versions(tool).into_iter().find(|name| *name != current)
}

/// The newest verified patch release of the pin that is not older than it.
fn newest_patch(tool: Tool) -> Option<String> {
    let pin = triple(tool.pinned())?;
    versions(tool).into_iter().find(|name| {
        triple(name).is_some_and(|v| (v.0, v.1) == (pin.0, pin.1) && v >= pin)
            && tool.dir_for(name).join(VERIFIED).is_file()
    })
}

/// `1.2.3`, without a pre-release or build part.
fn triple(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split('.');
    let triple = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(triple)
}

/// For ordering only: the leading digits of each part, so a canary sorts
/// with its release.
fn loose_triple(text: &str) -> (u32, u32, u32) {
    let mut parts = text.split('.').map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

const SENTINEL: &str = ".install-complete";
const VERIFIED: &str = ".verified";

/// Where the tools, the hub's state and agent-device's launcher live.
pub fn root() -> PathBuf {
    crate::store::monocode_database().with_file_name("devices")
}

/// The PATH device tools run with: what a terminal would have, plus the
/// Android SDK's tools.
pub fn search_path() -> String {
    let mut parts: Vec<PathBuf> = Vec::new();
    if let Some(sdk) = android_sdk() {
        parts.push(sdk.join("platform-tools"));
        parts.push(sdk.join("emulator"));
    }
    parts.extend(std::env::split_paths(&crate::scm::shell::gui_search_path()));
    std::env::join_paths(parts)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&search_path())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

pub fn node() -> Option<PathBuf> {
    which("node")
}

/// The Android SDK the emulator tools come from: `ANDROID_HOME`, the SDK
/// Manager's default location, or wherever `adb` on PATH lives.
pub fn android_sdk() -> Option<PathBuf> {
    let has_tools = |root: &Path| {
        root.join("platform-tools/adb").is_file() || root.join("emulator/emulator").is_file()
    };
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(root) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(root));
        }
    }
    let home = crate::accounts::home()?;
    for candidate in [home.join("Library/Android/sdk"), home.join("Android/Sdk")] {
        if has_tools(&candidate) {
            return Some(candidate);
        }
    }
    let adb = std::env::split_paths(&crate::scm::shell::gui_search_path())
        .map(|dir| dir.join("adb"))
        .find(|path| path.is_file())?;
    let resolved = std::fs::canonicalize(&adb).unwrap_or(adb);
    let root = resolved.parent()?.parent()?.to_path_buf();
    has_tools(&root).then_some(root)
}

/// What this Mac has for each platform, shown in Settings and the panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doctor {
    pub node: Option<(PathBuf, String)>,
    pub xcode: bool,
    pub android_sdk: Option<PathBuf>,
    pub ffmpeg: bool,
}

impl Doctor {
    /// Blocking: runs `node --version` and `xcrun --find simctl`.
    pub fn check() -> Self {
        let node = node().and_then(|path| {
            let output = Command::new(&path)
                .arg("--version")
                .env("PATH", search_path())
                .output()
                .ok()?;
            output.status.success().then(|| {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (path, version)
            })
        });
        let xcode = cfg!(target_os = "macos")
            && Command::new("/usr/bin/xcrun")
                .args(["--find", "simctl"])
                .output()
                .is_ok_and(|output| output.status.success());
        Self {
            node,
            xcode,
            android_sdk: android_sdk(),
            ffmpeg: which("ffmpeg").is_some(),
        }
    }
    /// Node 22.12 or newer, which agent-device asks for.
    pub fn node_supported(&self) -> bool {
        self.node
            .as_ref()
            .and_then(|(_, version)| parse_version(version))
            .is_some_and(|version| version >= (22, 12))
    }
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// One install at a time: two would race for the same directory.
static INSTALLING: Mutex<()> = Mutex::new(());

/// Blocking: installs `tool` unless it already is. Runs npm from the user's
/// Node, so nothing is downloaded but the pinned package and its tree.
///
/// The hub's older versions stay until the new one has run (`settle`), so a
/// pin that won't start here leaves one to fall back to. agent-device's go
/// at once: nothing checks its runs.
pub fn install(tool: Tool) -> Result<PathBuf, String> {
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    if tool.installed() {
        return Ok(tool.entry_path());
    }
    let version = tool.version();
    install_version(tool, &version)?;
    if tool == Tool::AgentDevice {
        remove_other_versions(tool, &version);
    }
    Ok(tool.entry_for(&version))
}

fn npm() -> Result<PathBuf, String> {
    let node = node().ok_or("MonoCode needs Node.js for device tools. Install it from nodejs.org or with Homebrew, then try again.")?;
    node.parent()
        .map(|dir| dir.join("npm"))
        .filter(|path| path.is_file())
        .or_else(|| which("npm"))
        .ok_or_else(|| "npm was not found beside Node.js.".into())
}

/// Installs one version into its folder. The caller holds `INSTALLING`.
fn install_version(tool: Tool, version: &str) -> Result<(), String> {
    let npm = npm()?;
    let final_dir = tool.dir_for(version);
    let parent = final_dir.parent().ok_or("Invalid tools directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&final_dir);
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(&npm);
    command
        .args(["install", "--prefix"])
        .arg(staging.path())
        .args([
            "--no-fund",
            "--no-audit",
            "--no-update-notifier",
            "--loglevel=error",
        ])
        .arg(format!("{}@{}", tool.package(), version))
        .env("PATH", search_path())
        .current_dir(staging.path());
    crate::process::configure_child(&mut command);
    let output = command
        .output()
        .map_err(|e| format!("Could not run npm: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().rev().take(6).collect::<Vec<_>>();
        let detail = detail.into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(format!(
            "npm could not install {}.\n{detail}",
            tool.package()
        ));
    }
    let mut staged = staging.path().join("node_modules").join(tool.package());
    for part in tool.entry() {
        staged.push(part);
    }
    if !staged.is_file() {
        return Err(format!(
            "{} installed without its entry point.",
            tool.package()
        ));
    }
    std::fs::write(staging.path().join(SENTINEL), format!("{version}\n"))
        .map_err(|e| e.to_string())?;
    let staging = staging.keep();
    if let Err(error) = std::fs::rename(&staging, &final_dir) {
        let _ = std::fs::remove_dir_all(&staging);
        if !complete(tool, version) {
            return Err(format!(
                "Could not move {} into place: {error}",
                tool.package()
            ));
        }
    }
    Ok(())
}

/// Marks `version` as one that ran here and removes the others: there is
/// no need to fall back any more.
pub fn settle(tool: Tool, version: &str) {
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    if !complete(tool, version) {
        return;
    }
    let _ = std::fs::write(tool.dir_for(version).join(VERIFIED), "");
    remove_other_versions(tool, version);
}

/// Blocking, once at launch: when a MonoCode update moved a pin, installs
/// it now, so the first use doesn't wait on npm. Only tools some version of
/// which is already here: a person who never set devices up downloads
/// nothing. Then moves agent-device to its newest patch release.
pub fn refresh(agents: bool) {
    for tool in Tool::ALL {
        if tool == Tool::AgentDevice && !agents {
            continue;
        }
        if !tool.installed() && !versions(tool).is_empty() {
            let _ = install(tool);
        }
    }
    if agents {
        update_agent_device();
    }
}

/// How often agent-device asks npm for a newer patch release.
const UPDATE_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Blocking, at most once a day: installs agent-device's newest patch
/// release of its pin, and switches to it once it answers `--version`.
fn update_agent_device() {
    let tool = Tool::AgentDevice;
    if !tool.installed() {
        return;
    }
    let stamp = tool.versions_dir().join(".checked");
    let recent = std::fs::metadata(&stamp)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < UPDATE_EVERY);
    if recent {
        return;
    }
    let _ = std::fs::write(&stamp, "");
    let Some(latest) = latest_patch(tool) else {
        return;
    };
    let current = tool.version();
    if triple(&latest) <= triple(&current) {
        return;
    }
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    if install_version(tool, &latest).is_err() {
        return;
    }
    if reports_version(tool, &latest) {
        let _ = std::fs::write(tool.dir_for(&latest).join(VERIFIED), "");
        remove_other_versions(tool, &latest);
    } else if !in_use(&tool.dir_for(&latest)) {
        let _ = std::fs::remove_dir_all(tool.dir_for(&latest));
    }
}

/// The newest release npm has in the pin's patch range (`~pin`).
fn latest_patch(tool: Tool) -> Option<String> {
    let mut command = Command::new(npm().ok()?);
    command
        .args(["view", &format!("{}@~{}", tool.package(), tool.pinned())])
        .args(["version", "--json", "--no-update-notifier"])
        .env("PATH", search_path())
        .current_dir(root())
        .stdin(std::process::Stdio::null());
    crate::process::configure_child(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    newest_listed(&String::from_utf8_lossy(&output.stdout))
}

/// `npm view --json` gives one version as a string and several as a list.
fn newest_listed(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let listed: Vec<String> = match value {
        serde_json::Value::String(one) => vec![one],
        serde_json::Value::Array(many) => many
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => return None,
    };
    listed
        .into_iter()
        .filter_map(|v| triple(&v).map(|t| (t, v)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, v)| v)
}

/// Whether the installed CLI runs and names the version it was installed as.
fn reports_version(tool: Tool, version: &str) -> bool {
    let Some(node) = node() else {
        return false;
    };
    let mut command = Command::new(node);
    command
        .arg(tool.entry_for(version))
        .arg("--version")
        .env("PATH", search_path())
        .env("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1")
        .stdin(std::process::Stdio::null());
    crate::process::configure_child(&mut command);
    command.output().is_ok_and(|output| {
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == version
    })
}

/// Versions other than `keep` are dead weight, unless something still runs
/// from one: another copy of MonoCode built with the older pin keeps its hub
/// there, serve-sim starts its helpers from disk, and an agent may be midway
/// through an agent-device command. The next settle after it quits removes
/// it.
fn remove_other_versions(tool: Tool, keep: &str) {
    let Ok(entries) = std::fs::read_dir(tool.versions_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != keep
            && !name.starts_with('.')
            && entry.path().is_dir()
            && !in_use(&entry.path())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether a running process was started from inside `dir`. Unsure counts
/// as yes: a leftover folder costs disk, a deleted one breaks a hub.
fn in_use(dir: &Path) -> bool {
    let Ok(output) = Command::new("/bin/ps")
        .args(["-axww", "-o", "command="])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let prefix = format!("{}/", dir.to_string_lossy());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|command| command.contains(&prefix))
}

/// Removes every installed tool. The hub must be stopped first.
pub fn remove_all() -> Result<(), String> {
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::remove_dir_all(root().join("tools")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// A launcher for agent-device that agents run by its full path, so a login
/// shell resetting PATH cannot lose it. It runs the pinned CLI with the
/// user's Node.
pub fn agent_launcher() -> Result<PathBuf, String> {
    if !Tool::AgentDevice.installed() {
        return Err(
            "agent-device is not installed. Turn on agent access in Settings › Devices.".into(),
        );
    }
    let node = node().ok_or("Node.js was not found.")?;
    let dir = root().join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = launcher_script(
        &search_path(),
        &node,
        &Tool::AgentDevice.entry_path(),
        &uses_dir(),
    );
    let path = dir.join("agent-device");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
        let temporary = dir.join(".agent-device.tmp");
        std::fs::write(&temporary, &script).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Where the agent-device launcher marks each run: an empty file named for
/// the device it drove, touched every time, so the hub can tell an agent is
/// using a device that nobody watches. A run that names no device marks
/// `ANY_DEVICE`, which counts for all of them.
pub fn uses_dir() -> PathBuf {
    root().join("uses")
}

pub const ANY_DEVICE: &str = "any";

/// The launcher's script. It finds the device in `--udid`, `--serial` or
/// `--device` and marks it before running agent-device; a name with
/// anything but letters, digits, `.`, `_` and `-` marks `ANY_DEVICE`.
fn launcher_script(path: &str, node: &Path, entry: &Path, uses: &Path) -> String {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"));
    format!(
        "#!/bin/sh
PATH={path}
export PATH
export AGENT_DEVICE_NO_UPDATE_NOTIFIER=1
used={ANY_DEVICE}
previous=
for arg in \"$@\"; do
  case \"$previous\" in --udid|--serial|--device) used=$arg ;; esac
  case \"$arg\" in --udid=*|--serial=*|--device=*) used=${{arg#*=}} ;; esac
  previous=$arg
done
case \"$used\" in ''|*[!A-Za-z0-9._-]*) used={ANY_DEVICE} ;; esac
/bin/mkdir -p {uses} 2>/dev/null && /usr/bin/touch {uses}/\"$used\" 2>/dev/null
exec {node} {entry} \"$@\"
",
        path = quote(Path::new(path)),
        uses = quote(uses),
        node = quote(node),
        entry = quote(entry),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launcher marks the device each run names, then runs the CLI with
    /// every argument as given.
    #[cfg(unix)]
    #[test]
    fn the_agent_launcher_marks_the_device_it_drives() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("monocode-launcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let uses = dir.join("it's uses");
        let script = dir.join("agent-device");
        // `node` stands in as /bin/echo, so the run prints what it was given.
        std::fs::write(
            &script,
            launcher_script(
                "/usr/bin:/bin",
                Path::new("/bin/echo"),
                Path::new("cli.js"),
                &uses,
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |args: &[&str]| {
            let output = Command::new(&script).args(args).output().unwrap();
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        assert_eq!(
            run(&[
                "snapshot",
                "-i",
                "--platform",
                "ios",
                "--udid",
                "0E9B-1",
                "--session",
                "mc-1"
            ]),
            "cli.js snapshot -i --platform ios --udid 0E9B-1 --session mc-1"
        );
        assert!(uses.join("0E9B-1").is_file());
        run(&["press", "--serial=emulator-5554"]);
        assert!(uses.join("emulator-5554").is_file());
        assert!(!uses.join(ANY_DEVICE).exists());
        run(&["devices"]);
        run(&["press", "--udid", "../escape"]);
        assert!(uses.join(ANY_DEVICE).is_file());
        assert!(!dir.join("escape").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A version another MonoCode's hub runs from is kept.
    #[test]
    fn a_version_a_process_runs_from_is_in_use() {
        let dir = std::env::temp_dir().join(format!("monocode-in-use-{}", std::process::id()));
        let entry = dir.join("0.1.0/node_modules/expo-device-hub/dist/server/cli.mjs");
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 5; true"])
            .arg(&entry)
            .spawn()
            .unwrap();
        // ps lists the child once it has started.
        let started = std::time::Instant::now();
        while !in_use(&dir.join("0.1.0")) && started.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let used = in_use(&dir.join("0.1.0"));
        let other = in_use(&dir.join("0.1"));
        let _ = child.kill();
        let _ = child.wait();
        assert!(used);
        // A name that only begins the same is a different folder.
        assert!(!other);
    }

    #[test]
    fn node_versions_compare_by_major_and_minor() {
        assert_eq!(parse_version("v22.11.0"), Some((22, 11)));
        let doctor = |version: &str| Doctor {
            node: Some(("node".into(), version.into())),
            ..Default::default()
        };
        assert!(!doctor("v22.11.0").node_supported());
        assert!(doctor("v22.12.0").node_supported());
        assert!(doctor("v24.1.0").node_supported());
        assert!(!Doctor::default().node_supported());
    }

    #[test]
    fn patch_releases_are_read_from_npm_view() {
        assert_eq!(newest_listed("\"0.22.1\"\n").as_deref(), Some("0.22.1"));
        assert_eq!(
            newest_listed("[\"0.21.31\", \"0.21.36\", \"0.21.9\"]").as_deref(),
            Some("0.21.36")
        );
        // Pre-releases never count as an update.
        assert_eq!(
            newest_listed("[\"0.21.31\", \"0.21.40-canary.1\"]").as_deref(),
            Some("0.21.31")
        );
        assert_eq!(newest_listed(""), None);
        assert_eq!(newest_listed("{}"), None);
    }

    #[test]
    fn versions_order_newest_first_with_canaries_beside_releases() {
        assert_eq!(triple("0.21.36"), Some((0, 21, 36)));
        assert_eq!(triple("0.19.0-canary-20261023"), None);
        assert_eq!(triple("0.21"), None);
        let mut names = vec!["0.19.0-canary-20261023", "0.22.1", "0.21.9", "0.21.31"];
        names.sort_by_key(|name| std::cmp::Reverse(loose_triple(name)));
        assert_eq!(names, ["0.22.1", "0.21.31", "0.21.9", "0.19.0-canary-20261023"]);
    }

    #[test]
    fn tools_live_under_their_pinned_version() {
        let entry = Tool::Hub.entry_path();
        assert!(entry.ends_with("node_modules/expo-device-hub/dist/server/cli.mjs"));
        assert!(entry.to_string_lossy().contains(Tool::Hub.pinned()));
    }
}
