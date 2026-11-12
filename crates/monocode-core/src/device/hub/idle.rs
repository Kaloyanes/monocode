//! Shuts down simulators and emulators MonoCode started once nobody watches
//! or uses them, so a device booted for one task doesn't run all day.
//!
//! Only a device this app booted, and still in the boot it started (the
//! same `launchd_sim` or emulator process), is ever shut down, and only once
//! every sign of use has been quiet for `IDLE_DEVICE`:
//! - no viewer streams it;
//! - nothing in MonoCode named it (`touch_device`: the panel's stream, the
//!   tool rail, agents' device tools);
//! - no agent ran `agent-device` against it: the launcher MonoCode hands
//!   agents marks each run (`toolchain::uses_dir`);
//! - no other process names it, such as Xcode's test runner, simctl, adb or
//!   another app's viewer;
//! - for a simulator, Simulator.app or Xcode's DeviceHub isn't open, where a
//!   person may be using it in a window MonoCode can't see into.
use super::super::{toolchain, Platform};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

/// A device MonoCode started is shut down after this long with nobody
/// watching it and nothing using it.
pub const IDLE_DEVICE: Duration = Duration::from_secs(15 * 60);

/// A device this app booted, with the pid of the process that is that boot
/// (a simulator's `launchd_sim`, an emulator's own process). Kept on disk
/// so a restart still knows it; a different pid means someone else booted
/// it since, and it is no longer ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Started {
    pub id: String,
    pub platform: Platform,
    pub name: String,
    pub pid: u32,
}

#[derive(Default)]
struct Use {
    viewers: usize,
    last: Option<Instant>,
}

static USES: Mutex<BTreeMap<String, Use>> = Mutex::new(BTreeMap::new());
/// Loaded from disk on first use.
static STARTED: Mutex<Option<Vec<Started>>> = Mutex::new(None);

/// When this run began watching: a device nothing touched this run counts
/// as idle from here, not from whenever it booted.
fn since() -> Instant {
    static AT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *AT.get_or_init(Instant::now)
}

/// Counts as use of one device, holding off its idle shutdown.
pub fn touch_device(id: &str) {
    since();
    let mut uses = USES.lock().unwrap_or_else(|e| e.into_inner());
    // Called for every read of a video stream: no allocation once known.
    match uses.get_mut(id) {
        Some(entry) => entry.last = Some(Instant::now()),
        None => {
            uses.insert(
                id.to_owned(),
                Use {
                    viewers: 0,
                    last: Some(Instant::now()),
                },
            );
        }
    }
}

pub(super) fn viewer(id: &str, added: bool) {
    since();
    let mut uses = USES.lock().unwrap_or_else(|e| e.into_inner());
    let entry = uses.entry(id.to_owned()).or_default();
    entry.viewers = if added {
        entry.viewers + 1
    } else {
        entry.viewers.saturating_sub(1)
    };
    entry.last = Some(Instant::now());
}

/// How many viewers `id` has and how long since MonoCode last used it.
fn in_app(id: &str) -> (usize, Duration) {
    let uses = USES.lock().unwrap_or_else(|e| e.into_inner());
    let entry = uses.get(id);
    let last = entry.and_then(|e| e.last).unwrap_or_else(since);
    (entry.map_or(0, |e| e.viewers), last.elapsed())
}

/// How long since an agent last ran `agent-device` against `id`, or against
/// no device in particular; `None` if never.
fn agent_quiet(id: &str) -> Option<Duration> {
    agent_quiet_in(&toolchain::uses_dir(), id, SystemTime::now())
}

fn agent_quiet_in(dir: &std::path::Path, id: &str, now: SystemTime) -> Option<Duration> {
    [id, toolchain::ANY_DEVICE]
        .into_iter()
        .filter_map(|name| std::fs::metadata(dir.join(name)).ok()?.modified().ok())
        .map(|at| now.duration_since(at).unwrap_or_default())
        .min()
}

/// Everything known about one started device at a check.
#[derive(Clone, Debug)]
pub(super) struct Signs {
    /// Its boot is still the one this app started.
    pub same_boot: bool,
    pub viewers: usize,
    pub in_app: Duration,
    pub agent: Option<Duration>,
    /// A process outside the device itself and outside this app's hub names
    /// it on its command line.
    pub named_elsewhere: bool,
    /// Simulator.app or DeviceHub is open (for a simulator only).
    pub simulator_app: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum Verdict {
    Keep,
    ShutDown,
    /// It shut down, or someone else booted it since; it isn't ours.
    Forget,
}

pub(super) fn verdict(signs: &Signs) -> Verdict {
    if !signs.same_boot {
        return Verdict::Forget;
    }
    let quiet = signs.viewers == 0
        && signs.in_app >= IDLE_DEVICE
        && signs.agent.is_none_or(|quiet| quiet >= IDLE_DEVICE)
        && !signs.named_elsewhere
        && !signs.simulator_app;
    if quiet {
        Verdict::ShutDown
    } else {
        Verdict::Keep
    }
}

/// One `ps` line.
#[derive(Clone, Debug)]
struct Process {
    pid: u32,
    ppid: u32,
    pgid: u32,
    command: String,
}

/// Every process on the machine, read once per check.
pub(super) struct Processes(Vec<Process>);

impl Processes {
    pub(super) fn read() -> Option<Self> {
        let output = Command::new("/bin/ps")
            .args(["-axww", "-o", "pid=,ppid=,pgid=,command="])
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        Some(Self::parse(&String::from_utf8_lossy(&output.stdout)))
    }

    fn parse(text: &str) -> Self {
        Self(
            text.lines()
                .filter_map(|line| {
                    let mut parts = line.split_whitespace();
                    let pid = parts.next()?.parse().ok()?;
                    let ppid = parts.next()?.parse().ok()?;
                    let pgid = parts.next()?.parse().ok()?;
                    let command = parts.collect::<Vec<_>>().join(" ");
                    Some(Process {
                        pid,
                        ppid,
                        pgid,
                        command,
                    })
                })
                .collect(),
        )
    }

    /// The process that is this device's boot: a simulator's `launchd_sim`,
    /// or the topmost emulator process for the AVD (on `serial`'s port,
    /// when the serial is known).
    pub(super) fn root(&self, platform: Platform, id: &str, name: &str) -> Option<u32> {
        let matches = |process: &Process| match platform {
            Platform::Ios => {
                process.command.contains("launchd_sim")
                    && process.command.contains(&format!("/Devices/{id}/"))
            }
            Platform::Android => {
                let words: Vec<&str> = process.command.split_whitespace().collect();
                let has = |flag: &str, value: &str| {
                    words
                        .windows(2)
                        .any(|pair| pair[0] == flag && pair[1] == value)
                };
                let emulator = words.first().is_some_and(|program| {
                    program.ends_with("/emulator") || program.contains("qemu-system")
                });
                let port = id.strip_prefix("emulator-");
                emulator && has("-avd", name) && port.is_none_or(|port| has("-port", port))
            }
        };
        let found: Vec<&Process> = self.0.iter().filter(|p| matches(p)).collect();
        found
            .iter()
            .find(|p| !found.iter().any(|parent| parent.pid == p.ppid))
            .map(|p| p.pid)
    }

    /// Whether `pid` is `root` or runs under it.
    fn within(&self, mut pid: u32, root: u32) -> bool {
        for _ in 0..64 {
            if pid == root {
                return true;
            }
            match self.0.iter().find(|p| p.pid == pid) {
                Some(process) if process.ppid > 1 && process.ppid != pid => pid = process.ppid,
                _ => return false,
            }
        }
        false
    }

    /// Whether anything but the device itself, this app and its hub names
    /// `id` on its command line.
    pub(super) fn names_elsewhere(
        &self,
        id: &str,
        root: Option<u32>,
        hub_group: Option<u32>,
        own: u32,
    ) -> bool {
        self.0.iter().any(|process| {
            process.command.contains(id)
                && process.pid != own
                && Some(process.pgid) != hub_group
                && !root.is_some_and(|root| self.within(process.pid, root))
        })
    }

    pub(super) fn simulator_app_open(&self) -> bool {
        self.0.iter().any(|process| {
            let program = process.command.split(" -").next().unwrap_or_default();
            program.ends_with("/DeviceHub.app/Contents/MacOS/DeviceHub")
                || program.ends_with("/Simulator.app/Contents/MacOS/Simulator")
        })
    }
}

fn file() -> std::path::PathBuf {
    toolchain::root().join("started-devices.json")
}

fn with_started<T>(f: impl FnOnce(&mut Vec<Started>) -> T) -> T {
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let list = started.get_or_insert_with(|| {
        std::fs::read_to_string(file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    });
    f(list)
}

fn save(list: &[Started]) {
    if let Ok(text) = serde_json::to_string(list) {
        let _ = std::fs::write(file(), text);
    }
}

/// Records that this app just booted `id`. `before` is the process list from
/// just before the boot: a device that was already running there was booted
/// by someone else, and a boot with no process to tell it by is left alone.
pub(super) fn booted(platform: Platform, id: &str, name: &str, before: Option<&Processes>) {
    let Some(before) = before else {
        return;
    };
    if before.root(platform, id, name).is_some()
        || (platform == Platform::Android && before.root(platform, name, name).is_some())
    {
        return;
    }
    let Some(pid) = Processes::read().and_then(|now| now.root(platform, id, name)) else {
        return;
    };
    touch_device(id);
    with_started(|list| {
        list.retain(|s| s.id != id);
        list.push(Started {
            id: id.to_owned(),
            platform,
            name: name.to_owned(),
            pid,
        });
        save(list);
    });
}

/// Shuts down started devices that have gone idle, and forgets those that
/// aren't ours any more. `shut` does the shutting down. Cheap when nothing
/// could be due: the process list is read only when a device has been
/// quiet in MonoCode for `IDLE_DEVICE`.
pub(super) fn retire(hub_group: Option<u32>, shut: impl Fn(&Started) -> Result<(), String>) {
    let started = with_started(|list| list.clone());
    if started.is_empty() {
        return;
    }
    let due = started.iter().any(|s| {
        let (viewers, quiet) = in_app(&s.id);
        viewers == 0 && quiet >= IDLE_DEVICE && agent_quiet(&s.id).is_none_or(|q| q >= IDLE_DEVICE)
    });
    if !due {
        return;
    }
    let Some(processes) = Processes::read() else {
        return;
    };
    let simulator_app = processes.simulator_app_open();
    let own = std::process::id();
    let mut done = Vec::new();
    for device in &started {
        let root = processes.root(device.platform, &device.id, &device.name);
        let (viewers, in_app) = in_app(&device.id);
        let signs = Signs {
            same_boot: root == Some(device.pid),
            viewers,
            in_app,
            agent: agent_quiet(&device.id),
            named_elsewhere: processes.names_elsewhere(&device.id, root, hub_group, own),
            simulator_app: device.platform == Platform::Ios && simulator_app,
        };
        match verdict(&signs) {
            Verdict::Keep => {}
            Verdict::Forget => done.push(device.id.clone()),
            Verdict::ShutDown => {
                if shut(device).is_ok() {
                    done.push(device.id.clone());
                }
            }
        }
    }
    if !done.is_empty() {
        with_started(|list| {
            list.retain(|s| !done.contains(&s.id));
            save(list);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
    1     0     1 /sbin/launchd
  500     1   500 launchd_sim /Users/me/Library/Developer/CoreSimulator/Devices/AAAA-1111/data/var/run/launchd_bootstrap.plist
  501   500   501 /Library/Developer/CoreSimulator/Volumes/iOS/SpringBoard --udid AAAA-1111
  600     1   600 launchd_sim /Users/me/Library/Developer/CoreSimulator/Devices/BBBB-2222/data/var/run/launchd_bootstrap.plist
  700   650   700 /Users/me/sdk/emulator/emulator -avd Pixel_9 -no-audio -no-window -port 5554
  701   700   700 /Users/me/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -avd Pixel_9 -port 5554
  650     1   650 node /x/expo-device-hub/cli.mjs --port 4000
  651   650   650 adb -s emulator-5554 shell app_process / com.genymobile.scrcpy.Server 4.0 max_fps=60
  900     1   900 xcodebuild test-without-building -destination id=BBBB-2222
  950     1   950 /Applications/Xcode.app/Contents/Applications/DeviceHub.app/Contents/MacOS/DeviceHub -psn_0
";

    #[test]
    fn a_device_is_told_by_the_process_that_is_its_boot() {
        let ps = Processes::parse(PS);
        assert_eq!(ps.root(Platform::Ios, "AAAA-1111", "iPhone"), Some(500));
        assert_eq!(ps.root(Platform::Ios, "CCCC-3333", "iPhone"), None);
        // The launcher, not the qemu it starts.
        assert_eq!(
            ps.root(Platform::Android, "emulator-5554", "Pixel_9"),
            Some(700)
        );
        // Before boot an emulator is known by its AVD name alone.
        assert_eq!(ps.root(Platform::Android, "Pixel_9", "Pixel_9"), Some(700));
        assert_eq!(ps.root(Platform::Android, "emulator-5556", "Pixel_9"), None);
    }

    #[test]
    fn use_is_another_process_naming_the_device() {
        let ps = Processes::parse(PS);
        // A simulator's own processes name it; they aren't use.
        assert!(!ps.names_elsewhere("AAAA-1111", Some(500), Some(650), 1));
        // Xcode's test runner for an agent is.
        assert!(ps.names_elsewhere("BBBB-2222", Some(600), Some(650), 1));
        // This app's hub streaming it isn't; another process would be.
        assert!(!ps.names_elsewhere("emulator-5554", Some(700), Some(650), 1));
        assert!(ps.names_elsewhere("emulator-5554", Some(700), None, 1));
        assert!(ps.simulator_app_open());
        assert!(!Processes::parse("1 0 1 /sbin/launchd\n").simulator_app_open());
    }

    fn quiet() -> Signs {
        Signs {
            same_boot: true,
            viewers: 0,
            in_app: IDLE_DEVICE,
            agent: None,
            named_elsewhere: false,
            simulator_app: false,
        }
    }

    #[test]
    fn only_a_device_quiet_on_every_count_shuts_down() {
        assert_eq!(verdict(&quiet()), Verdict::ShutDown);
        assert_eq!(
            verdict(&Signs {
                agent: Some(IDLE_DEVICE),
                ..quiet()
            }),
            Verdict::ShutDown
        );
        let minute = Duration::from_secs(60);
        for (busy, why) in [
            (
                Signs {
                    viewers: 1,
                    ..quiet()
                },
                "watched",
            ),
            (
                Signs {
                    in_app: IDLE_DEVICE - minute,
                    ..quiet()
                },
                "used in MonoCode",
            ),
            (
                Signs {
                    agent: Some(minute),
                    ..quiet()
                },
                "driven by an agent",
            ),
            (
                Signs {
                    named_elsewhere: true,
                    ..quiet()
                },
                "used by another process",
            ),
            (
                Signs {
                    simulator_app: true,
                    ..quiet()
                },
                "open in Simulator",
            ),
        ] {
            assert_eq!(verdict(&busy), Verdict::Keep, "{why}");
        }
        assert_eq!(
            verdict(&Signs {
                same_boot: false,
                ..quiet()
            }),
            Verdict::Forget,
            "rebooted by someone else"
        );
    }

    #[test]
    fn agent_runs_are_read_from_the_launchers_marks() {
        let dir = std::env::temp_dir().join(format!("monocode-uses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        assert_eq!(agent_quiet_in(&dir, "AAAA", now), None);
        std::fs::write(dir.join("AAAA"), "").unwrap();
        let later = now + Duration::from_secs(120);
        let quiet = agent_quiet_in(&dir, "AAAA", later).unwrap();
        assert!(quiet >= Duration::from_secs(119), "{quiet:?}");
        // A run that named no device counts for every device.
        std::fs::write(dir.join(toolchain::ANY_DEVICE), "").unwrap();
        assert!(agent_quiet_in(&dir, "BBBB", later).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_app_use_and_viewers_count_per_device() {
        let id = "test-device-in-app";
        viewer(id, true);
        let (viewers, quiet) = in_app(id);
        assert_eq!(viewers, 1);
        assert!(quiet < Duration::from_secs(5));
        viewer(id, false);
        assert_eq!(in_app(id).0, 0);
        assert_eq!(in_app("never-touched").0, 0);
    }

    /// Reads this Mac's processes, changing nothing, and prints what the
    /// idle check would see for each booted simulator:
    /// `cargo test --lib live_processes -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads this Mac's processes and booted simulators"]
    fn live_processes() {
        let ps = Processes::read().expect("ps runs");
        let output = Command::new("/usr/bin/xcrun")
            .args(["simctl", "list", "devices", "booted", "--json"])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let udids: Vec<String> = value["devices"]
            .as_object()
            .into_iter()
            .flat_map(|runtimes| runtimes.values())
            .flat_map(|devices| devices.as_array().cloned().unwrap_or_default())
            .filter_map(|device| device["udid"].as_str().map(str::to_owned))
            .collect();
        eprintln!(
            "{} processes, Simulator.app or DeviceHub open: {}",
            ps.0.len(),
            ps.simulator_app_open()
        );
        for udid in &udids {
            let root = ps.root(Platform::Ios, udid, "");
            eprintln!(
                "{udid}: boot process {root:?}, named elsewhere {}",
                ps.names_elsewhere(udid, root, None, std::process::id())
            );
            assert!(root.is_some(), "a booted simulator has its launchd_sim");
        }
    }
}
