//! OpenShogiPairings server library.
//!
//! The HTTP API is exposed here as a reusable [`Router`] plus a [`serve`] helper,
//! so it can be run two ways from the same code:
//!
//! - as a standalone process (the `osp-server` binary, for browser dev and a
//!   future CLI), and
//! - embedded inside the Tauri desktop app, which starts it in-process on an
//!   OS-assigned port so the packaged single-exe needs no separate server.
//!
//! The server holds any number of tournaments in memory as the single source of
//! truth shared by all connected clients; see [`AppState`] and
//! `docs/archive/multi-tournament.md`.
//!
//! The routes themselves are exercised end-to-end from `crates/server/tests/`,
//! one binary per area (`tournament_api`, `auth`, `public_access`, …) over the
//! shared helpers in `tests/common/`. Going through the crate's public surface
//! the way the desktop app and the SPA do is the point: what the tests can
//! reach is what a client can reach.
//!
//! Public items link to the private modules that explain them (see the note in
//! `osp_core`'s crate docs); rustdoc renders those unlinked unless it is run with
//! `--document-private-items`, so that warning is off here too.
//!
//! `unreachable_pub` and `unnameable_types` are turned on for the reason given
//! in `osp_core`'s crate docs: between them they pin down what is really part
//! of the API.
#![allow(rustdoc::private_intra_doc_links)]
#![warn(unnameable_types, unreachable_pub)]

mod auth;
mod backup;
mod error;
mod error_codes;
mod live;
mod public;
mod ratings;
mod registry;
mod save;
mod scope;
mod state;
mod tournament;

pub use auth::AuthConfig;
// Everything [`AppState`] hands back — the registry and its instances, a store
// behind a lock guard, the summaries `list` returns, the error `mutate` returns —
// is named here too. They are reachable through its public methods either way;
// exporting them is what lets a caller (the desktop wrapper, a test) write the
// type down.
pub use ratings::CachedRatings;
pub use state::{
    AppState, MutateError, NoSuchTournament, TournamentInstance, TournamentProblem,
    TournamentRegistry, TournamentStore, TournamentSummary,
};

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{request, HeaderName, HeaderValue, Method};
use axum::{middleware, routing::get, Json, Router};
use osp_core::HealthStatus;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

/// The `http:` origins the SPA can be served from when it is *not* same-origin
/// with the API. The `tauri:` scheme is handled in [`is_app_origin`]; a hosted
/// server serves the SPA same-origin (see [`router_with_static`]) and so appears
/// nowhere here.
const CROSS_ORIGIN_CLIENTS: [&str; 3] = [
    // The Tauri webview on Windows, which uses `http:` where the other
    // platforms use the `tauri:` scheme.
    "http://tauri.localhost",
    // The Vite dev server (`devUrl` in `tauri.conf.json`, and `strictPort` in
    // `vite.config.ts`, so the port is fixed).
    "http://localhost:5173",
    "http://127.0.0.1:5173",
];

/// Whether `origin` is one of this app's own front-ends.
///
/// Named origins rather than `Any`, because the API answers several routes that
/// need no credentials at all — the picker listing, `/api/health`, the SSE
/// stream, the public reader group — and on a server with no admin password
/// (every desktop and embedded one) *every* route is open by design. `Any` lets
/// a page on an unrelated site read and script all of it; the desktop server is
/// on an OS-assigned port, but `osp-server` itself defaults to the well-known
/// `127.0.0.1:3000`. Naming them keeps the browser's own same-origin policy as
/// the backstop it is meant to be.
fn is_app_origin(origin: &HeaderValue, extra: &[String]) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if extra.iter().any(|allowed| allowed == origin) {
        return true;
    }
    // The Tauri webview's own scheme on macOS and Linux. Nothing fetched over
    // the network can claim a `tauri:` origin, so admitting the scheme is safe —
    // and it avoids depending on the host Tauri picks, which differs by platform
    // and has changed between Tauri versions.
    origin.starts_with("tauri://") || CROSS_ORIGIN_CLIENTS.contains(&origin)
}

/// `X-Tournament-Version`, the optimistic-concurrency header the client sends on
/// every mutation (see [`live::check_version`]).
const TOURNAMENT_VERSION_HEADER: HeaderName = HeaderName::from_static("x-tournament-version");

/// Build the API-only router around the given state.
///
/// `/api/health` and `GET /api/tournaments` are always open; creating or
/// importing a tournament and the FESA ratings proxy (`/api/ratings*`) may require
/// [`AppState::admin_auth`] (see `POST /api/admin/login`); everything scoped
/// to one tournament (`/api/tournaments/{id}/...`) may require that
/// tournament's own password — see [`tournament::scope`].
pub fn router(state: AppState) -> Router {
    router_inner(state, None)
}

/// Like [`router`], but also serves a built SPA from `static_dir` as the
/// fallback for every non-API request (the hosted remote server, §2.1).
///
/// The static assets are **public** — the app shell has to load before the
/// login overlay can appear — while the API stays gated. Unknown paths fall back
/// to `index.html` so deep links / refreshes work.
pub fn router_with_static(state: AppState, static_dir: PathBuf) -> Router {
    router_inner(state, Some(static_dir))
}

fn router_inner(state: AppState, static_dir: Option<PathBuf>) -> Router {
    // In dev the SPA is served cross-origin from the Vite dev server (:5173),
    // and the desktop app talks from the `tauri://` / `http://tauri.localhost`
    // webview origin. The hosted server serves the SPA same-origin (see
    // `router_with_static`), so this only matters for those — and it names them
    // rather than allowing any origin (see `CROSS_ORIGIN_CLIENTS`).
    //
    // No `allow_credentials`: authentication here is a bearer token the client
    // reads from `localStorage` and sets by hand, never an ambient cookie, so
    // there is nothing for a browser to attach on its own.
    let extra = Arc::clone(&state.extra_origins);
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            move |origin: &HeaderValue, _parts: &request::Parts| is_app_origin(origin, &extra),
        ))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION, TOURNAMENT_VERSION_HEADER]);

    // The FESA ratings proxy is a shared, instance-wide resource (not
    // per-tournament data), so it's gated by the admin password like
    // tournament creation, rather than any individual tournament's password.
    let admin_protected = Router::new()
        .merge(registry::admin_routes())
        .route("/api/ratings", get(ratings::ratings_handler))
        .route(
            "/api/ratings/refresh",
            axum::routing::post(ratings::refresh_handler),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin_auth,
        ));

    let mut app = Router::new()
        .route("/api/health", get(health))
        .route("/api/admin/login", axum::routing::post(auth::admin_login))
        .merge(registry::public_routes())
        .merge(admin_protected)
        .nest("/api/tournaments/{id}", tournament::scope(state.clone()))
        .with_state(state);

    if let Some(dir) = static_dir {
        // Serve files from `dir`; anything not found (e.g. a client-side route)
        // returns `index.html`.
        let index = dir.join("index.html");
        // The public reader page (see `public.rs`). Routed explicitly rather
        // than left to the fallback below, which answers `404` with the shell —
        // harmless for a stray refresh, but this is a URL printed on a QR code
        // and handed to a roomful of people, so it must be a plain `200`.
        app = app.route_service("/t/{id}/public", ServeFile::new(&index));
        app = app.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
    }

    // Outermost, so it covers every route including the static fallback: a
    // panic anywhere below becomes a 500 the client can report, rather than a
    // dropped connection it cannot tell from a network failure (see
    // `error::panic_response` for why that distinction wedged a client).
    app.layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::custom(error::panic_response))
}

/// Serve the API on an already-bound listener until the process ends, with an
/// empty in-memory registry, no admin auth and no backups kept — the fully
/// open, throwaway configuration (tests, quick manual runs), which writes
/// nothing anywhere. The Tauri desktop app uses [`serve_with_config`] instead,
/// so its tournaments persist across restarts and its backups are kept.
///
/// Taking a bound [`TcpListener`](tokio::net::TcpListener) (rather than an
/// address) lets the caller bind first and read back the chosen port — which the
/// embedded server relies on when binding to an OS-assigned port.
pub async fn serve(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    axum::serve(listener, router(AppState::default())).await
}

/// How the standalone (remote) server is configured. All fields default to the
/// open, in-memory, API-only behaviour of [`serve`].
#[derive(Default)]
pub struct ServerConfig {
    /// Gates `POST /api/tournaments` and `POST /api/tournaments/import` (the
    /// two ways to mint a tournament); `None` lets anyone create one (fine on a
    /// trusted machine, risky on a public host whose URL might circulate beyond
    /// its referees).
    pub admin_password: Option<String>,
    /// Directory of the built SPA to serve same-origin; `None` = API only.
    pub static_dir: Option<PathBuf>,
    /// Directory holding one file per tournament, loaded on boot and written
    /// through to on every change; `None` = in-memory only (lost on restart).
    pub data_dir: Option<PathBuf>,
    /// Root holding one directory of automatic backups per tournament (see
    /// [`backup`]); `None` = the per-user data directory
    /// ([`backup::default_root`]), which is where they have always gone.
    /// Separate from `data_dir` on purpose: the backups are the recovery copy,
    /// and putting them on another disk (or a synced folder) is exactly what a
    /// recovery copy is for.
    pub backup_dir: Option<PathBuf>,
    /// How long the backups of a *deleted* tournament are kept before they are
    /// swept (see [`state::TournamentRegistry::remove`]); `None` =
    /// [`backup::DEFAULT_DELETED_RETENTION`] (a month). `Duration::ZERO` deletes
    /// them with the tournament, as every release before this one did.
    ///
    /// An `Option` rather than a plain `Duration` precisely so that
    /// `ServerConfig::default()` cannot mean "keep nothing": the default here
    /// has to be the safe end, not the zero value.
    pub backup_retention: Option<std::time::Duration>,
    /// Browser origins to answer beyond the app's own (see
    /// [`CROSS_ORIGIN_CLIENTS`]). Empty by default, and meant for one thing:
    /// running a second checkout whose Vite server is on another port. Listed
    /// rather than wildcarded — see [`AppState::extra_origins`].
    pub extra_origins: Vec<String>,
}

/// Where automatic backups go when [`ServerConfig::backup_dir`] is left unset:
/// `<per-user data dir>/openshogipairings/backups`, or `None` on a platform
/// with no known data directory (backups are then kept nowhere). Exposed so a
/// host — the standalone binary, the desktop app — can *report* the effective
/// location without duplicating how it is derived.
pub fn backup_default_root() -> Option<PathBuf> {
    backup::default_root()
}

/// Serve the standalone server with the given [`ServerConfig`] (remote mode).
pub async fn serve_with_config(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> std::io::Result<()> {
    let state = AppState {
        registry: Arc::new(TournamentRegistry::with_retention(
            config.data_dir,
            // Resolve the per-user default here, at the one boundary that has a
            // host behind it. The registry itself treats `None` as "keep no
            // backups" precisely so that the in-process states tests build
            // cannot land in a real data directory.
            config.backup_dir.or_else(backup::default_root),
            config
                .backup_retention
                .unwrap_or(backup::DEFAULT_DELETED_RETENTION),
        )),
        admin_auth: config.admin_password.map(AuthConfig::new),
        extra_origins: Arc::from(config.extra_origins),
        ..Default::default()
    };
    let app = match config.static_dir {
        Some(dir) => router_with_static(state, dir),
        None => router(state),
    };
    axum::serve(listener, app).await
}

/// Report that the server is up. Returns the shared [`HealthStatus`] shape.
async fn health() -> Json<HealthStatus> {
    Json(HealthStatus::current())
}
