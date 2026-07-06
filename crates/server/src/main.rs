//! OpenShogiPairings HTTP server.
//!
//! For this bring-up phase the server does exactly one meaningful thing: expose
//! `GET /api/health` so a client can confirm it is reachable. The tournament API
//! (create tournament, add players, generate a round, subscribe to live updates
//! over WebSocket) will be layered onto this same `Router` later.

use std::net::SocketAddr;

use axum::{routing::get, Json, Router};
use osp_core::HealthStatus;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Address the server listens on. Kept in one place so clients and docs agree.
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

    let app = build_router();

    let listener = tokio::net::TcpListener::bind(BIND_ADDR)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {BIND_ADDR}: {e}"));

    let addr: SocketAddr = listener.local_addr().expect("listener has a local address");
    tracing::info!("OpenShogiPairings server listening on http://{addr}");

    axum::serve(listener, app)
        .await
        .expect("server exited unexpectedly");
}

/// Assemble the application router.
///
/// Split out from `main` so it can be exercised directly in tests without
/// binding a real socket.
fn build_router() -> Router {
    // Dev-only CORS: the Vite dev server runs on a different origin (:5173) than
    // this API (:3000), so browsers block the fetch without an explicit allow.
    // `permissive` is fine for local development; lock this down before any
    // deployment that is reachable beyond localhost.
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/api/health", get(health))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// Report that the server is up. Returns the shared [`HealthStatus`] shape.
async fn health() -> Json<HealthStatus> {
    Json(HealthStatus::current())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = build_router();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: HealthStatus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.status, "ok");
    }
}
