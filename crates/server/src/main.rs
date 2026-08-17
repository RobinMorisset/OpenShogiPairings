//! Standalone OpenShogiPairings server binary.
//!
//! Runs the API (see the `osp_server` library) as its own process — used for
//! browser development and for the hosted remote server that referees share over
//! the internet (see `docs/archive/multi-referee-internet.md` and
//! `docs/archive/multi-tournament.md`). The Tauri desktop app does not use this binary;
//! it embeds the library directly.
//!
//! Configuration is entirely via environment variables:
//!
//! - `OSP_BIND`            — address to listen on (default `127.0.0.1:3000`).
//!   Behind a TLS reverse proxy, loopback is enough; set `0.0.0.0:3000` to
//!   expose it directly.
//! - `OSP_ADMIN_PASSWORD`  — gates *creating* new tournaments and the FESA
//!   ratings proxy (`/api/ratings*`). Unset lets anyone reach this server do
//!   either, which is only appropriate on a trusted machine or before the URL
//!   has circulated beyond its referees. Each tournament's own (separate)
//!   password is set when it's created.
//! - `OSP_EXTRA_ORIGINS`   — comma-separated browser origins to answer besides
//!   the app's own (e.g. `http://localhost:5174`), for running a second checkout
//!   whose Vite server is on another port. Unset on any real deployment.
//! - `OSP_STATIC_DIR`      — directory of the built SPA to serve same-origin.
//!   Unset serves the API only (the dev flow uses the Vite server for the SPA).
//! - `OSP_DATA_DIR`        — directory holding one file per tournament, loaded
//!   on boot and written through to on every change. Unset keeps everything in
//!   memory (lost on restart).
//! - `OSP_BACKUP_RETENTION_DAYS` — how long a *deleted* tournament's backups are
//!   kept before they are swept (default 30). `0` deletes them along with the
//!   tournament, which is unrecoverable.

use std::net::SocketAddr;
use std::time::Duration;

use osp_server::ServerConfig;

/// Default listen address when `OSP_BIND` is unset. Kept here so clients and
/// docs agree.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

/// `OSP_EXTRA_ORIGINS` split on commas, blanks dropped.
///
/// For running a second checkout: its Vite server is on another port, so its
/// origin is not one of the app's own and the browser would refuse every
/// response. Listing the origin is deliberately more typing than a "allow any
/// localhost port" switch would be — that switch is what would follow somebody
/// home onto a real host, where every route is open when no admin password is
/// set, and any other local page could then read and script the lot.
fn extra_origins() -> Vec<String> {
    std::env::var("OSP_EXTRA_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `OSP_BACKUP_RETENTION_DAYS` as a number of days, or `None` when it is unset
/// (the library's own default, a month, then applies).
///
/// A value that isn't a whole number of days is fatal rather than ignored:
/// silently falling back to the default would leave a host that asked for one
/// week keeping a month, or — worse, had the default been the other way — a
/// host that asked for a year keeping nothing.
fn backup_retention_days() -> Option<u64> {
    let raw = std::env::var("OSP_BACKUP_RETENTION_DAYS").ok()?;
    let days: u64 = raw.trim().parse().unwrap_or_else(|e| {
        panic!("OSP_BACKUP_RETENTION_DAYS must be a whole number of days, got {raw:?}: {e}")
    });
    match days {
        0 => tracing::warn!(
            "OSP_BACKUP_RETENTION_DAYS=0: deleting a tournament deletes its backups too, \
             with no way back"
        ),
        days => tracing::info!("keeping deleted tournaments' backups for {days} days"),
    }
    Some(days)
}

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
        admin_password: std::env::var("OSP_ADMIN_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty()),
        static_dir: std::env::var_os("OSP_STATIC_DIR").map(Into::into),
        data_dir: std::env::var_os("OSP_DATA_DIR").map(Into::into),
        backup_dir: std::env::var_os("OSP_BACKUP_DIR").map(Into::into),
        backup_retention: backup_retention_days().map(|days| Duration::from_secs(days * 86_400)),
        extra_origins: extra_origins(),
    };

    match &config.admin_password {
        Some(_) => tracing::info!("tournament creation gated by an admin password"),
        None => tracing::warn!(
            "tournament creation is OPEN — set OSP_ADMIN_PASSWORD before exposing this server \
             beyond a trusted machine"
        ),
    }
    if let Some(dir) = &config.static_dir {
        tracing::info!("serving the SPA from {}", dir.display());
    }
    if let Some(dir) = &config.data_dir {
        tracing::info!("persisting tournaments under {}", dir.display());
    }
    // Say where the backups go either way: unset means the per-user data
    // directory, which is worth printing rather than leaving the referee to
    // guess where their recovery copies landed.
    match config
        .backup_dir
        .clone()
        .or_else(osp_server::backup_default_root)
    {
        Some(dir) => tracing::info!("writing automatic backups under {}", dir.display()),
        None => tracing::warn!(
            "no backups directory could be resolved — automatic backups are DISABLED; set \
             OSP_BACKUP_DIR to enable them"
        ),
    }

    osp_server::serve_with_config(listener, config)
        .await
        .expect("server exited unexpectedly");
}
