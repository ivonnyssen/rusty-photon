//! Shared test fixtures used by per-device-type test modules
//! (`focuser::tests`, `mount::tests`). Spawns an axum-backed Alpaca
//! stub server on `127.0.0.1:0` that individual tests then shape with
//! their own routes.
//!
//! A workspace-wide testing-strategy decision is being tracked in
//! issue #111; stubs in this module are the agreed interim approach.

use std::net::SocketAddr;

use axum::Router;

pub(crate) struct AlpacaStub {
    addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl AlpacaStub {
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for AlpacaStub {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

pub(crate) async fn spawn_stub(router: Router) -> AlpacaStub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    AlpacaStub {
        addr,
        shutdown_tx: Some(tx),
        handle: Some(handle),
    }
}
