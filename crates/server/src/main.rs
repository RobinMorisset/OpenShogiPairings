//! Standalone OpenShogiPairings server binary.
//!
//! Runs the API (see the `osp_server` library) as its own process on a fixed
//! port — used for browser development and, later, a CLI client. The Tauri
//! desktop app does not use this binary; it embeds the library directly.

use std::net::SocketAddr;

/// Address the standalone server listens on. Kept in one place so clients and
/// docs agree.
const BIND_ADDR: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() {
    // Log at INFO by default; override with e.g. `RUST_LOG=debug`.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let listener = tokio::net::TcpListener::bind(BIND_ADDR)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {BIND_ADDR}: {e}"));

    let addr: SocketAddr = listener.local_addr().expect("listener has a local address");
    tracing::info!("OpenShogiPairings server listening on http://{addr}");

    // Remote mode gates the whole API behind a shared password (see
    // `docs/multi-referee-internet.md`). Supply it via `OSP_PASSWORD`; leaving it
    // unset runs the server open, which is only appropriate on a trusted machine.
    let password = std::env::var("OSP_PASSWORD").ok().filter(|p| !p.is_empty());
    match password {
        Some(_) => tracing::info!("authentication enabled (shared password)"),
        None => tracing::warn!(
            "authentication DISABLED — set OSP_PASSWORD to require a password before exposing this server"
        ),
    }

    osp_server::serve_with_auth(listener, password)
        .await
        .expect("server exited unexpectedly");
}
