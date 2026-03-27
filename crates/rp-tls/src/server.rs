use std::net::SocketAddr;
use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::debug;

use crate::config::TlsConfig;
use crate::error::{Result, TlsError};

/// Bind a TCP listener with dual-stack (IPv4+IPv6) support.
///
/// This replicates the socket2 binding logic from `ascom-alpaca-rs`'s
/// `Server::bind()`, ensuring consistent behavior on all platforms.
/// On Linux, dual-stack is the default for IPv6 sockets, but on Windows
/// it is not — this function explicitly sets `only_v6(false)`.
pub fn bind_dual_stack(addr: SocketAddr) -> Result<std::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;

    if addr.is_ipv6() {
        socket.set_only_v6(false)?;
    }

    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;

    Ok(socket.into())
}

/// Bind a TCP listener with dual-stack support and return a tokio `TcpListener`.
pub async fn bind_dual_stack_tokio(addr: SocketAddr) -> Result<TcpListener> {
    crate::install_crypto_provider();
    let std_listener = bind_dual_stack(addr)?;
    let listener = TcpListener::from_std(std_listener)?;
    Ok(listener)
}

/// Load TLS certificate and key from a `TlsConfig`, returning a `TlsAcceptor`.
pub fn build_tls_acceptor(tls_config: &TlsConfig) -> Result<TlsAcceptor> {
    crate::install_crypto_provider();
    let cert_path = tls_config.resolved_cert_path();
    let key_path = tls_config.resolved_key_path();

    debug!("Loading TLS cert from {}", cert_path.display());
    debug!("Loading TLS key from {}", key_path.display());

    let cert_pem = std::fs::read_to_string(&cert_path)?;
    let key_pem = std::fs::read_to_string(&key_path)?;

    let certs: Vec<CertificateDer> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| TlsError::Pem(format!("failed to parse cert PEM: {e}")))?;

    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| TlsError::Pem(format!("failed to parse key PEM: {e}")))?
        .ok_or_else(|| TlsError::Pem("no private key found in PEM file".to_string()))?;

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

/// Serve an axum router over TLS on the given listener.
///
/// The `shutdown` future is polled to initiate graceful shutdown.
pub async fn serve_tls<F>(
    listener: TcpListener,
    router: axum::Router,
    tls_config: &TlsConfig,
    shutdown: F,
) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let acceptor = build_tls_acceptor(tls_config)?;

    debug!(
        "Starting TLS server on {}",
        listener
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
    );

    axum_server_tls(listener, router, acceptor, shutdown).await
}

/// Serve an axum router over plain HTTP on the given listener.
///
/// The `shutdown` future is polled to initiate graceful shutdown.
pub async fn serve_plain<F>(listener: TcpListener, router: axum::Router, shutdown: F) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    debug!(
        "Starting plain HTTP server on {}",
        listener
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

/// Internal: run an axum TLS server loop using tokio-rustls.
///
/// Accepts TCP connections, wraps them with TLS, and serves the router.
/// Stops accepting new connections when `shutdown` completes.
async fn axum_server_tls<F>(
    listener: TcpListener,
    router: axum::Router,
    acceptor: TlsAcceptor,
    shutdown: F,
) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    use tokio::pin;

    let shutdown = shutdown;
    pin!(shutdown);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, remote_addr) = result?;
                let acceptor = acceptor.clone();
                let router = router.clone();

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            debug!("TLS handshake failed from {}: {}", remote_addr, e);
                            return;
                        }
                    };

                    let io = TokioIo::new(tls_stream);
                    let service = TowerToHyperService::new(router.into_service());

                    if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, service)
                    .await
                    {
                        debug!("Error serving TLS connection from {}: {}", remote_addr, e);
                    }
                });
            }
            () = &mut shutdown => {
                debug!("TLS server shutting down");
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_dual_stack_on_port_zero() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind_dual_stack(addr).unwrap();
        let bound_addr = listener.local_addr().unwrap();
        assert_ne!(bound_addr.port(), 0, "should be assigned a real port");
    }

    #[test]
    fn bind_dual_stack_ipv6() {
        let addr: SocketAddr = "[::]:0".parse().unwrap();
        // This may fail on systems without IPv6 support, so we just
        // check it doesn't panic for the wrong reason.
        match bind_dual_stack(addr) {
            Ok(listener) => {
                let bound = listener.local_addr().unwrap();
                assert_ne!(bound.port(), 0);
            }
            Err(TlsError::Io(e)) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {
                // IPv6 not available on this system, that's fine
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
