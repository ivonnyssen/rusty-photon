use std::net::SocketAddr;
use std::sync::Arc;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::debug;

use crate::config::TlsConfig;
use crate::error::Result;
use crate::resolver::ReloadableCertResolver;

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

    // On Unix, set SO_REUSEADDR so the server can rebind its port immediately
    // across a restart or in-process reload, even while a previous listener's
    // accepted connections (an HTTP client's keep-alive pool) or TIME_WAIT
    // sockets still linger — without it a reloading service (dsd-fp2's
    // `config.apply`) fails to rebind with `AddrInUse`. On Unix this still
    // rejects a second *live* listener on the port, so it can't mask an
    // "already running" error.
    //
    // We deliberately do NOT set SO_REUSEADDR on Windows, where its semantics
    // differ — there it would let another process bind (hijack) the same port.
    // Windows already lets the original owner rebind, so the reload works there
    // without it. (The exclusive-bind opt-in, SO_EXCLUSIVEADDRUSE, isn't exposed
    // by socket2; the default Windows behaviour is the safe choice here.)
    #[cfg(unix)]
    socket.set_reuse_address(true)?;

    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;

    Ok(socket.into())
}

/// Bind a TCP listener with dual-stack support and return a tokio `TcpListener`.
pub async fn bind_dual_stack_tokio(addr: SocketAddr) -> Result<TcpListener> {
    let std_listener = bind_dual_stack(addr)?;
    let listener = TcpListener::from_std(std_listener)?;
    Ok(listener)
}

/// Load TLS certificate and key from a `TlsConfig`, returning a `TlsAcceptor`
/// that hot-reloads the pair when the files change on disk (see
/// [`ReloadableCertResolver`]).
pub fn build_tls_acceptor(tls_config: &TlsConfig) -> Result<TlsAcceptor> {
    let cert_path = tls_config.resolved_cert_path();
    let key_path = tls_config.resolved_key_path();

    debug!("Loading TLS cert from {}", cert_path.display());
    debug!("Loading TLS key from {}", key_path.display());

    let resolver = ReloadableCertResolver::load(cert_path, key_path)?;
    Ok(acceptor_from_resolver(resolver))
}

/// Wrap a [`ReloadableCertResolver`] into a `TlsAcceptor` — the seam tests
/// use to shorten the resolver's check interval before serving.
pub fn acceptor_from_resolver(resolver: ReloadableCertResolver) -> TlsAcceptor {
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    TlsAcceptor::from(Arc::new(server_config))
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

    serve_tls_with_acceptor(listener, router, acceptor, shutdown).await
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

/// Run an axum TLS server loop using tokio-rustls with a caller-built
/// acceptor (e.g. one from [`acceptor_from_resolver`]).
///
/// Accepts TCP connections, wraps them with TLS, and serves the router.
/// Stops accepting new connections when `shutdown` completes.
pub async fn serve_tls_with_acceptor<F>(
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::error::TlsError;

    #[test]
    fn bind_dual_stack_on_port_zero() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind_dual_stack(addr).unwrap();
        let bound_addr = listener.local_addr().unwrap();
        assert_ne!(bound_addr.port(), 0, "should be assigned a real port");
    }

    #[test]
    fn bind_dual_stack_rebinds_port_with_lingering_connection() {
        // A service that reloads tears down its listener and rebinds the same
        // port while a client's previous connection may still linger on it.
        // SO_REUSEADDR must let the rebind succeed instead of failing
        // `AddrInUse`. This guards the OS-sensitive rebind behaviour (dsd-fp2's
        // `config.apply` reload) across platforms.
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind_dual_stack(addr).unwrap();
        // bind_dual_stack sets non-blocking; block in accept() for the test.
        listener.set_nonblocking(false).unwrap();
        let bound = listener.local_addr().unwrap();

        // Establish and accept a connection, then drop the listener while the
        // accepted connection lingers — independent of the listening socket.
        let client = std::net::TcpStream::connect(bound).unwrap();
        let (server_conn, _) = listener.accept().unwrap();
        drop(listener);

        // Rebinding the same port must succeed thanks to SO_REUSEADDR.
        let rebind = bind_dual_stack(bound);
        assert!(
            rebind.is_ok(),
            "rebind of {bound} failed (SO_REUSEADDR regression?): {:?}",
            rebind.err()
        );

        drop(server_conn);
        drop(client);
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
