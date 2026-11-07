//! Keep what a panic said. A bundled app's stderr is /dev/null, and a panic
//! that reaches an AppKit callback aborts there with its own frames already
//! unwound, so the system crash report names only the callback. The hook runs
//! before any unwinding, while the panicking stack still exists.
use std::{
    backtrace::Backtrace,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// One report per run that panicked; the oldest are removed first.
const MAX_REPORTS: usize = 20;
/// A thread that retries and panics in a loop must not fill the disk.
const MAX_PANICS_PER_RUN: usize = 8;

pub(crate) fn install(directory: PathBuf) {
    let path = directory.join(format!(
        "panic-{}-{}.txt",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        std::process::id()
    ));
    let recorded = AtomicUsize::new(0);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let count = recorded.fetch_add(1, Ordering::Relaxed);
        if count < MAX_PANICS_PER_RUN {
            let thread = std::thread::current();
            let report = format!(
                "{} Monocode Native {} thread '{}' {info}\n\n{}\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                env!("CARGO_PKG_VERSION"),
                thread.name().unwrap_or("<unnamed>"),
                Backtrace::force_capture(),
            );
            if let Err(error) = record(&path, &report, count == 0) {
                eprintln!("Could not save panic report: {error}");
            }
        }
        default_hook(info);
    }));
}

/// Appends, because a panic that reaches `extern "C"` panics a second time and
/// the first one is the report that matters.
fn record(path: &Path, report: &str, first_of_run: bool) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
        if first_of_run {
            prune(directory)?;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(report.as_bytes())
}

fn prune(directory: &Path) -> std::io::Result<()> {
    let mut reports = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("panic-"))
                && path.extension().is_some_and(|ext| ext == "txt")
        })
        .collect::<Vec<_>>();
    reports.sort();
    let excess = reports.len().saturating_sub(MAX_REPORTS - 1);
    for path in reports.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "monocode-panic-report-test-{}",
            monocode_core::mono::new_id()
        ))
    }

    #[test]
    fn a_second_panic_in_the_run_keeps_the_first() {
        let directory = directory();
        let path = directory.join("panic-20000101-000000-1.txt");
        record(&path, "the real panic\n", true).unwrap();
        record(&path, "panic in a function that cannot unwind\n", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "the real panic\npanic in a function that cannot unwind\n"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn keeps_only_the_newest_reports_and_nothing_else_is_touched() {
        let directory = directory();
        std::fs::create_dir_all(&directory).unwrap();
        for run in 0..MAX_REPORTS + 3 {
            std::fs::write(
                directory.join(format!("panic-20000101-{run:06}-1.txt")),
                "old",
            )
            .unwrap();
        }
        let other = directory.join("notes.txt");
        std::fs::write(&other, "mine").unwrap();
        let path = directory.join("panic-20990101-000000-1.txt");
        record(&path, "new", true).unwrap();
        let reports = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("panic-"))
            .count();
        assert_eq!(reports, MAX_REPORTS);
        assert!(path.exists() && other.exists());
        assert!(!directory.join("panic-20000101-000000-1.txt").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reports_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = directory();
        let path = directory.join("panic-20000101-000000-1.txt");
        record(&path, "secret", true).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
