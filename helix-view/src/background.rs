//! Work too big to do between two frames (the diff of a long file), done on a thread of its
//! own: the editor goes on drawing and taking keys, and picks the result up once it is there.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// What a [`Background`] work gives: its result, or the panic it ended with.
pub type Outcome<T> = std::thread::Result<T>;

/// Work running on a thread of its own (see the module). Cloning it shares the work.
pub struct Background<T> {
    slot: Arc<(Mutex<Option<Outcome<T>>>, Condvar)>,
}

impl<T> Clone for Background<T> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
        }
    }
}

impl<T> std::fmt::Debug for Background<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let done = self
            .slot
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("Background").field("done", &done).finish()
    }
}

impl<T: Send + 'static> Background<T> {
    /// Starts `work` on a thread of its own; once done, the editor is asked to draw again (to
    /// pick the result up). Run here, right away, if no thread can be started.
    pub fn spawn(work: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot: Arc<(Mutex<Option<Outcome<T>>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        // whoever takes the work out runs it: the thread, or this one when there is none
        let work = Arc::new(Mutex::new(Some(work)));
        let finish = |slot: &(Mutex<Option<Outcome<T>>>, Condvar), work: Option<_>| {
            let Some(work) = work else {
                return;
            };
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(work));
            *slot.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            slot.1.notify_all();
            helix_event::request_redraw();
        };
        let started = std::thread::Builder::new()
            .name("helix-diff".to_string())
            .spawn({
                let (slot, work) = (Arc::clone(&slot), Arc::clone(&work));
                move || {
                    finish(
                        &slot,
                        work.lock().unwrap_or_else(PoisonError::into_inner).take(),
                    )
                }
            });
        if let Err(err) = started {
            log::error!("could not start a thread for a diff, worked out here: {err}");
            finish(
                &slot,
                work.lock().unwrap_or_else(PoisonError::into_inner).take(),
            );
        }
        Self { slot }
    }

    /// The outcome, once done, waiting for it up to `wait`; taken: later calls give `None`.
    pub fn take(&self, wait: Duration) -> Option<Outcome<T>> {
        let (slot, done) = &*self.slot;
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() && !wait.is_zero() {
            slot = done
                .wait_timeout_while(slot, wait, |slot| slot.is_none())
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        slot.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outcome_is_picked_up_once_done() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let work = Background::spawn(move || {
            wait.recv().unwrap();
            42_u32
        });
        // not done: nothing yet, even after waiting a little
        assert!(work.take(Duration::ZERO).is_none());
        assert!(work.take(Duration::from_millis(5)).is_none());
        go.send(()).unwrap();
        let outcome: u32 = work.take(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(outcome, 42_u32);
        // taken
        assert!(work.take(Duration::ZERO).is_none());
    }

    #[test]
    fn a_panic_is_an_outcome_too() {
        let work = Background::spawn(|| -> u32 { panic!("on purpose") });
        let outcome: Option<Outcome<u32>> = work.take(Duration::from_secs(10));
        assert!(outcome.is_some_and(|outcome| outcome.is_err()));
    }
}
