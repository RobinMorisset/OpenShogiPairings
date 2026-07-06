//! OpenShogiPairings HTTP server.
//!
//! Exposes the tournament API consumed by the web UI (and, later, a CLI client
//! and the Tauri desktop app). The server holds the current tournament in memory
//! as the single source of truth shared by all connected referees; see
//! [`state::AppState`]. Rounds, pairings and live updates over WebSocket will be
//! layered onto this same router later.

mod error;
mod state;
mod tournament;

use std::net::SocketAddr;

use axum::{routing::get, Json, Router};
use osp_core::HealthStatus;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

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

    let app = build_router(AppState::default());

    let listener = tokio::net::TcpListener::bind(BIND_ADDR)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {BIND_ADDR}: {e}"));

    let addr: SocketAddr = listener.local_addr().expect("listener has a local address");
    tracing::info!("OpenShogiPairings server listening on http://{addr}");

    axum::serve(listener, app)
        .await
        .expect("server exited unexpectedly");
}

/// Assemble the application router around the given state.
///
/// Split out from `main` so it can be exercised directly in tests without
/// binding a real socket.
fn build_router(state: AppState) -> Router {
    // Dev-only CORS: the Vite dev server runs on a different origin (:5173) than
    // this API (:3000), so browsers block requests without an explicit allow.
    // `permissive` is fine for local development; lock this down before any
    // deployment that is reachable beyond localhost.
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/api/health", get(health))
        .merge(tournament::routes())
        .with_state(state)
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
    use osp_core::Tournament;
    use serde_json::json;
    use tower::ServiceExt; // for `oneshot`

    /// Send one request through the router and return (status, parsed JSON body).
    async fn send(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn json_req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (status, body) = send(build_router(AppState::default()), get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn get_tournament_is_404_before_creation() {
        let (status, body) =
            send(build_router(AppState::default()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn create_then_add_and_remove_player() {
        let state = AppState::default();

        // Create.
        let (status, body) = send(
            build_router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Paris Open" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["name"], "Paris Open");
        assert_eq!(body["format_version"], 1);
        assert!(body["players"].as_array().unwrap().is_empty());

        // Register a player.
        let (status, body) = send(
            build_router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "name": "Alice", "rating": 1800 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let players = body["players"].as_array().unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0]["name"], "Alice");
        let player_id = players[0]["id"].as_str().unwrap().to_string();

        // Remove that player.
        let (status, body) = send(
            build_router(state.clone()),
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/tournament/players/{player_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["players"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_player_without_tournament_is_404() {
        let (status, _) = send(
            build_router(AppState::default()),
            json_req("POST", "/api/tournament/players", json!({ "name": "Alice" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn blank_player_name_is_400() {
        let state = AppState::default();
        send(
            build_router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Paris Open" })),
        )
        .await;

        let (status, _) = send(
            build_router(state.clone()),
            json_req("POST", "/api/tournament/players", json!({ "name": "   " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn replace_tournament_loads_a_saved_file() {
        let state = AppState::default();

        // A tournament as it would appear in a saved file.
        let mut saved = Tournament::new("Loaded Cup").unwrap();
        saved.add_player(osp_core::NewPlayer {
            name: "Bob".into(),
            ..Default::default()
        })
        .unwrap();
        let saved_json = serde_json::to_value(&saved).unwrap();

        let (status, body) = send(
            build_router(state.clone()),
            json_req("PUT", "/api/tournament", saved_json),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "Loaded Cup");

        // It is now the current tournament.
        let (status, body) = send(build_router(state.clone()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["players"][0]["name"], "Bob");
    }

    #[tokio::test]
    async fn replace_tournament_rejects_unknown_version() {
        let state = AppState::default();
        let mut future = serde_json::to_value(Tournament::new("X").unwrap()).unwrap();
        future["format_version"] = json!(999);

        let (status, _) = send(
            build_router(state),
            json_req("PUT", "/api/tournament", future),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
