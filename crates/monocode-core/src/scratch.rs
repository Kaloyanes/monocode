//! Claude Code gives every session a temporary folder,
//! `<tmp>/claude-<uid>/<project>/<session id>/`, holding its `scratchpad/`
//! and background `tasks/` output, and never deletes it. Only a reboot
//! clears `/tmp`, so on a Mac that stays on the folders pile up by the
//! gigabyte. The sweep removes a session's folder once its thread has been
//! archived for a day, or once nothing in it has changed for two days, and
//! never while a MonoCode thread on that session is working.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// An archived thread's files outlive the archive this long, so undo keeps them.
pub const ARCHIVED_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// A folder nothing has written to for this long is left over.
pub const IDLE: Duration = Duration::from_secs(2 * 24 * 60 * 60);
/// How deep the newest-file walk looks; scratch trees are shallow.
const MAX_DEPTH: usize = 6;

/// One session's folder and the newest change anywhere inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub session: String,
    pub touched: SystemTime,
}

/// What MonoCode knows about a Claude session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thread {
    /// When its thread was archived; `None` while it is in the sidebar.
    pub archived_at: Option<SystemTime>,
    /// A turn is running, so its files are in use.
    pub working: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub folders: usize,
    pub bytes: u64,
    pub failed: usize,
}

/// Claude Code's own rule: `$CLAUDE_CODE_TMPDIR`, else `/tmp`, then
/// `claude-<uid>`. MonoCode passes its environment to the CLI, so both agree.
pub fn root() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let base = std::env::var_os("CLAUDE_CODE_TMPDIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        Some(base.join(format!("claude-{uid}")))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Every `<root>/<project>/<uuid>/` folder. Anything else, such as the shared
/// `bash-edit-diff/`, is left alone, and symlinks are never followed.
pub fn scan(root: &Path) -> Vec<Folder> {
    let mut folders = Vec::new();
    for project in dirs(root) {
        for session in dirs(&project) {
            let Some(name) = session.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if uuid::Uuid::try_parse(name).is_err() {
                continue;
            }
            let Some(touched) = newest(&session, 0) else {
                continue;
            };
            folders.push(Folder {
                session: name.to_owned(),
                path: session,
                touched,
            });
        }
    }
    folders
}

/// Real directories directly inside `dir`, symlinks excluded.
fn dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// The newest modification time of `path` or anything under it.
fn newest(path: &Path, depth: usize) -> Option<SystemTime> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let mut latest = meta.modified().ok()?;
    if meta.is_dir() && depth < MAX_DEPTH {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            if let Some(time) = newest(&entry.path(), depth + 1) {
                latest = latest.max(time);
            }
        }
    }
    Some(latest)
}

/// The folders to delete. A working thread's folder always stays; otherwise a
/// folder goes once its thread has been archived for [`ARCHIVED_GRACE`] or
/// nothing in it has changed for [`IDLE`].
pub fn pick<'a>(
    folders: &'a [Folder],
    threads: &HashMap<String, Thread>,
    now: SystemTime,
) -> Vec<&'a Folder> {
    let older =
        |time: SystemTime, limit: Duration| now.duration_since(time).is_ok_and(|age| age >= limit);
    folders
        .iter()
        .filter(|folder| {
            let thread = threads.get(&folder.session);
            if thread.is_some_and(|t| t.working) {
                return false;
            }
            thread
                .and_then(|t| t.archived_at)
                .is_some_and(|at| older(at, ARCHIVED_GRACE))
                || older(folder.touched, IDLE)
        })
        .collect()
}

/// Claude sessions MonoCode knows, keyed by provider session id. `working`
/// holds the thread ids with a turn running. Remote threads are left out:
/// their folders are on the other machine.
pub fn threads(
    store: &crate::store::Store,
    working: &HashSet<i64>,
) -> Result<HashMap<String, Thread>, String> {
    let archived = store.archive_times()?;
    let mut threads: HashMap<String, Thread> = HashMap::new();
    for session in store.sessions()? {
        if session.provider != "claude"
            || session.cwd.starts_with("remote://")
            || store.remote_session(session.id)?.is_some()
        {
            continue;
        }
        let Some(provider_id) = session.provider_id.filter(|id| !id.is_empty()) else {
            continue;
        };
        let archived_at = session.archived.then(|| {
            // An archive from before archive times were recorded counts as old.
            archived
                .get(&session.id)
                .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(*ms))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let thread = Thread {
            archived_at,
            working: working.contains(&session.id),
        };
        threads
            .entry(provider_id)
            .and_modify(|seen| *seen = merge(seen, &thread))
            .or_insert(thread);
    }
    Ok(threads)
}

/// Two threads on one Claude session (a fork, an import): the folder stays
/// while either is working or in the sidebar, and counts from the latest archive.
fn merge(a: &Thread, b: &Thread) -> Thread {
    Thread {
        working: a.working || b.working,
        archived_at: a.archived_at.zip(b.archived_at).map(|(a, b)| a.max(b)),
    }
}

/// Deletes each picked folder, carrying on past failures.
pub fn remove(folders: &[&Folder]) -> Swept {
    let mut swept = Swept::default();
    for folder in folders {
        let bytes = size(&folder.path, 0);
        match std::fs::remove_dir_all(&folder.path) {
            Ok(()) => {
                swept.folders += 1;
                swept.bytes += bytes;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                swept.failed += 1;
                eprintln!("Cannot remove {}: {error}", folder.path.display());
            }
        }
    }
    swept
}

fn size(path: &Path, depth: usize) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    if depth >= 32 {
        return 0;
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| size(&entry.path(), depth + 1))
                .sum()
        })
        .unwrap_or(0)
}

/// Scans `root` and removes what [`pick`] chooses. Blocking: run it off the
/// UI thread, without holding the store.
pub fn sweep(root: &Path, threads: &HashMap<String, Thread>, now: SystemTime) -> Swept {
    let folders = scan(root);
    remove(&pick(&folders, threads, now))
}

#[cfg(test)]
mod tests;
