//! Bounded remote-session cleanup ownership.
//!
//! The SDK call is injected as a Future so production and portable tests share
//! exactly the same cancellation, timeout, Drop and explicit-shutdown behavior.
//! Normal application paths await cleanup. Drop can only request best-effort
//! cleanup while the Tokio runtime is alive; it never blocks an executor thread.
use crate::error::{AppError, Result};
use std::{future::Future, time::Duration};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::warn;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct SessionCleanup {
    trigger: Option<oneshot::Sender<()>>,
    transport: watch::Sender<Option<CancellationToken>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl SessionCleanup {
    pub(crate) fn new(cleanup: impl Future<Output = Result<()>> + Send + 'static) -> Self {
        let (trigger, mut receiver) = oneshot::channel();
        let (transport, mut updates) = watch::channel::<Option<CancellationToken>>(None);
        let task = tokio::spawn(async move {
            let mut closed = CancellationToken::new();
            loop {
                tokio::select! {
                    _=&mut receiver=>break,
                    _=closed.cancelled()=>break,
                    changed=updates.changed()=>{
                        if changed.is_err() {break;}
                        let replacement=updates.borrow_and_update().clone();
                        if let Some(token)=replacement {closed=token;}
                    }
                }
            }
            let result = tokio::time::timeout(CLEANUP_TIMEOUT, cleanup)
                .await
                .unwrap_or_else(|_| Err(AppError::Ssm("Remote SSM termination timed out".into())));
            if let Err(error) = &result {
                warn!("Remote SSM cleanup failed: {error}");
            }
            result
        });
        Self {
            trigger: Some(trigger),
            transport,
            task: Some(task),
        }
    }
    pub(crate) fn follow_transport(&self, closed: CancellationToken) {
        self.transport.send_replace(Some(closed));
    }
    pub(crate) fn request(&mut self) {
        if let Some(trigger) = self.trigger.take() {
            let _ = trigger.send(());
        }
    }
    pub(crate) async fn finish(mut self) -> Result<()> {
        self.request();
        if let Some(task) = self.task.take() {
            task.await.map_err(|error| {
                AppError::Ssm(format!("Remote SSM cleanup task failed: {error}"))
            })?
        } else {
            Ok(())
        }
    }
}
impl Drop for SessionCleanup {
    fn drop(&mut self) {
        // One bounded cleanup actor per created remote session. Dropping the
        // completion handle is intentional: aborting it would skip termination.
        self.request();
    }
}

/// Checking a listener must not open a real smux stream in another MFZ session.
/// This preflight is advisory; the production listener's bind is authoritative.
pub(crate) fn port_is_in_use(port: u16) -> bool {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[tokio::test]
    async fn preflight_never_connects_to_a_running_forwarder() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_is_in_use(port));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        drop(listener);
        assert!(!port_is_in_use(port));
    }
    #[tokio::test]
    async fn lease_defers_cleanup_until_requested_and_finishes_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let mut lease = SessionCleanup::new(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        lease.request();
        lease.request();
        lease.finish().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn drop_requests_cleanup_after_a_cancelled_or_failed_start() {
        let (called, received) = oneshot::channel();
        let lease = SessionCleanup::new(async move {
            called.send(()).unwrap();
            Ok(())
        });
        drop(lease);
        tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn unexpected_transport_exit_triggers_cleanup_without_manual_stop() {
        let (called, received) = oneshot::channel();
        let lease = SessionCleanup::new(async move {
            called.send(()).unwrap();
            Ok(())
        });
        let closed = CancellationToken::new();
        lease.follow_transport(closed.clone());
        closed.cancel();
        tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap();
        lease.finish().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn hung_cleanup_is_bounded_instead_of_stranding_a_background_task() {
        let lease = SessionCleanup::new(std::future::pending::<Result<()>>());
        let error = lease.finish().await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
    #[tokio::test]
    async fn cleanup_error_is_reported_to_explicit_shutdown() {
        let lease = SessionCleanup::new(async { Err(AppError::Ssm("test denial".into())) });
        assert!(lease
            .finish()
            .await
            .unwrap_err()
            .to_string()
            .contains("test denial"));
    }
}
