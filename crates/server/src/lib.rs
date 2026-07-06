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
//! The server holds the current tournament in memory as the single source of
//! truth shared by all connected clients; see [`AppState`].

mod error;
mod ratings;
mod state;
mod tournament;

pub use state::AppState;

use axum::{
    routing::{get, post},
    Json, Router,
};
use osp_core::HealthStatus;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Build the application router around the given state.
pub fn router(state: AppState) -> Router {
    // Permissive CORS: clients are served from a different origin than this API
    // — the Vite dev server (:5173) in the browser, and the `tauri://` /
    // `http://tauri.localhost` webview origin in the desktop app. Fine for a
    // localhost-only API; lock down before exposing this beyond the machine.
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/api/health", get(health))
        .route("/api/ratings", get(ratings::ratings_handler))
        .route("/api/ratings/refresh", post(ratings::refresh_handler))
        .merge(tournament::routes())
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// Serve the API on an already-bound listener until the process ends.
///
/// Taking a bound [`TcpListener`](tokio::net::TcpListener) (rather than an
/// address) lets the caller bind first and read back the chosen port — which the
/// embedded server relies on when binding to an OS-assigned port.
pub async fn serve(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    axum::serve(listener, router(AppState::default())).await
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
        let (status, body) = send(router(AppState::default()), get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn get_tournament_is_404_before_creation() {
        let (status, body) = send(router(AppState::default()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn create_then_add_and_remove_player() {
        let state = AppState::default();

        // Create.
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Paris Open" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["name"], "Paris Open");
        assert_eq!(body["format_version"], 2);
        assert!(body["players"].as_array().unwrap().is_empty());

        // Register a player.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Kobayashi", "first_name": "Taichi", "rating": 2556, "nationality": "jp" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let players = body["players"].as_array().unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0]["last_name"], "Kobayashi");
        assert_eq!(players[0]["first_name"], "Taichi");
        assert_eq!(players[0]["nationality"], "JP"); // uppercased server-side
        let player_id = players[0]["id"].as_str().unwrap().to_string();

        // Remove that player.
        let (status, body) = send(
            router(state.clone()),
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
            router(AppState::default()),
            json_req("POST", "/api/tournament/players", json!({ "last_name": "Alice" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn blank_player_name_is_400() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Paris Open" })),
        )
        .await;

        let (status, _) = send(
            router(state.clone()),
            json_req("POST", "/api/tournament/players", json!({ "last_name": "   " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn replace_tournament_loads_a_saved_file() {
        let state = AppState::default();

        let mut saved = Tournament::new("Loaded Cup").unwrap();
        saved
            .add_player(osp_core::NewPlayer {
                last_name: "Bob".into(),
                ..Default::default()
            })
            .unwrap();
        let saved_json = serde_json::to_value(&saved).unwrap();

        let (status, body) = send(
            router(state.clone()),
            json_req("PUT", "/api/tournament", saved_json),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "Loaded Cup");

        let (status, body) = send(router(state.clone()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["players"][0]["last_name"], "Bob");
    }

    #[tokio::test]
    async fn replace_tournament_rejects_unknown_version() {
        let state = AppState::default();
        let mut future = serde_json::to_value(Tournament::new("X").unwrap()).unwrap();
        future["format_version"] = json!(999);

        let (status, _) = send(router(state), json_req("PUT", "/api/tournament", future)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
