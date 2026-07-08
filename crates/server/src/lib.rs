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

mod auth;
mod backup;
mod error;
mod ratings;
mod state;
mod tournament;

pub use auth::AuthConfig;
pub use state::AppState;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::{
    middleware,
    routing::{get, post},
    Json, Router,
};
use osp_core::HealthStatus;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::TournamentStore;

/// Build the API-only router around the given state.
///
/// `/api/health` and `/api/login` are always open; every other route sits
/// behind the [`auth::require_auth`] middleware, which is a no-op unless the
/// state carries an [`AuthConfig`] (remote mode).
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
    // Permissive CORS: in dev the SPA is served cross-origin from the Vite dev
    // server (:5173), and the desktop app talks from the `tauri://` /
    // `http://tauri.localhost` webview origin. The hosted server serves the SPA
    // same-origin (see `router_with_static`), so this only matters for those.
    let cors = CorsLayer::permissive();

    // Everything except health/login requires a valid session token when auth
    // is enabled. The middleware self-disables when it isn't.
    let protected = Router::new()
        .route("/api/ratings", get(ratings::ratings_handler))
        .route("/api/ratings/refresh", post(ratings::refresh_handler))
        .merge(tournament::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let mut app = Router::new()
        .route("/api/health", get(health))
        .route("/api/login", post(auth::login))
        .merge(protected)
        .with_state(state);

    if let Some(dir) = static_dir {
        // Serve files from `dir`; anything not found (e.g. a client-side route)
        // returns `index.html`.
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
    }

    app.layer(cors).layer(TraceLayer::new_for_http())
}

/// Serve the API on an already-bound listener until the process ends, without
/// authentication or persistence (embedded/local mode).
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
    /// Shared password gating the whole API; `None` runs open (trusted machine).
    pub password: Option<String>,
    /// Directory of the built SPA to serve same-origin; `None` = API only.
    pub static_dir: Option<PathBuf>,
    /// File the current tournament is loaded from and written through to; `None`
    /// = in-memory only (lost on restart).
    pub data_file: Option<PathBuf>,
}

/// Serve the standalone server with the given [`ServerConfig`] (remote mode).
pub async fn serve_with_config(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> std::io::Result<()> {
    let store = match config.data_file {
        Some(path) => TournamentStore::with_persistence(path),
        None => TournamentStore::default(),
    };
    let state = AppState {
        store: Arc::new(RwLock::new(store)),
        auth: config.password.map(AuthConfig::new),
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

    /// Send one request and return (status, body as text) — for non-JSON
    /// responses such as served static files.
    async fn send_text(app: Router, req: Request<Body>) -> (StatusCode, String) {
        let response = app.oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_empty(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn json_req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// A JSON request carrying an `Authorization: Bearer <token>` header.
    fn json_req_auth(
        method: &str,
        uri: &str,
        body: serde_json::Value,
        token: &str,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// State whose API is gated behind the shared password `secret`.
    fn authed_state() -> AppState {
        AppState {
            auth: Some(AuthConfig::new("secret")),
            ..Default::default()
        }
    }

    /// Prepare and confirm the next round with no customization.
    async fn start_round(state: &AppState) {
        send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        send(router(state.clone()), post_empty("/api/tournament/rounds")).await;
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
        assert_eq!(body["tournament"]["name"], "Paris Open");
        assert_eq!(body["tournament"]["format_version"], 4);
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
        assert_eq!(body["can_undo"], false); // nothing to undo on a fresh tournament

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
        let players = body["tournament"]["players"].as_array().unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0]["last_name"], "Kobayashi");
        assert_eq!(players[0]["first_name"], "Taichi");
        assert_eq!(players[0]["nationality"], "JP"); // uppercased server-side
        assert_eq!(body["can_undo"], true); // a mutation is now undoable
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
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_and_remove_point_adjustment_over_http() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        let (_, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Alice" }),
            ),
        )
        .await;
        let id = body["tournament"]["players"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Apply a bonus.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &format!("/api/tournament/players/{id}/adjustments"),
                json!({ "delta": 2, "reason": "Fair-play bonus" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let adjustments = body["tournament"]["players"][0]["adjustments"]
            .as_array()
            .unwrap();
        assert_eq!(adjustments.len(), 1);
        assert_eq!(adjustments[0]["delta"], 2);
        assert_eq!(adjustments[0]["reason"], "Fair-play bonus");
        let adjustment_id = adjustments[0]["id"].as_str().unwrap().to_string();

        // A blank reason is rejected.
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                &format!("/api/tournament/players/{id}/adjustments"),
                json!({ "delta": 1, "reason": "  " }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Remove the bonus.
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/tournament/players/{id}/adjustments/{adjustment_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Empty adjustments are omitted from the JSON entirely (skip_serializing_if).
        assert!(body["tournament"]["players"][0]["adjustments"].is_null());

        // Removing it again is a 404.
        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/tournament/players/{id}/adjustments/{adjustment_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn edit_then_undo_reverts_step_by_step() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        let (_, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Alice", "rating": 1500 }),
            ),
        )
        .await;
        let id = body["tournament"]["players"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Edit the rating.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &format!("/api/tournament/players/{id}"),
                json!({ "last_name": "Alice", "rating": 1900 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["rating"], 1900);

        // Undo the edit → rating restored.
        let (status, body) = send(router(state.clone()), post_empty("/api/tournament/undo")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["rating"], 1500);
        assert_eq!(body["can_undo"], true); // the add is still undoable

        // Undo the add → back to empty, nothing left to undo.
        let (status, body) = send(router(state.clone()), post_empty("/api/tournament/undo")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
        assert_eq!(body["can_undo"], false);
    }

    #[tokio::test]
    async fn start_round_pairs_current_players() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        for name in ["Alice", "Bob", "Carol"] {
            send(
                router(state.clone()),
                json_req(
                    "POST",
                    "/api/tournament/players",
                    json!({ "last_name": name }),
                ),
            )
            .await;
        }
        send(
            router(state.clone()),
            post_empty("/api/tournament/finalize-registration"),
        )
        .await;
        send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;

        let (status, body) =
            send(router(state.clone()), post_empty("/api/tournament/rounds")).await;
        assert_eq!(status, StatusCode::CREATED);
        let rounds = body["tournament"]["rounds"].as_array().unwrap();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0]["number"], 1);
        assert_eq!(rounds[0]["boards"].as_array().unwrap().len(), 1); // 3 → 1 board
        assert!(rounds[0]["bye"].is_string()); // + a bye
    }

    #[tokio::test]
    async fn round_lifecycle_is_gated_over_http() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req(
                    "POST",
                    "/api/tournament/players",
                    json!({ "last_name": name }),
                ),
            )
            .await;
        }

        // Can't prepare a round before finalizing.
        let (status, _) = send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Finalize, prepare, then confirm round 1.
        let (status, _) = send(
            router(state.clone()),
            post_empty("/api/tournament/finalize-registration"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(router(state.clone()), post_empty("/api/tournament/rounds")).await;
        assert_eq!(status, StatusCode::CREATED);

        // Can't complete before the game is played.
        let (status, _) = send(
            router(state.clone()),
            post_empty("/api/tournament/complete-round"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Play the game, then complete succeeds and marks the round completed.
        send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/rounds/1/boards/0/result",
                json!({ "clicked": "player1" }),
            ),
        )
        .await;
        let (status, body) = send(
            router(state.clone()),
            post_empty("/api/tournament/complete-round"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["rounds"][0]["completed"], true);

        // Now round 2 can be prepared and started.
        send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        let (status, _) = send(router(state.clone()), post_empty("/api/tournament/rounds")).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn set_board_result_toggles_winner() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req(
                    "POST",
                    "/api/tournament/players",
                    json!({ "last_name": name }),
                ),
            )
            .await;
        }
        send(
            router(state.clone()),
            post_empty("/api/tournament/finalize-registration"),
        )
        .await;
        start_round(&state).await;

        // Click player 1 → they win.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/rounds/1/boards/0/result",
                json!({ "clicked": "player1" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["tournament"]["rounds"][0]["boards"][0]["result"],
            "player1"
        );

        // Bad board index → 404.
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/rounds/1/boards/9/result",
                json!({ "clicked": "player1" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn start_round_needs_two_players_is_400() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Solo" }),
            ),
        )
        .await;
        send(
            router(state.clone()),
            post_empty("/api/tournament/finalize-registration"),
        )
        .await;
        send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        let (status, _) = send(router(state.clone()), post_empty("/api/tournament/rounds")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn edit_missing_player_is_404() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Cup" })),
        )
        .await;
        let (status, _) = send(
            router(state),
            json_req(
                "PUT",
                &format!("/api/tournament/players/{}", uuid::Uuid::new_v4()),
                json!({ "last_name": "Ghost" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn add_player_without_tournament_is_404() {
        let (status, _) = send(
            router(AppState::default()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Alice" }),
            ),
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
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "   " }),
            ),
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
        assert_eq!(body["tournament"]["name"], "Loaded Cup");

        let (status, body) = send(router(state.clone()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["last_name"], "Bob");
    }

    #[tokio::test]
    async fn backups_are_taken_on_round_transitions_and_can_be_restored() {
        let state = AppState::default();
        send(
            router(state.clone()),
            json_req("POST", "/api/tournament", json!({ "name": "Backup Cup" })),
        )
        .await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req(
                    "POST",
                    "/api/tournament/players",
                    json!({ "last_name": name }),
                ),
            )
            .await;
        }

        // Transition 1: finalize registration.
        send(
            router(state.clone()),
            post_empty("/api/tournament/finalize-registration"),
        )
        .await;

        // A plain player edit is *not* a round-state-machine transition, so it
        // must not show up as an extra backup.
        send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournament/players",
                json!({ "last_name": "Carol" }),
            ),
        )
        .await;

        // Transitions 2 and 3: prepare, then start, round 1.
        send(
            router(state.clone()),
            post_empty("/api/tournament/rounds/prepare"),
        )
        .await;
        send(router(state.clone()), post_empty("/api/tournament/rounds")).await;

        let (status, backups) = send(router(state.clone()), get("/api/tournament/backups")).await;
        assert_eq!(status, StatusCode::OK);
        let backups = backups.as_array().unwrap();
        assert_eq!(backups.len(), 3);
        // Newest first.
        assert_eq!(backups[0]["label"], "round 1 started");
        assert_eq!(backups[1]["label"], "round 1 drafting");
        assert_eq!(backups[2]["label"], "registration finalized");

        // Restore the "registration finalized" backup: back to 2 players, no
        // rounds, but still finalized — and the later edit/rounds are gone.
        let backup_id = backups[2]["id"].as_str().unwrap();
        let (status, body) = send(
            router(state.clone()),
            post_empty(&format!("/api/tournament/backups/{backup_id}/restore")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"].as_array().unwrap().len(), 2);
        assert_eq!(body["tournament"]["registration_finalized"], true);
        assert!(body["tournament"]["rounds"].as_array().unwrap().is_empty());
        assert_eq!(body["can_undo"], false); // restoring resets undo history, like load

        // An unknown backup id is a 404.
        let (status, _) = send(
            router(state.clone()),
            post_empty("/api/tournament/backups/nonexistent-0-id/restore"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn replace_tournament_rejects_unknown_version() {
        let state = AppState::default();
        let mut future = serde_json::to_value(Tournament::new("X").unwrap()).unwrap();
        future["format_version"] = json!(999);

        let (status, _) = send(router(state), json_req("PUT", "/api/tournament", future)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn health_is_open_without_a_token() {
        let (status, body) = send(router(authed_state()), get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn protected_route_without_a_token_is_401() {
        let (status, _) = send(router(authed_state()), get("/api/tournament")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_with_the_wrong_password_is_401() {
        let (status, _) = send(
            router(authed_state()),
            json_req("POST", "/api/login", json!({ "password": "nope" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_then_use_the_token_to_reach_the_api() {
        let state = authed_state();

        // Log in with the right password to obtain the session token.
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", "/api/login", json!({ "password": "secret" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = body["token"].as_str().unwrap().to_string();
        assert!(!token.is_empty());

        // A bogus token is still rejected.
        let (status, _) = send(
            router(state.clone()),
            json_req_auth("POST", "/api/tournament", json!({ "name": "Cup" }), "wrong"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // The real token gets through and the request succeeds.
        let (status, body) = send(
            router(state.clone()),
            json_req_auth("POST", "/api/tournament", json!({ "name": "Cup" }), &token),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["tournament"]["name"], "Cup");
    }

    #[tokio::test]
    async fn login_is_404_when_auth_is_disabled() {
        let (status, _) = send(
            router(AppState::default()),
            json_req("POST", "/api/login", json!({ "password": "whatever" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serves_spa_assets_and_falls_back_to_index() {
        // A throwaway "built SPA" directory.
        let dir = std::env::temp_dir().join(format!("osp-spa-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>app</title>").unwrap();
        std::fs::write(dir.join("app.js"), "console.log('hi')").unwrap();

        let app = router_with_static(AppState::default(), dir.clone());

        // A real asset is served as-is.
        let (status, body) = send_text(app.clone(), get("/app.js")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("console.log"));

        // The root serves the app shell (ServeDir's directory index).
        let (status, body) = send_text(app.clone(), get("/")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("<title>app</title>"));

        // An unknown path still returns the app shell as a safety net (a stray
        // refresh loads the SPA rather than a blank page). tower-http keeps the
        // 404 status here, which is harmless since the app has no URL routing.
        let (status, body) = send_text(app.clone(), get("/players")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("<title>app</title>"));

        // The API still works alongside the static fallback.
        let (status, body) = send(app, get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");

        std::fs::remove_dir_all(&dir).ok();
    }
}
