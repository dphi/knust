use super::*;
use crate::transport::{ConnectionStats, TransportState};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;
use tokio::time::{Duration, timeout};

struct ActiveSend(Arc<AtomicUsize>);
impl Drop for ActiveSend {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct BlockingConnection {
    entered: Notify,
    release: Semaphore,
    active: Arc<AtomicUsize>,
    sent: AtomicUsize,
}

#[async_trait::async_trait]
impl Connection for BlockingConnection {
    async fn send(&self, _frame: &[u8]) -> Result<()> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveSend(self.active.clone());
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        self.sent.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn recv(&self) -> Result<Vec<u8>> {
        std::future::pending().await
    }
    async fn close(&self) -> Result<()> {
        Ok(())
    }
    fn state(&self) -> TransportState {
        TransportState::Connected
    }
    fn stats(&self) -> ConnectionStats {
        ConnectionStats::default()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

async fn knx_without_cleanup() -> Knx {
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    })
    .await
    .unwrap();
    let handle = knx.cleanup_task.write().await.take().unwrap();
    handle.abort();
    let _ = handle.await;
    knx
}

async fn sending(knx: &Knx) -> Arc<BlockingConnection> {
    let connection = Arc::new(BlockingConnection {
        entered: Notify::new(),
        release: Semaphore::new(0),
        active: Arc::new(AtomicUsize::new(0)),
        sent: AtomicUsize::new(0),
    });
    knx.connection.send_replace(Some(connection.clone()));
    *knx.sending_task.write().await = Some(knx.start_telegram_sending_task());
    knx.send_telegram(&Telegram::group_write(
        RawGroupAddress::try_from_raw(1).unwrap(),
        vec![1],
    ))
    .await
    .unwrap();
    timeout(Duration::from_secs(1), connection.entered.notified())
        .await
        .unwrap();
    connection
}

#[tokio::test(start_paused = true)]
async fn stop_waits_for_in_flight_send_to_finish() {
    let knx = knx_without_cleanup().await;
    let connection = sending(&knx).await;
    let stopping = tokio::spawn({
        let knx = knx.clone();
        async move { knx.stop().await }
    });
    tokio::task::yield_now().await;
    assert!(
        !stopping.is_finished(),
        "stop must join the sender before returning"
    );
    connection.release.add_permits(1);
    stopping.await.unwrap();
    assert_eq!(connection.sent.load(Ordering::SeqCst), 1);
    assert_eq!(connection.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_a_sender_that_exceeds_shutdown_deadline() {
    let knx = knx_without_cleanup().await;
    let connection = sending(&knx).await;
    timeout(Duration::from_secs(30), knx.stop()).await.unwrap();
    assert_eq!(
        connection.active.load(Ordering::SeqCst),
        0,
        "sender survived stop"
    );
    connection.release.add_permits(1);
    tokio::task::yield_now().await;
    assert_eq!(
        connection.sent.load(Ordering::SeqCst),
        0,
        "telegram sent after stop returned"
    );
}

struct Completed(Arc<AtomicUsize>);
impl Drop for Completed {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn stop_aborts_and_joins_tasks_instead_of_detaching_them() {
    let knx = knx_without_cleanup().await;
    let completed = Arc::new(AtomicUsize::new(0));
    let slots = [
        &knx.receiving_task,
        &knx.processing_task,
        &knx.control_task,
        &knx.reconnect_task,
        &knx.cleanup_task,
        #[cfg(feature = "dpt")]
        &knx.refresh_task,
    ];
    let expected = slots.len();
    for slot in &slots {
        let guard = Completed(completed.clone());
        *slot.write().await = Some(tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        }));
    }
    timeout(Duration::from_secs(30), knx.stop()).await.unwrap();
    assert_eq!(
        completed.load(Ordering::SeqCst),
        expected,
        "a timed-out task was detached"
    );
    for slot in slots {
        assert!(slot.read().await.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_reconnect_before_waiting_for_sender_and_joins_receive_drain() {
    let knx = knx_without_cleanup().await;
    let drain = knx
        .receive_drain_task
        .read()
        .await
        .as_ref()
        .unwrap()
        .abort_handle();
    let connection = sending(&knx).await;
    let completed = Arc::new(AtomicUsize::new(0));
    let guard = Completed(completed.clone());
    *knx.reconnect_task.write().await = Some(tokio::spawn(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
    }));
    let stopping = tokio::spawn({
        let knx = knx.clone();
        async move { knx.stop().await }
    });
    timeout(Duration::from_secs(1), async {
        while completed.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(!stopping.is_finished());
    assert_eq!(connection.active.load(Ordering::SeqCst), 1);
    connection.release.add_permits(1);
    stopping.await.unwrap();
    assert!(drain.is_finished(), "receive drain survived stop");
    assert!(knx.sending_task.read().await.is_none());
    assert!(knx.receive_drain_task.read().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn concurrent_stops_wait_for_the_same_in_flight_work() {
    let knx = knx_without_cleanup().await;
    let connection = sending(&knx).await;
    let first = tokio::spawn({
        let knx = knx.clone();
        async move { knx.stop().await }
    });
    tokio::task::yield_now().await;
    let second = tokio::spawn({
        let knx = knx.clone();
        async move { knx.stop().await }
    });
    tokio::task::yield_now().await;
    assert!(
        !second.is_finished(),
        "second stop returned while first still owned running tasks"
    );
    connection.release.add_permits(1);
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(connection.active.load(Ordering::SeqCst), 0);
}
