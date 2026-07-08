//! Shared-password authentication for the remote (standalone) server.
//!
//! Remote mode (see `docs/multi-referee-internet.md`) gates the whole API behind
//! a single shared password so only referees can reach a hosted tournament. The
//! embedded desktop server runs *without* auth — it is loopback-only and
//! in-process — so everything here is inert when [`AppState::auth`] is `None`.
//!
//! Model: the server boots with one password and one random per-boot bearer
//! token. `POST /api/login` trades the password for that token; every protected
//! route then requires `Authorization: Bearer <token>`. All clients share the
//! same token — this is a shared-password model, not per-user accounts — and a
//! restart rotates it. Password and token are compared in constant time.

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
use crate::state::AppState;

/// How long a failed login is delayed, as light brute-force friction. With a
/// long password and TLS in front this is belt-and-suspenders — guessing a
/// password half a second per attempt is hopeless — but it costs nothing.
const FAILED_LOGIN_DELAY: Duration = Duration::from_millis(500);

/// Server-side authentication state, shared cheaply via `Arc`.
///
/// Present only when the server is started with a password (remote mode). Holds
/// the shared password and the random per-boot token handed out on login.
#[derive(Clone)]
pub struct AuthConfig {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    password: String,
    token: String,
}

impl AuthConfig {
    /// Build auth around a shared `password`, minting a fresh random bearer
    /// token for this server run.
    pub fn new(password: impl Into<String>) -> Self {
        // Two v4 UUIDs ≈ 244 bits of entropy — far more than a session token
        // needs — concatenated as hex into one opaque string.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        Self {
            inner: Arc::new(AuthInner {
                password: password.into(),
                token,
            }),
        }
    }

    /// Whether `presented` matches the shared password (constant-time).
    fn password_matches(&self, presented: &str) -> bool {
        constant_time_eq(presented.as_bytes(), self.inner.password.as_bytes())
    }

    /// Whether `presented` matches the current bearer token (constant-time).
    fn token_matches(&self, presented: &str) -> bool {
        constant_time_eq(presented.as_bytes(), self.inner.token.as_bytes())
    }
}

/// `POST /api/login` request body.
#[derive(Deserialize)]
pub struct LoginRequest {
    password: String,
}

/// `POST /api/login` success body.
#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

/// Exchange the shared password for the session bearer token.
///
/// - No auth configured (embedded mode) → 404, the endpoint is effectively off.
/// - Wrong password → 401 after a short delay.
/// - Right password → 200 `{ token }`.
pub async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> Response {
    let Some(auth) = state.auth.as_ref() else {
        return ApiError::NotFound("authentication is not enabled".into()).into_response();
    };
    if auth.password_matches(&body.password) {
        (
            StatusCode::OK,
            Json(LoginResponse {
                token: auth.inner.token.clone(),
            }),
        )
            .into_response()
    } else {
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        ApiError::Unauthorized.into_response()
    }
}

/// Middleware gating the protected routes.
///
/// A no-op when auth is disabled (embedded/local mode); otherwise it requires a
/// valid `Authorization: Bearer <token>` header and answers 401 without one.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(auth) = state.auth.as_ref() else {
        // No auth configured: let everything through.
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) if auth.token_matches(token) => next.run(req).await,
        _ => ApiError::Unauthorized.into_response(),
    }
}
