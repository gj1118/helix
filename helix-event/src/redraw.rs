//! Signals that control when/if the editor redraws

use std::future::Future;

use parking_lot::{RwLock, RwLockReadGuard};
use tokio::sync::Notify;

use crate::runtime_local;

runtime_local! {
    /// A `Notify` instance that can be used to (asynchronously) request
    /// the editor to render a new frame.
    static REDRAW_NOTIFY: Notify = Notify::const_new();

    /// A `RwLock` that prevents the next frame from being
    /// drawn until an exclusive (write) lock can be acquired.
    /// This allows asynchronous tasks to acquire `non-exclusive`
    /// locks (read) to prevent the next frame from being drawn
    /// until a certain computation has finished.
    static RENDER_LOCK: RwLock<()> = RwLock::new(());
}

pub type RenderLockGuard = RwLockReadGuard<'static, ()>;

/// Requests that the editor is redrawn. The redraws are debounced (currently to
/// 30FPS) so this can be called many times without causing a ton of frames to
/// be rendered.
pub fn request_redraw() {
    REDRAW_NOTIFY.notify_one();
}

/// In integration tests each runtime has its own redraw notifier,
/// so the returned function enters the caller's runtime first. Made outside
/// of a runtime (a unit test building a picker), it requests the redraw of
/// whatever runtime it is called in, or of none.
pub fn redraw_requester() -> impl Fn() + Send + Sync + 'static {
    #[cfg(feature = "integration_test")]
    {
        let runtime = tokio::runtime::Handle::try_current().ok();
        move || {
            let _guard = runtime.as_ref().map(|runtime| runtime.enter());
            request_redraw();
        }
    }
    #[cfg(not(feature = "integration_test"))]
    request_redraw
}

/// Returns a future that will yield once a redraw has been asynchronously
/// requested using [`request_redraw`].
pub fn redraw_requested() -> impl Future<Output = ()> {
    REDRAW_NOTIFY.notified()
}

/// Wait until all locks acquired with [`lock_frame`] have been released.
/// This function is called before rendering and is intended to allow the frame
/// to wait for async computations that should be included in the current frame.
pub fn start_frame() {
    drop(RENDER_LOCK.write());
    // exhaust any leftover redraw notifications
    let notify = REDRAW_NOTIFY.notified();
    tokio::pin!(notify);
    notify.enable();
}

/// Acquires the render lock which will prevent the next frame from being drawn
/// until the returned guard is dropped.
pub fn lock_frame() -> RenderLockGuard {
    RENDER_LOCK.read()
}

/// A zero sized type that requests a redraw via [request_redraw] when the type [Drop]s.
pub struct RequestRedrawOnDrop;

impl Drop for RequestRedrawOnDrop {
    fn drop(&mut self) {
        request_redraw();
    }
}

#[cfg(test)]
mod test {
    /// Outside of a runtime (with or without the `integration_test` feature), the
    /// requester is made and called without a panic.
    #[test]
    fn a_requester_works_outside_of_a_runtime() {
        let request = super::redraw_requester();
        request();
        // and from inside one, for its runtime
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let request = runtime.block_on(async { super::redraw_requester() });
        request();
        let requested: bool = runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                super::redraw_requested(),
            )
            .await
            .is_ok()
        });
        assert!(requested);
    }
}
