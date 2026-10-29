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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::paths::Paths;
use crate::tui::data::{self, Snapshot};

/// A snapshot from the poll thread, with the UI epoch it was taken under.
pub struct Polled {
    pub epoch: u64,
    pub snapshot: Snapshot,
}

/// Handle to the running poll thread. Dropping it stops the thread and waits
/// for any in-flight load, so nothing outlives the TUI's hold on the terminal.
pub struct Poller {
    epoch: Arc<AtomicU64>,
    stop: Option<mpsc::Sender<()>>,
    rx: mpsc::Receiver<Polled>,
    handle: Option<JoinHandle<()>>,
}

impl Poller {
    /// Start polling every `interval`. The first load runs one interval in,
    /// since the caller has just loaded the initial snapshot itself.
    // spec: TUI-86
    pub fn start(paths: &Paths, interval: Duration) -> Self {
        Self::start_with(interval, {
            let paths = paths.clone();
            move || data::try_poll(&paths)
        })
    }

    /// [`Poller::start`] with the load supplied, so tests can drive it.
    fn start_with<F>(interval: Duration, mut load: F) -> Self
    where
        F: FnMut() -> Option<Snapshot> + Send + 'static,
    {
        let epoch = Arc::new(AtomicU64::new(0));
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
                // try_poll skips (None) while a writer holds the lock (TUI-25).
                let Some(snapshot) = load() else { continue };
                if let Err(mpsc::TrySendError::Disconnected(_)) =
                    tx.try_send(Polled { epoch, snapshot })
                {
                    return;
                }
            }
        });
        Self {
            epoch,
            stop: Some(stop_tx),
            rx,
            handle: Some(handle),
        }
    }

    /// Publish the UI's current epoch, read by the next load to begin.
    pub fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
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

    #[test]
    fn a_slow_load_does_not_block_the_caller() {
        // spec: TUI-86 -- the load runs off the calling thread, so asking for
        // the latest snapshot returns at once while a load is still running.
        let p = Poller::start_with(Duration::from_millis(1), || {
            std::thread::sleep(Duration::from_millis(500));
            Some(snapshot("slow"))
        });
        let start = Instant::now();
        assert!(p.latest(0).is_none());
        assert!(start.elapsed() < Duration::from_millis(100));
        assert_eq!(wait_latest(&p, 0), Some(snapshot("slow")));
    }

    #[test]
    fn a_snapshot_from_an_earlier_epoch_is_dropped() {
        // spec: TUI-86 -- a load begun before the UI applied an action's own
        // snapshot (and bumped its epoch) must not revert the tree.
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let p = Poller::start_with(Duration::from_millis(1), move || {
            go_rx.recv().ok()?;
            Some(snapshot("stale"))
        });
        // Let the first load start under epoch 0, then bump before it lands.
        std::thread::sleep(Duration::from_millis(50));
        p.set_epoch(1);
        go_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(p.latest(1).is_none(), "an epoch-0 load must be dropped");
        // The next load starts under epoch 1 and is kept.
        go_tx.send(()).unwrap();
        assert_eq!(wait_latest(&p, 1), Some(snapshot("stale")));
        // Unblock the next load so dropping the poller can join it.
        drop(go_tx);
    }

    #[test]
    fn a_skipped_load_sends_nothing_and_dropping_stops_the_thread() {
        // spec: TUI-86 TUI-25 -- a load skipped under a writer's lock (None)
        // delivers nothing; dropping the poller ends the thread.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let p = Poller::start_with(Duration::from_millis(1), move || {
            seen.fetch_add(1, Ordering::SeqCst);
            None
        });
        std::thread::sleep(Duration::from_millis(30));
        assert!(p.latest(0).is_none());
        drop(p);
        let after = calls.load(Ordering::SeqCst);
        assert!(after > 0, "the load ran");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no load after drop");
    }
}
