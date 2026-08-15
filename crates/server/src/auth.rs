//! Password authentication (see `docs/archive/multi-tournament.md`).
//!
//! Two independent uses of the same [`AuthConfig`] shape:
//!
//! - **Per-tournament**: each tournament may have its own password, checked at
//!   `POST /api/tournaments/{id}/login`. The hash is what gets persisted to
//!   disk (`{id}.auth.json`, see [`crate::state`]) — never the plaintext.
//! - **Admin** (`AppState::admin_auth`): one process-wide password gating
//!   `POST /api/tournaments` (creating new tournaments) and the FESA ratings
//!   proxy (`/api/ratings*`) — both are instance-wide resources/capabilities
//!   rather than something scoped to one tournament, and both are worth
//!   protecting from a stranger who's found the URL: unbounded tournament
//!   creation fills disk, and an open ratings proxy lets anyone use this
//!   server as a free relay to FESA. Never persisted (comes from an env var
//!   each boot), just hashed in memory like everything else here.
//!
//! Model: a per-boot random bearer token is exchanged for the password via
//! login (`POST /api/tournaments/{id}/login` or `POST /api/admin/login`);
//! every protected route then requires `Authorization: Bearer <token>`. All
//! clients sharing one password share its token — this is a shared-password
//! model, not per-user accounts — and a restart rotates it.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use constant_time_eq::constant_time_eq;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::scope::TournamentCtx;
use crate::state::AppState;

/// How long a failed login is delayed, as light brute-force friction. With a
/// long password and TLS in front this is belt-and-suspenders — guessing a
/// password half a second per attempt is hopeless — but it costs nothing.
const FAILED_LOGIN_DELAY: Duration = Duration::from_millis(500);

/// A password (hashed, never held in plaintext) plus the per-boot bearer token
/// exchanged for it, shared cheaply via `Arc`.
#[derive(Clone)]
pub struct AuthConfig {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    password_hash: String,
    token: String,
}

impl AuthConfig {
    /// Hash `password` and mint a fresh random bearer token for this server run.
    pub fn new(password: impl AsRef<str>) -> Self {
        let password_hash =
            bcrypt::hash(password.as_ref(), bcrypt::DEFAULT_COST).expect("bcrypt hashing failed");
        Self::from_hash(password_hash)
    }

    /// Rebuild from an already-computed hash (e.g. loaded from
    /// `{id}.auth.json` on boot) — no re-hashing, since the plaintext isn't
    /// available. Still mints a fresh per-boot token.
    pub fn from_hash(password_hash: String) -> Self {
        // Two v4 UUIDs ≈ 244 bits of entropy — far more than a session token
        // needs — concatenated as hex into one opaque string.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        Self {
            inner: Arc::new(AuthInner {
                password_hash,
                token,
            }),
        }
    }

    /// The password hash, for persisting alongside the tournament.
    pub fn password_hash(&self) -> &str {
        &self.inner.password_hash
    }

    /// Whether `presented` matches the password this hash was built from.
    pub fn password_matches(&self, presented: &str) -> bool {
        Self::hash_matches(&self.inner.password_hash, presented)
    }

    /// Whether `presented` matches a bare `hash` — for a password that outlives
    /// its `AuthConfig`, as a deleted tournament's does (see
    /// [`crate::backup::DeletedMarker`]). Same check as
    /// [`password_matches`](Self::password_matches), without having to mint a
    /// per-boot token for a tournament that isn't running.
    pub fn hash_matches(hash: &str, presented: &str) -> bool {
        // A `verify` error is not a wrong password: it means the stored hash
        // itself is unusable (truncated by a partial write, hand-edited, from a
        // future format). Answering `false` is the only safe reply — nothing else
        // may authenticate — but doing it silently means the referee is locked out
        // of their own tournament with "wrong password" forever and no way to tell
        // why, so say so once per attempt. Not a `debug_assert!`: the hash comes
        // off disk, so this is malformed external input rather than a bug here.
        match bcrypt::verify(presented, hash) {
            Ok(ok) => ok,
            Err(e) => {
                tracing::warn!(
                    "auth: stored password hash is unusable ({e}); every password \
                     will be rejected until it is replaced"
                );
                false
            }
        }
    }

    /// The current bearer token, e.g. to hand back immediately on creation so
    /// the creator doesn't have to log in to their own tournament.
    pub fn token(&self) -> &str {
        &self.inner.token
    }

    /// Whether `presented` matches the current bearer token (constant-time).
    fn token_matches(&self, presented: &str) -> bool {
        constant_time_eq(presented.as_bytes(), self.inner.token.as_bytes())
    }
}

/// `POST /api/tournaments/{id}/login` request body.
#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    password: String,
}

/// `POST /api/tournaments/{id}/login` success body.
#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

/// Check `presented` against `auth` and return the login response: the token
/// on a match, 401 (after [`FAILED_LOGIN_DELAY`]) otherwise. Shared by
/// [`tournament_login`] and [`admin_login`].
async fn login_response(auth: &AuthConfig, presented: &str) -> Response {
    if auth.password_matches(presented) {
        (
            StatusCode::OK,
            Json(LoginResponse {
                token: auth.token().to_string(),
            }),
        )
            .into_response()
    } else {
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        ApiError::Unauthorized.into_response()
    }
}

/// Whether `req` carries a valid `Authorization: Bearer <token>` for `auth`.
pub(crate) fn has_valid_bearer(auth: &AuthConfig, req: &Request) -> bool {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| auth.token_matches(token))
}

/// Exchange a tournament's password for its session bearer token.
///
/// - The tournament has no password (open) → 404, the endpoint is effectively
///   off for it, same as embedded/no-auth mode.
/// - Wrong password → 401 after a short delay.
/// - Right password → 200 `{ token }`.
pub(crate) async fn tournament_login(
    TournamentCtx(instance): TournamentCtx,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(auth) = instance.auth.as_ref() else {
        return ApiError::NotFound("this tournament has no password".into()).into_response();
    };
    login_response(auth, &body.password).await
}

/// Middleware gating the protected per-tournament routes.
///
/// A tournament's own password is the gate whenever it has one: a valid
/// `Authorization: Bearer <token>` for *that* tournament, or 401. 404s first if
/// the id in the path doesn't resolve to any tournament at all (via
/// [`TournamentCtx`]'s own rejection).
///
/// A tournament with no password of its own falls back to the **admin** token,
/// because an admin password configured is exactly this codebase's marker for
/// "this host is reachable by people who are not its referees" — the same
/// marker [`crate::state::TournamentRegistry::list`] uses to hide unpublished
/// tournaments from strangers. Without the fallback, a password-less
/// tournament is writable by anyone who learns its id, and publishing one
/// hands that id to every caller of `GET /api/tournaments`: a stranger could
/// add players to it, unpublish it, or `DELETE` it outright.
///
/// The reader path is unaffected — `public::scope()` is merged *outside* this
/// layer (see [`crate::tournament::scope`]), so the public page keeps working
/// from its capability key with no password anywhere.
///
/// With no admin password either — every local and embedded server, and a
/// hosted one deliberately run open — nothing is gated, as before.
pub(crate) async fn require_tournament_auth(
    State(state): State<AppState>,
    TournamentCtx(instance): TournamentCtx,
    req: Request,
    next: Next,
) -> Response {
    match (&instance.auth, &state.admin_auth) {
        // This tournament has its own password: it is the gate.
        (Some(auth), _) if has_valid_bearer(auth, &req) => next.run(req).await,
        (Some(_), _) => ApiError::Unauthorized.into_response(),
        // No password of its own, on a host that has an admin password.
        (None, Some(admin)) if has_valid_bearer(admin, &req) => next.run(req).await,
        (None, Some(_)) => ApiError::Unauthorized.into_response(),
        // No password anywhere: the deliberately open server.
        (None, None) => next.run(req).await,
    }
}

/// Exchange the admin password for its session bearer token.
///
/// - No admin password configured → 404, the endpoint is effectively off
///   (local/embedded/dev, or a hosted server that deliberately runs open).
/// - Wrong password → 401 after a short delay.
/// - Right password → 200 `{ token }`.
pub(crate) async fn admin_login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(auth) = state.admin_auth.as_ref() else {
        return ApiError::NotFound("admin authentication is not enabled".into()).into_response();
    };
    login_response(auth, &body.password).await
}

/// Middleware gating the admin-only routes (`POST /api/tournaments`,
/// `/api/ratings*`).
///
/// A no-op when no admin password is configured; otherwise requires a valid
/// `Authorization: Bearer <token>` obtained from [`admin_login`].
pub(crate) async fn require_admin_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    match &state.admin_auth {
        None => next.run(req).await,
        Some(auth) if has_valid_bearer(auth, &req) => next.run(req).await,
        Some(_) => ApiError::Unauthorized.into_response(),
    }
}
