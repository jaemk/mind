//! Background refresh poll (TUI-15, TUI-86).
//!
//! The poll loads registry, manifest, and catalog, which takes hundreds of
//! milliseconds on a large or linked catalog. Running it on the input thread
//! stalled every key behind it, so it runs here on its own thread and hands
//! each snapshot to the event loop over a channel.
//!
//! Each snapshot is tagged with the epoch the UI had published when the load
//! began. The UI bumps its epoch whenever it applies a snapshot of its own (an
//! action's result), so a load that read the state from before that action is
//! recognised and dropped instead of reverting the tree.
//!
//! The UI also publishes the sources it has on screen. A load reads those
//! fresh and reuses the others' live parts until they age out (TUI-87).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::paths::Paths;
use crate::tui::data::{self, Snapshot};

/// A snapshot from the poll thread, with the UI epoch it was taken under.
pub struct Polled {
    pub epoch: u64,
    pub snapshot: Snapshot,
}

/// The sources the UI has on screen, read fresh by each load (TUI-87).
type OnScreen = Arc<Mutex<HashSet<String>>>;

/// Handle to the running poll thread. Dropping it stops the thread and waits
/// for any in-flight load, so nothing outlives the TUI's hold on the terminal.
pub struct Poller {
    epoch: Arc<AtomicU64>,
    on_screen: OnScreen,
    stop: Option<mpsc::Sender<()>>,
    rx: mpsc::Receiver<Polled>,
    handle: Option<JoinHandle<()>>,
}

impl Poller {
    /// Start polling every `interval`. The first load runs one interval in,
    /// since the caller has just loaded the initial snapshot itself.
    // spec: TUI-86 TUI-87
    pub fn start(paths: &Paths, interval: Duration) -> Self {
        Self::start_with(interval, {
            let paths = paths.clone();
            move |on_screen| data::try_poll_focused(&paths, on_screen)
        })
    }

    /// [`Poller::start`] with the load supplied, so tests can drive it. The
    /// load is given the sources last published by [`Poller::set_on_screen`].
    fn start_with<F>(interval: Duration, mut load: F) -> Self
    where
        F: FnMut(&HashSet<String>) -> Option<Snapshot> + Send + 'static,
    {
        let epoch = Arc::new(AtomicU64::new(0));
        let on_screen: OnScreen = Arc::default();
        let shown = on_screen.clone();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        // One slot: while the UI is busy (an action runs, or the TUI is
        // suspended for one) loads that find it full are dropped rather than
        // queued, so a long action does not pile up snapshots.
        let (tx, rx) = mpsc::sync_channel(1);
        let shared = epoch.clone();
        let handle = std::thread::spawn(move || {
            // Wakes on the interval; a stop message or a dropped sender ends it.
            while let Err(mpsc::RecvTimeoutError::Timeout) = stop_rx.recv_timeout(interval) {
                let epoch = shared.load(Ordering::Acquire);
                let on_screen = shown.lock().unwrap_or_else(|e| e.into_inner()).clone();
                // try_poll skips (None) while a writer holds the lock (TUI-25).
                let Some(snapshot) = load(&on_screen) else {
                    continue;
                };
                if let Err(mpsc::TrySendError::Disconnected(_)) =
                    tx.try_send(Polled { epoch, snapshot })
                {
                    return;
                }
            }
        });
        Self {
            epoch,
            on_screen,
            stop: Some(stop_tx),
            rx,
            handle: Some(handle),
        }
    }

    /// Publish the UI's current epoch, read by the next load to begin.
    pub fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
    }

    /// Publish the sources with a row on screen, read by the next load to
    /// begin (TUI-87).
    pub fn set_on_screen(&self, sources: HashSet<String>) {
        *self.on_screen.lock().unwrap_or_else(|e| e.into_inner()) = sources;
    }

    /// The newest snapshot taken under `epoch`, if any arrived. Older ones and
    /// any taken under an earlier epoch are discarded.
    // spec: TUI-86
    pub fn latest(&self, epoch: u64) -> Option<Snapshot> {
        self.rx
            .try_iter()
            .filter(|p| p.epoch == epoch)
            .last()
            .map(|p| p.snapshot)
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn snapshot(name: &str) -> Snapshot {
        Snapshot {
            source_names: vec![name.to_string()],
            ..Default::default()
        }
    }

    /// Wait (bounded) for the poller to deliver something under `epoch`.
    fn wait_latest(p: &Poller, epoch: u64) -> Option<Snapshot> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(s) = p.latest(epoch) {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// A load that reports each start on `started` and then waits for a
    /// release on `go` (none once `go`'s sender is dropped), so a test steps
    /// the poll thread without sleeping on timing.
    fn gated_load(
        started: mpsc::Sender<()>,
        go: mpsc::Receiver<()>,
        name: &'static str,
    ) -> impl FnMut(&HashSet<String>) -> Option<Snapshot> + Send + 'static {
        move |_| {
            let _ = started.send(());
            go.recv().ok()?;
            Some(snapshot(name))
        }
    }

    #[test]
    fn a_slow_load_does_not_block_the_caller() {
        // spec: TUI-86 -- the load runs off the calling thread, so asking for
        // the latest snapshot returns at once while a load is still running.
        let (started_tx, started_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        let p = Poller::start_with(
            Duration::from_millis(1),
            gated_load(started_tx, go_rx, "slow"),
        );
        started_rx.recv().unwrap();
        // The load is blocked mid-flight; a blocking `latest` would hang here.
        assert!(p.latest(0).is_none());
        go_tx.send(()).unwrap();
        assert_eq!(wait_latest(&p, 0), Some(snapshot("slow")));
        // Unblock the next load so dropping the poller can join it.
        drop(go_tx);
    }

    #[test]
    fn a_snapshot_from_an_earlier_epoch_is_dropped() {
        // spec: TUI-86 -- a load begun before the UI applied an action's own
        // snapshot (and bumped its epoch) must not revert the tree.
        let (started_tx, started_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        let p = Poller::start_with(
            Duration::from_millis(1),
            gated_load(started_tx, go_rx, "stale"),
        );
        // The first load has read epoch 0; bump before it lands.
        started_rx.recv().unwrap();
        p.set_epoch(1);
        go_tx.send(()).unwrap();
        // Once the next load starts, the first one has been sent.
        started_rx.recv().unwrap();
        assert!(p.latest(1).is_none(), "an epoch-0 load must be dropped");
        // The load now running began under epoch 1 and is kept.
        go_tx.send(()).unwrap();
        assert_eq!(wait_latest(&p, 1), Some(snapshot("stale")));
        drop(go_tx);
    }

    #[test]
    fn each_load_is_given_the_sources_on_screen() {
        // spec: TUI-87
        let p = Poller::start_with(Duration::from_millis(1), |on_screen| {
            let mut names: Vec<String> = on_screen.iter().cloned().collect();
            names.sort();
            Some(Snapshot {
                source_names: names,
                ..Default::default()
            })
        });
        p.set_on_screen(HashSet::from(["b".to_string(), "a".to_string()]));
        let start = Instant::now();
        let mut got = None;
        while start.elapsed() < Duration::from_secs(5) {
            got = p.latest(0).filter(|s| !s.source_names.is_empty());
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            got.map(|s| s.source_names),
            Some(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn a_skipped_load_sends_nothing_and_dropping_stops_the_thread() {
        // spec: TUI-86 TUI-25 -- a load skipped under a writer's lock (None)
        // delivers nothing; dropping the poller ends the thread.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let p = Poller::start_with(Duration::from_millis(1), move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = started_tx.send(());
            None
        });
        // Two loads have finished once the third starts.
        for _ in 0..3 {
            started_rx.recv().unwrap();
        }
        assert!(p.latest(0).is_none(), "a skipped load sends nothing");
        // Drop joins the thread, so the count is final once it returns.
        drop(p);
        let after = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no load after drop");
    }
}
