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
//! `docs/multi-tournament.md`.

mod auth;
mod backup;
mod error;
mod live;
mod ratings;
mod registry;
mod scope;
mod state;
mod tournament;

pub use auth::AuthConfig;
pub use state::AppState;

use std::path::PathBuf;
use std::sync::Arc;

use axum::{middleware, routing::get, Json, Router};
use osp_core::HealthStatus;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::TournamentRegistry;

/// Build the API-only router around the given state.
///
/// `/api/health` and `GET /api/tournaments` are always open; creating a
/// tournament and the FESA ratings proxy (`/api/ratings*`) may require
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
    // Permissive CORS: in dev the SPA is served cross-origin from the Vite dev
    // server (:5173), and the desktop app talks from the `tauri://` /
    // `http://tauri.localhost` webview origin. The hosted server serves the SPA
    // same-origin (see `router_with_static`), so this only matters for those.
    let cors = CorsLayer::permissive();

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
        app = app.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
    }

    app.layer(cors).layer(TraceLayer::new_for_http())
}

/// Serve the API on an already-bound listener until the process ends, with an
/// empty in-memory registry and no admin auth — the fully open, throwaway
/// configuration (tests, quick manual runs). The Tauri desktop app uses
/// [`serve_with_config`] instead, so its tournaments persist across restarts.
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
    /// Gates `POST /api/tournaments` (creating new tournaments); `None` lets
    /// anyone create one (fine on a trusted machine, risky on a public host
    /// whose URL might circulate beyond its referees).
    pub admin_password: Option<String>,
    /// Directory of the built SPA to serve same-origin; `None` = API only.
    pub static_dir: Option<PathBuf>,
    /// Directory holding one file per tournament, loaded on boot and written
    /// through to on every change; `None` = in-memory only (lost on restart).
    pub data_dir: Option<PathBuf>,
}

/// Serve the standalone server with the given [`ServerConfig`] (remote mode).
pub async fn serve_with_config(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> std::io::Result<()> {
    let state = AppState {
        registry: Arc::new(TournamentRegistry::new(config.data_dir)),
        admin_auth: config.admin_password.map(AuthConfig::new),
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
    use uuid::Uuid;

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

    fn delete(uri: &str) -> Request<Body> {
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    /// Create a tournament named `name` (no password), returning its id.
    async fn create(state: &AppState, name: &str) -> Uuid {
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", "/api/tournaments", json!({ "name": name })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }

    /// Create a password-protected tournament, returning (id, token).
    async fn create_with_password(state: &AppState, name: &str, password: &str) -> (Uuid, String) {
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournaments",
                json!({ "name": name, "password": password }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
        let token = body["token"].as_str().unwrap().to_string();
        (id, token)
    }

    /// Path scoped to tournament `id`, e.g. `t(id, "/players")`.
    fn t(id: Uuid, suffix: &str) -> String {
        format!("/api/tournaments/{id}{suffix}")
    }

    /// Prepare and confirm the next round with no customization.
    async fn start_round(state: &AppState, id: Uuid) {
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (status, body) = send(router(AppState::default()), get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn get_tournament_is_404_for_an_unknown_id() {
        let (status, body) = send(router(AppState::default()), get(&t(Uuid::new_v4(), ""))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn create_then_add_and_remove_player() {
        let state = AppState::default();
        let id = create(&state, "Paris Open").await;

        let (status, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["name"], "Paris Open");
        assert_eq!(body["tournament"]["format_version"], 4);
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
        assert_eq!(body["can_undo"], false); // nothing to undo on a fresh tournament

        // Register a player.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
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
            delete(&t(id, &format!("/players/{player_id}"))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_and_remove_point_adjustment_over_http() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        let (_, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "Alice" })),
        )
        .await;
        let pid = body["tournament"]["players"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Apply a bonus.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, &format!("/players/{pid}/adjustments")),
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
                &t(id, &format!("/players/{pid}/adjustments")),
                json!({ "delta": 1, "reason": "  " }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Remove the bonus.
        let (status, body) = send(
            router(state.clone()),
            delete(&t(
                id,
                &format!("/players/{pid}/adjustments/{adjustment_id}"),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Empty adjustments are omitted from the JSON entirely (skip_serializing_if).
        assert!(body["tournament"]["players"][0]["adjustments"].is_null());

        // Removing it again is a 404.
        let (status, _) = send(
            router(state.clone()),
            delete(&t(
                id,
                &format!("/players/{pid}/adjustments/{adjustment_id}"),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn edit_then_undo_reverts_step_by_step() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        let (_, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": "Alice", "rating": 1500 }),
            ),
        )
        .await;
        let pid = body["tournament"]["players"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Edit the rating.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, &format!("/players/{pid}")),
                json!({ "last_name": "Alice", "rating": 1900 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["rating"], 1900);

        // Undo the edit → rating restored.
        let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["rating"], 1500);
        assert_eq!(body["can_undo"], true); // the add is still undoable

        // Undo the add → back to empty, nothing left to undo.
        let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
        assert_eq!(body["can_undo"], false);
    }

    #[tokio::test]
    async fn start_round_pairs_current_players() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        for name in ["Alice", "Bob", "Carol"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }
        send(
            router(state.clone()),
            post_empty(&t(id, "/finalize-registration")),
        )
        .await;
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

        let (status, body) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
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
        let id = create(&state, "Cup").await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }

        // Preparing round 1 auto-finalizes registration in the same step, then
        // confirm round 1.
        let (status, body) =
            send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["registration_finalized"], true);
        let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
        assert_eq!(status, StatusCode::CREATED);

        // Round 1 isn't complete until its game is played, so round 2 can't
        // start yet.
        let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Recording the game's result completes the round automatically.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/rounds/1/boards/0/result"),
                json!({ "clicked": "player1" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["rounds"][0]["completed"], true);

        // Now round 2 can be prepared and started.
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn preparing_round_one_finalizes_as_a_single_undo_step() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }

        // Preparing round 1 both finalizes registration and opens the draft.
        let (status, body) =
            send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["registration_finalized"], true);
        assert!(body["tournament"]["draft"].is_object());

        // A single undo reverts *both*: back to open registration, no draft.
        // (Were it two steps, one undo would leave registration still finalized.)
        let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["registration_finalized"], false);
        assert!(body["tournament"]["draft"].is_null());
    }

    #[tokio::test]
    async fn set_board_result_toggles_winner() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }
        send(
            router(state.clone()),
            post_empty(&t(id, "/finalize-registration")),
        )
        .await;
        start_round(&state, id).await;

        // Click player 1 → they win.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/rounds/1/boards/0/result"),
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
                &t(id, "/rounds/1/boards/9/result"),
                json!({ "clicked": "player1" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn start_round_needs_two_players_is_400() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "Solo" })),
        )
        .await;
        send(
            router(state.clone()),
            post_empty(&t(id, "/finalize-registration")),
        )
        .await;
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn edit_missing_player_is_404() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, &format!("/players/{}", Uuid::new_v4())),
                json!({ "last_name": "Ghost" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn add_player_to_an_unknown_tournament_is_404() {
        let (status, _) = send(
            router(AppState::default()),
            json_req(
                "POST",
                &t(Uuid::new_v4(), "/players"),
                json!({ "last_name": "Alice" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn blank_player_name_is_400() {
        let state = AppState::default();
        let id = create(&state, "Paris Open").await;
        let (status, _) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "   " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn replace_tournament_loads_a_saved_file() {
        let state = AppState::default();
        let id = create(&state, "Original").await;

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
            json_req("PUT", &t(id, ""), saved_json),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["name"], "Loaded Cup");
        // The registry's id wins over whatever id the uploaded file carried.
        assert_eq!(body["tournament"]["id"], id.to_string());

        let (status, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["players"][0]["last_name"], "Bob");
    }

    #[tokio::test]
    async fn backups_are_taken_on_round_transitions_and_can_be_restored() {
        let state = AppState::default();
        let id = create(&state, "Backup Cup").await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }

        // Transition 1: finalize registration.
        send(
            router(state.clone()),
            post_empty(&t(id, "/finalize-registration")),
        )
        .await;

        // A plain player edit is *not* a round-state-machine transition, so it
        // must not show up as an extra backup.
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "Carol" })),
        )
        .await;

        // Transitions 2 and 3: prepare, then start, round 1.
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;

        let (status, backups) = send(router(state.clone()), get(&t(id, "/backups"))).await;
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
            post_empty(&t(id, &format!("/backups/{backup_id}/restore"))),
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
            post_empty(&t(id, "/backups/nonexistent-0-id/restore")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn replace_tournament_rejects_unknown_version() {
        let state = AppState::default();
        let id = create(&state, "X").await;
        let mut future = serde_json::to_value(Tournament::new("X").unwrap()).unwrap();
        future["format_version"] = json!(999);

        let (status, _) = send(router(state), json_req("PUT", &t(id, ""), future)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn health_is_open_without_a_token() {
        let state = AppState {
            admin_auth: Some(AuthConfig::new("admin-secret")),
            ..Default::default()
        };
        let (status, body) = send(router(state), get("/api/health")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn protected_tournament_route_without_a_token_is_401() {
        let state = AppState::default();
        let (id, _token) = create_with_password(&state, "Cup", "secret").await;
        let (status, _) = send(router(state), get(&t(id, ""))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_with_the_wrong_password_is_401() {
        let state = AppState::default();
        let (id, _token) = create_with_password(&state, "Cup", "secret").await;
        let (status, _) = send(
            router(state),
            json_req("POST", &t(id, "/login"), json!({ "password": "nope" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_then_use_the_token_to_reach_the_api() {
        let state = AppState::default();
        let (id, _token) = create_with_password(&state, "Cup", "secret").await;

        // Log in with the right password to obtain the session token.
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/login"), json!({ "password": "secret" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = body["token"].as_str().unwrap().to_string();
        assert!(!token.is_empty());

        // A bogus token is still rejected.
        let (status, _) = send(
            router(state.clone()),
            json_req_auth(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": "Alice" }),
                "wrong",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // The real token gets through and the request succeeds.
        let (status, body) = send(
            router(state.clone()),
            json_req_auth(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": "Alice" }),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["tournament"]["players"][0]["last_name"], "Alice");
    }

    #[tokio::test]
    async fn login_is_404_when_the_tournament_has_no_password() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        let (status, _) = send(
            router(state),
            json_req("POST", &t(id, "/login"), json!({ "password": "whatever" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stale_version_header_is_409_but_matching_one_passes() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;

        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
        let version = body["version"].as_u64().unwrap();

        // A stale base version is rejected with 409.
        let stale = version - 1;
        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("POST")
                .uri(t(id, "/players"))
                .header("content-type", "application/json")
                .header("x-tournament-version", stale.to_string())
                .body(Body::from(json!({ "last_name": "Stale" }).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // The current version is accepted, and bumps the version again.
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .method("POST")
                .uri(t(id, "/players"))
                .header("content-type", "application/json")
                .header("x-tournament-version", version.to_string())
                .body(Body::from(json!({ "last_name": "Fresh" }).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(body["version"].as_u64().unwrap() > version);
    }

    #[tokio::test]
    async fn a_malformed_version_header_is_a_400_not_a_skipped_check() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;

        // A present-but-unparseable version must be a hard error, not silently
        // treated like "no header" (which would drop conflict detection).
        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("POST")
                .uri(t(id, "/players"))
                .header("content-type", "application/json")
                .header("x-tournament-version", "not-a-number")
                .body(Body::from(json!({ "last_name": "Nope" }).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // The rejected request changed nothing.
        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    }

    /// Log in as admin, asserting success, and return the bearer token.
    async fn admin_login(state: &AppState, password: &str) -> String {
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", "/api/admin/login", json!({ "password": password })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn admin_password_gates_tournament_creation() {
        let state = AppState {
            admin_auth: Some(AuthConfig::new("admin-secret")),
            ..Default::default()
        };

        // No token → rejected, nothing created.
        let (status, _) = send(
            router(state.clone()),
            json_req("POST", "/api/tournaments", json!({ "name": "Cup" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (_, list) = send(router(state.clone()), get("/api/tournaments")).await;
        assert!(list.as_array().unwrap().is_empty());

        // Wrong password at login → 401.
        let (status, _) = send(
            router(state.clone()),
            json_req("POST", "/api/admin/login", json!({ "password": "nope" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Log in, then use the token to create a tournament.
        let token = admin_login(&state, "admin-secret").await;
        let (status, _) = send(
            router(state.clone()),
            json_req_auth("POST", "/api/tournaments", json!({ "name": "Cup" }), &token),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn admin_login_is_404_when_no_admin_password_is_configured() {
        let (status, _) = send(
            router(AppState::default()),
            json_req(
                "POST",
                "/api/admin/login",
                json!({ "password": "whatever" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_password_gates_the_ratings_proxy() {
        let state = AppState {
            admin_auth: Some(AuthConfig::new("admin-secret")),
            ..Default::default()
        };

        let (status, _) = send(router(state.clone()), get("/api/ratings")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let token = admin_login(&state, "admin-secret").await;
        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .uri("/api/ratings")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        // Reaches the handler (no admin-auth 401); it may still 502 in this
        // test environment since it has no cache and no network access.
        assert_ne!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn creating_two_tournaments_keeps_them_isolated() {
        let state = AppState::default();
        let a = create(&state, "Cup A").await;
        let b = create(&state, "Cup B").await;

        send(
            router(state.clone()),
            json_req("POST", &t(a, "/players"), json!({ "last_name": "Alice" })),
        )
        .await;

        // B is untouched by A's mutation.
        let (_, body_a) = send(router(state.clone()), get(&t(a, ""))).await;
        let (_, body_b) = send(router(state.clone()), get(&t(b, ""))).await;
        assert_eq!(body_a["tournament"]["players"].as_array().unwrap().len(), 1);
        assert!(body_b["tournament"]["players"]
            .as_array()
            .unwrap()
            .is_empty());

        // Undo history is per-tournament: undoing B (no history) is a no-op,
        // and does not touch A.
        let (status, body_b) = send(router(state.clone()), post_empty(&t(b, "/undo"))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body_b["tournament"]["players"]
            .as_array()
            .unwrap()
            .is_empty());
        let (_, body_a) = send(router(state.clone()), get(&t(a, ""))).await;
        assert_eq!(body_a["tournament"]["players"].as_array().unwrap().len(), 1);

        let names: Vec<String> = state.registry.list().into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"Cup A".to_string()));
        assert!(names.contains(&"Cup B".to_string()));
    }

    #[tokio::test]
    async fn deleting_a_tournament_removes_it_from_the_registry() {
        let state = AppState::default();
        let id = create(&state, "Disposable Cup").await;

        let (status, _) = send(router(state.clone()), delete(&t(id, ""))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = send(router(state.clone()), get(&t(id, ""))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (_, list) = send(router(state.clone()), get("/api/tournaments")).await;
        assert!(list.as_array().unwrap().is_empty());

        // Deleting again is a 404, not a panic.
        let (status, _) = send(router(state), delete(&t(id, ""))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serves_spa_assets_and_falls_back_to_index() {
        // A throwaway "built SPA" directory.
        let dir = std::env::temp_dir().join(format!("osp-spa-{}", Uuid::new_v4()));
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
