//! Keeps the Simulator window from opening over MonoCode.
//!
//! `expo run:ios`, `react-native run-ios` and Xcode open Simulator (DeviceHub
//! since Xcode 27) to show the app they launched. With Devices on, MonoCode
//! hides that window as it appears and shows the simulator in the Device tab.
//! Hiding leaves the simulators running; quitting could shut them down.
use monocode_core::device::{Device, DeviceList, Platform};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// DeviceHub (Xcode 27) and Simulator (Xcode 26 and older).
const BUNDLES: [&str; 2] = ["com.apple.dt.Devices", "com.apple.iphonesimulator"];

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The person asked to see the window: it's left alone until Simulator quits.
/// (DeviceHub reports no process id, so this can't name one instance.)
static ALLOWED: AtomicBool = AtomicBool::new(false);

/// A Simulator window MonoCode just hid.
#[derive(Clone, Debug)]
pub(crate) struct Hidden {
    /// The simulator Simulator was opened for, when it was launched with
    /// `-CurrentDeviceUDID` (Expo does this).
    pub(crate) udid: Option<String>,
}

pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Shows the window MonoCode hid, and stops hiding it until Simulator quits.
pub(crate) fn show() {
    ALLOWED.store(true, Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    mac::show();
}

/// Windows hidden from now on. Starts watching the first time.
pub(crate) fn hidden() -> async_channel::Receiver<Hidden> {
    static CHANNEL: OnceLock<async_channel::Receiver<Hidden>> = OnceLock::new();
    CHANNEL
        .get_or_init(|| {
            let (tx, rx) = async_channel::unbounded();
            #[cfg(target_os = "macos")]
            mac::watch(tx);
            #[cfg(not(target_os = "macos"))]
            drop(tx);
            rx
        })
        .clone()
}

/// The simulator to show for a hidden window: the one it was opened for, else
/// the one on screen if it still runs, else the first one running.
pub(crate) fn pick<'a>(
    list: &'a DeviceList,
    udid: Option<&str>,
    shown: Option<&str>,
) -> Option<&'a Device> {
    let running = || {
        list.devices
            .iter()
            .filter(|d| d.platform == Platform::Ios && d.booted && d.supported && !d.physical)
    };
    udid.and_then(|id| running().find(|d| d.id == id))
        .or_else(|| shown.and_then(|id| running().find(|d| d.id == id)))
        .or_else(|| running().next())
}

/// Whether to hide this app's window now.
fn should_hide(bundle: Option<&str>) -> bool {
    ENABLED.load(Ordering::Relaxed) && !ALLOWED.load(Ordering::Relaxed) && is_simulator(bundle)
}

fn is_simulator(bundle: Option<&str>) -> bool {
    bundle.is_some_and(|id| BUNDLES.contains(&id))
}

/// The `-CurrentDeviceUDID` argument in a process's `KERN_PROCARGS2` block:
/// argc, the executable's path, padding, then the arguments, each ending in
/// NUL.
fn device_arg(procargs: &[u8]) -> Option<String> {
    let argc = i32::from_ne_bytes(procargs.get(..4)?.try_into().ok()?);
    let mut parts = procargs[4..].split(|b| *b == 0).filter(|p| !p.is_empty());
    parts.next()?;
    let args: Vec<&[u8]> = parts.take(usize::try_from(argc).ok()?).collect();
    args.windows(2)
        .find(|pair| pair[0] == b"-CurrentDeviceUDID")
        .and_then(|pair| std::str::from_utf8(pair[1]).ok())
        .map(str::to_owned)
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{ALLOWED, BUNDLES, Hidden, device_arg, is_simulator, should_hide};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplicationActivationOptions, NSRunningApplication, NSWorkspace,
        NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
        NSWorkspaceDidLaunchApplicationNotification,
        NSWorkspaceDidTerminateApplicationNotification,
    };
    use objc2_foundation::{NSNotification, NSOperationQueue, NSString};
    use std::{ptr::NonNull, sync::atomic::Ordering};

    #[derive(Clone, Copy)]
    enum Seen {
        Launched,
        Activated,
        Quit,
    }

    pub(super) fn watch(tx: async_channel::Sender<Hidden>) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::mainQueue();
        // Launching also activates. Activation is watched too because Expo
        // and Xcode bring an open Simulator forward again for each run.
        for (name, seen) in [
            (
                unsafe { NSWorkspaceDidLaunchApplicationNotification },
                Seen::Launched,
            ),
            (
                unsafe { NSWorkspaceDidActivateApplicationNotification },
                Seen::Activated,
            ),
            (
                unsafe { NSWorkspaceDidTerminateApplicationNotification },
                Seen::Quit,
            ),
        ] {
            let tx = tx.clone();
            let block = RcBlock::new(move |note: NonNull<NSNotification>| {
                let Some(app) = running_app(unsafe { note.as_ref() }) else {
                    return;
                };
                let bundle = app.bundleIdentifier().map(|id| id.to_string());
                if let Seen::Quit = seen {
                    if is_simulator(bundle.as_deref()) {
                        ALLOWED.store(false, Ordering::Relaxed);
                    }
                    return;
                }
                if !should_hide(bundle.as_deref()) {
                    return;
                }
                app.hide();
                let udid = match seen {
                    Seen::Launched => launch_device(&app),
                    _ => None,
                };
                let _ = tx.try_send(Hidden { udid });
            });
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&queue),
                    &block,
                )
            };
            // Watched for the life of the app.
            std::mem::forget(token);
        }
    }

    fn running_app(note: &NSNotification) -> Option<Retained<NSRunningApplication>> {
        let info = note.userInfo()?;
        let app = info.objectForKey(unsafe { NSWorkspaceApplicationKey })?;
        app.downcast::<NSRunningApplication>().ok()
    }

    /// The simulator a just-launched Simulator was opened for.
    pub(super) fn launch_device(app: &NSRunningApplication) -> Option<String> {
        let pid = match app.processIdentifier() {
            pid if pid > 0 => pid,
            // DeviceHub reports -1: find it by its executable.
            _ => pid_of(&app.executableURL()?.path()?.to_string())?,
        };
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        let read = |buf: *mut libc::c_void, size: &mut libc::size_t, mib: &mut [i32; 3]| unsafe {
            libc::sysctl(mib.as_mut_ptr(), 3, buf, size, std::ptr::null_mut(), 0) == 0
        };
        if !read(std::ptr::null_mut(), &mut size, &mut mib) {
            return None;
        }
        let mut buf = vec![0u8; size];
        if !read(buf.as_mut_ptr().cast(), &mut size, &mut mib) {
            return None;
        }
        buf.truncate(size);
        device_arg(&buf)
    }

    /// The newest process running this executable.
    pub(super) fn pid_of(executable: &str) -> Option<i32> {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let mut pids = vec![0i32; usize::try_from(count).ok()? + 32];
        let bytes = i32::try_from(pids.len() * size_of::<i32>()).ok()?;
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(usize::try_from(count).ok()?);
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        pids.into_iter()
            .filter(|&pid| {
                let len =
                    unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
                len > 0 && &path[..len as usize] == executable.as_bytes()
            })
            .max()
    }

    pub(super) fn show() {
        for bundle in BUNDLES {
            let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(bundle),
            );
            for app in apps.iter() {
                app.unhide();
                app.activateWithOptions(NSApplicationActivationOptions::empty());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procargs(exe: &str, args: &[&str]) -> Vec<u8> {
        let mut out = (args.len() as i32).to_ne_bytes().to_vec();
        out.extend_from_slice(exe.as_bytes());
        out.extend_from_slice(&[0, 0, 0]);
        for arg in args {
            out.extend_from_slice(arg.as_bytes());
            out.push(0);
        }
        out.extend_from_slice(b"HOME=/Users/me\0");
        out
    }

    #[test]
    fn reads_the_simulator_expo_opened() {
        let block = procargs(
            "/Applications/Xcode.app/Contents/Applications/DeviceHub.app/Contents/MacOS/DeviceHub",
            &["DeviceHub", "-CurrentDeviceUDID", "DBBFF74F-0716"],
        );
        assert_eq!(device_arg(&block).as_deref(), Some("DBBFF74F-0716"));
    }

    #[test]
    fn ignores_the_environment_and_plain_launches() {
        assert_eq!(device_arg(&procargs("/x/DeviceHub", &["DeviceHub"])), None);
        // The flag's value would be in the environment, past argc.
        let mut block = procargs("/x/DeviceHub", &["DeviceHub", "-CurrentDeviceUDID"]);
        block.extend_from_slice(b"ABC\0");
        assert_eq!(device_arg(&block), None);
        assert_eq!(device_arg(&[1, 0]), None);
    }

    fn device(id: &str, platform: Platform, booted: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            version: String::new(),
            platform,
            booted,
            physical: false,
            supported: true,
        }
    }

    #[test]
    fn picks_the_simulator_the_window_was_for() {
        let list = DeviceList {
            devices: vec![
                device("emulator-5554", Platform::Android, true),
                device("A", Platform::Ios, true),
                device("B", Platform::Ios, true),
                device("C", Platform::Ios, false),
            ],
            ..DeviceList::default()
        };
        let id = |d: Option<&Device>| d.map(|d| d.id.clone());
        assert_eq!(id(pick(&list, Some("B"), Some("A"))).as_deref(), Some("B"));
        assert_eq!(id(pick(&list, None, Some("B"))).as_deref(), Some("B"));
        assert_eq!(id(pick(&list, Some("C"), Some("C"))).as_deref(), Some("A"));
        assert_eq!(id(pick(&list, None, None)).as_deref(), Some("A"));
        let none = DeviceList {
            devices: vec![device("C", Platform::Ios, false)],
            ..DeviceList::default()
        };
        assert!(pick(&none, None, None).is_none());
    }

    /// DeviceHub reports pid -1 to AppKit; its launch arguments are found
    /// through its executable instead.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Needs DeviceHub or Simulator open"]
    fn live_finds_the_running_simulator_app() {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;
        let app = BUNDLES
            .iter()
            .flat_map(|id| {
                NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
                    id,
                ))
            })
            .next()
            .expect("Simulator is not open");
        let path = app.executableURL().unwrap().path().unwrap().to_string();
        let pid = mac::pid_of(&path);
        println!(
            "AppKit pid {}, found {pid:?} for {path}",
            app.processIdentifier()
        );
        assert!(pid.is_some_and(|pid| pid > 0));
        println!("opened for {:?}", mac::launch_device(&app));
    }

    #[test]
    fn hides_only_simulator_apps_while_on() {
        set_enabled(false);
        assert!(!should_hide(Some("com.apple.dt.Devices")));
        set_enabled(true);
        assert!(should_hide(Some("com.apple.dt.Devices")));
        assert!(should_hide(Some("com.apple.iphonesimulator")));
        assert!(!should_hide(Some("com.apple.Safari")));
        assert!(!should_hide(None));
        ALLOWED.store(true, Ordering::Relaxed);
        assert!(!should_hide(Some("com.apple.dt.Devices")));
        ALLOWED.store(false, Ordering::Relaxed);
        set_enabled(false);
    }
}
