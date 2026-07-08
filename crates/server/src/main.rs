//! Standalone OpenShogiPairings server binary.
//!
//! Runs the API (see the `osp_server` library) as its own process — used for
//! browser development and for the hosted remote server that referees share over
//! the internet (see `docs/multi-referee-internet.md`). The Tauri desktop app
//! does not use this binary; it embeds the library directly.
//!
//! Configuration is entirely via environment variables:
//!
//! - `OSP_BIND`        — address to listen on (default `127.0.0.1:3000`). Behind
//!   a TLS reverse proxy, loopback is enough; set `0.0.0.0:3000` to expose it
//!   directly.
//! - `OSP_PASSWORD`    — shared referee password. Unset runs the API open, which
//!   is only appropriate on a trusted machine.
//! - `OSP_STATIC_DIR`  — directory of the built SPA to serve same-origin. Unset
//!   serves the API only (the dev flow uses the Vite server for the SPA).
//! - `OSP_DATA_FILE`   — file the tournament is loaded from on boot and written
//!   through to on every change. Unset keeps state in memory (lost on restart).

use std::net::SocketAddr;

use osp_server::ServerConfig;

/// Default listen address when `OSP_BIND` is unset. Kept here so clients and
/// docs agree.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() {
    // Log at INFO by default; override with e.g. `RUST_LOG=debug`.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let bind_addr = std::env::var("OSP_BIND").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind_addr}: {e}"));

    let addr: SocketAddr = listener.local_addr().expect("listener has a local address");
    tracing::info!("OpenShogiPairings server listening on http://{addr}");

    let config = ServerConfig {
        password: std::env::var("OSP_PASSWORD").ok().filter(|p| !p.is_empty()),
        static_dir: std::env::var_os("OSP_STATIC_DIR").map(Into::into),
        data_file: std::env::var_os("OSP_DATA_FILE").map(Into::into),
    };

    match &config.password {
        Some(_) => tracing::info!("authentication enabled (shared password)"),
        None => tracing::warn!(
            "authentication DISABLED — set OSP_PASSWORD to require a password before exposing this server"
        ),
    }
    if let Some(dir) = &config.static_dir {
        tracing::info!("serving the SPA from {}", dir.display());
    }
    if let Some(file) = &config.data_file {
        tracing::info!("persisting the tournament to {}", file.display());
    }

    osp_server::serve_with_config(listener, config)
        .await
        .expect("server exited unexpectedly");
}
