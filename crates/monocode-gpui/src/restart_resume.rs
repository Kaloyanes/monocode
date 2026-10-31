//! Turns cut short when MonoCode last stopped. Launch settles their rows before
//! any chat loads, then the shell continues the eligible ones once leftover
//! provider processes from the previous run are gone.
use monocode_core::store::{InFlight, Interrupted, Session, Store};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Pending {
    pub id: i64,
    pub turn: InFlight,
    pub interrupted: Interrupted,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
/// Settled but left to their owner to continue, such as a Mono's chat.
static OWNED: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static REAPED: AtomicBool = AtomicBool::new(false);

/// Before the first window: seal every interrupted turn and keep the ones a
/// relaunch may continue. Reaping runs off-thread; resumes wait for it.
pub(crate) fn prepare(store: &mut Store, sessions: &[Session]) {
    std::thread::spawn(|| {
        let reaped = monocode_core::orphans::reap();
        if reaped > 0 {
            eprintln!("Stopped {reaped} provider processes left by the previous run");
        }
        REAPED.store(true, Ordering::SeqCst);
    });
    let turns = store.in_flight().unwrap_or_else(|error| {
        eprintln!("Cannot read interrupted turns: {error}");
        Vec::new()
    });
    let mut pending = Vec::new();
    let mut owned = Vec::new();
    for (id, turn) in turns {
        // A Mono's own conversation is hidden from the sidebar list.
        let session = sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
            .or_else(|| store.mono_session(id).ok().flatten());
        let archived = session.as_ref().is_some_and(|session| session.archived);
        match session.map(|_| store.seal_interrupted_turn(id)) {
            Some(Ok(interrupted)) if !archived => {
                let turns = if turn.resume {
                    &mut pending
                } else {
                    &mut owned
                };
                turns.push(Pending {
                    id,
                    turn,
                    interrupted,
                });
            }
            Some(Err(error)) => eprintln!("Cannot settle interrupted turn {id}: {error}"),
            _ => {}
        }
        // A continuation records its own marker; an ineligible turn is done.
        if let Err(error) = store.clear_in_flight(id) {
            eprintln!("Cannot clear interrupted turn {id}: {error}");
        }
    }
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = pending;
    *OWNED.lock().unwrap_or_else(|e| e.into_inner()) = owned;
}

pub(crate) fn take() -> Vec<Pending> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The owner of these chats claims their interrupted turns, at most once.
pub(crate) fn take_owned(ids: &[i64]) -> Vec<Pending> {
    let mut owned = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    let (claimed, rest) = std::mem::take(&mut *owned)
        .into_iter()
        .partition(|turn| ids.contains(&turn.id));
    *owned = rest;
    claimed
}

pub(crate) fn reaped() -> bool {
    REAPED.load(Ordering::SeqCst)
}
