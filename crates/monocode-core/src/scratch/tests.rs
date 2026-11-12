use super::*;

const A: &str = "0a4afbb6-eca9-48c2-b36b-1a63dd05413a";
const B: &str = "05b52243-99be-473a-85fd-9a8b5a6b6ea0";
const HOUR: Duration = Duration::from_secs(60 * 60);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn folder(session: &str, age: Duration) -> Folder {
    Folder {
        path: PathBuf::from(format!("/root/project/{session}")),
        session: session.into(),
        touched: now() - age,
    }
}

fn picked(folders: &[Folder], threads: &HashMap<String, Thread>) -> Vec<String> {
    pick(folders, threads, now())
        .into_iter()
        .map(|f| f.session.clone())
        .collect()
}

#[test]
fn an_idle_folder_goes_after_two_days_and_a_fresh_one_stays() {
    let folders = [folder(A, IDLE), folder(B, IDLE - HOUR)];
    assert_eq!(picked(&folders, &HashMap::new()), [A]);
}

#[test]
fn an_archived_thread_keeps_its_files_for_a_day_so_undo_works() {
    let folders = [folder(A, HOUR), folder(B, HOUR)];
    let threads = HashMap::from([
        (
            A.to_owned(),
            Thread {
                archived_at: Some(now() - ARCHIVED_GRACE),
                working: false,
            },
        ),
        (
            B.to_owned(),
            Thread {
                archived_at: Some(now() - HOUR),
                working: false,
            },
        ),
    ]);
    assert_eq!(picked(&folders, &threads), [A]);
}

#[test]
fn a_working_thread_keeps_its_folder_however_old() {
    let folders = [folder(A, IDLE * 10)];
    let threads = HashMap::from([(
        A.to_owned(),
        Thread {
            archived_at: Some(SystemTime::UNIX_EPOCH),
            working: true,
        },
    )]);
    assert!(picked(&folders, &threads).is_empty());
}

#[test]
fn a_clock_set_back_deletes_nothing() {
    let folders = [Folder {
        touched: now() + IDLE,
        ..folder(A, HOUR)
    }];
    assert!(picked(&folders, &HashMap::new()).is_empty());
}

#[test]
fn two_threads_on_one_session_keep_it_while_either_is_in_the_sidebar() {
    let old = Thread {
        archived_at: Some(SystemTime::UNIX_EPOCH),
        working: false,
    };
    let open = Thread::default();
    assert_eq!(merge(&old, &open).archived_at, None);
    let later = Thread {
        archived_at: Some(now()),
        working: false,
    };
    assert_eq!(merge(&old, &later).archived_at, Some(now()));
    assert!(
        merge(
            &open,
            &Thread {
                working: true,
                ..old
            }
        )
        .working
    );
}

fn set_age(path: &Path, age: Duration) {
    let time = SystemTime::now() - age;
    std::fs::File::options()
        .write(path.is_file())
        .read(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn scan_reads_the_newest_file_inside_and_skips_what_is_not_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("-Users-me-project");
    let old = project.join(A);
    let busy = project.join(B);
    std::fs::create_dir_all(old.join("scratchpad")).unwrap();
    std::fs::create_dir_all(busy.join("scratchpad/nested")).unwrap();
    std::fs::create_dir_all(project.join("not-a-session")).unwrap();
    std::fs::create_dir_all(tmp.path().join("bash-edit-diff")).unwrap();
    std::fs::write(old.join("scratchpad/shot.png"), b"png").unwrap();
    std::fs::write(busy.join("scratchpad/nested/log.txt"), b"log").unwrap();
    let week = IDLE * 3;
    for path in [
        old.join("scratchpad/shot.png"),
        old.join("scratchpad"),
        old.clone(),
        busy.join("scratchpad"),
        busy.clone(),
    ] {
        set_age(&path, week);
    }

    let mut folders = scan(tmp.path());
    folders.sort_by(|a, b| a.session.cmp(&b.session));
    assert_eq!(
        folders
            .iter()
            .map(|f| f.session.as_str())
            .collect::<Vec<_>>(),
        [B, A]
    );
    let age = |f: &Folder| SystemTime::now().duration_since(f.touched).unwrap();
    assert!(age(&folders[0]) < HOUR, "the nested log keeps {B} fresh");
    assert!(age(&folders[1]) >= IDLE);

    let swept = sweep(tmp.path(), &HashMap::new(), SystemTime::now());
    assert_eq!((swept.folders, swept.bytes, swept.failed), (1, 3, 0));
    assert!(!old.exists());
    assert!(busy.exists());
    assert!(project.join("not-a-session").exists());
    assert!(tmp.path().join("bash-edit-diff").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_session_is_never_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(root.join("project")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("project").join(A)).unwrap();
    std::os::unix::fs::symlink(&outside, root.join(B)).unwrap();

    let far_future = SystemTime::now() + IDLE * 100;
    let swept = sweep(&root, &HashMap::new(), far_future);
    assert_eq!(swept.folders, 0);
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn threads_come_from_local_claude_sessions_only() {
    let tmp = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(&tmp.path().join("native.db")).unwrap();
    let archived = store.create("claude", "/project").unwrap();
    store.provider_id(archived.id, A).unwrap();
    store.set_archived(archived.id, true).unwrap();
    let working = store.create("claude", "/project").unwrap();
    store.provider_id(working.id, B).unwrap();
    let codex = store.create("codex", "/project").unwrap();
    store.provider_id(codex.id, "codex-thread").unwrap();
    store.create("claude", "/project").unwrap();

    let threads = threads(&store, &HashSet::from([working.id])).unwrap();
    assert_eq!(threads.len(), 2);
    let at = threads[A].archived_at.unwrap();
    assert!(SystemTime::now().duration_since(at).unwrap() < HOUR);
    assert!(!threads[A].working);
    assert!(threads[B].working);
    assert_eq!(threads[B].archived_at, None);
}
