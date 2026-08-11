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
mod error_codes;
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

    /// A `GET` carrying an `Authorization: Bearer <token>` header.
    fn get_with_token(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
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

    /// A `POST` carrying a raw text body (for the CSV import endpoint).
    fn post_text(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "text/plain")
            .body(Body::from(body.to_string()))
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
        assert_eq!(body["tournament"]["format_version"], 9);
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
    async fn import_players_csv_registers_the_roster_as_one_undo_step() {
        let state = AppState::default();
        let id = create(&state, "Paris Open").await;

        // Reordered columns, a semicolon delimiter, and a French header — all
        // handled by the shared osp-core parser.
        let csv = "Prénom;Nom;Classement;Club\n\
                   Ann;Alpha;2000;Paris\n\
                   Bo;Beta;;Lyon\n";
        let (status, body) = send(
            router(state.clone()),
            post_text(&t(id, "/players/import-csv"), csv),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let players = body["tournament"]["players"].as_array().unwrap();
        assert_eq!(players.len(), 2);
        assert_eq!(players[0]["last_name"], "Alpha");
        assert_eq!(players[0]["first_name"], "Ann");
        assert_eq!(players[0]["rating"], 2000);
        assert_eq!(players[0]["club"], "Paris");
        // The whole import is a single mutation, so one undo clears all of it.
        assert_eq!(body["can_undo"], true);
        let (_, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
        assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_players_csv_rejects_a_malformed_file() {
        let state = AppState::default();
        let id = create(&state, "Paris Open").await;

        // A header with no last-name/first-name column → 400, nothing imported.
        let (status, body) = send(
            router(state.clone()),
            post_text(&t(id, "/players/import-csv"), "ELO,Club\n2000,Paris\n"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("last-name"));
        // A stable machine code accompanies the message so the client localizes it.
        assert_eq!(body["code"], "csv_missing_name_columns");

        // A row with a blank last name → 400 with the offending rows for the client.
        let (status, body) = send(
            router(state.clone()),
            post_text(
                &t(id, "/players/import-csv"),
                "Last name,First name\nAlpha,Ann\n,Bo\n",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "csv_rows_missing_last_name");
        assert_eq!(body["values"]["rows"], "3"); // the second data row (file row 3)
                                                 // All-or-nothing: nothing was imported.
        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
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
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

        let (status, body) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
        assert_eq!(status, StatusCode::CREATED);
        let rounds = body["tournament"]["rounds"].as_array().unwrap();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0]["number"], 1);
        assert_eq!(rounds[0]["boards"].as_array().unwrap().len(), 1); // 3 → 1 board

        // + a bye, serialized as the round's one sit-out.
        let sitouts = rounds[0]["sitouts"].as_array().unwrap();
        assert_eq!(sitouts.len(), 1);
        assert!(sitouts[0]["player"].is_number());
        assert_eq!(sitouts[0]["kind"], "bye");
        assert_eq!(sitouts[0]["value"], "full");
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

    /// The team roster routes, end to end: a team tournament is configured,
    /// rostered, finalized and paired entirely over HTTP.
    #[tokio::test]
    async fn team_rosters_are_built_and_played_over_http() {
        let state = AppState::default();
        let id = create(&state, "Teams").await;

        // Switch to team mode with two players per team.
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, "/settings"),
                json!({ "teams": { "size": 2 } }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Four players.
        let mut players = Vec::new();
        for name in ["Alpha", "Beta", "Gamma", "Delta"] {
            let (_, body) = send(
                router(state.clone()),
                json_req(
                    "POST",
                    &t(id, "/players"),
                    json!({ "last_name": name, "rating": 1500 }),
                ),
            )
            .await;
            let list = body["tournament"]["players"].as_array().unwrap();
            players.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
        }

        // Two teams, two members each.
        let mut teams = Vec::new();
        for name in ["East", "West"] {
            let (status, body) = send(
                router(state.clone()),
                json_req("POST", &t(id, "/teams"), json!({ "name": name })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let list = body["tournament"]["teams"].as_array().unwrap();
            teams.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
        }
        for (i, player) in players.iter().enumerate() {
            let (status, _) = send(
                router(state.clone()),
                json_req(
                    "POST",
                    &t(id, &format!("/teams/{}/members", teams[i / 2])),
                    json!({ "player_id": player }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        // A duplicate name is refused, with the code the client localizes.
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/teams"), json!({ "name": " east " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "duplicate_team_name");

        // Reorder one team's boards, then reset the order by rating.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, &format!("/teams/{}/board-order", teams[0])),
                json!({ "order": [players[1], players[0]] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["tournament"]["teams"][0]["members"][0],
            json!(players[1])
        );
        send(
            router(state.clone()),
            post_empty(&t(id, &format!("/teams/{}/sort-by-rating", teams[0]))),
        )
        .await;

        // Pair round 1: one match, two boards, and the team standings appear.
        start_round(&state, id).await;
        let (status, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["tournament"]["rounds"][0]["boards"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let table = body["team_standings"].as_array().unwrap();
        assert_eq!(table.len(), 2);
        assert!(table[0]["members"].as_array().unwrap().len() == 2);

        // A finalized team tournament takes no late registration.
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "Late" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "no_late_registration_in_team_mode");
    }

    /// The draft route dispatches on the tournament's mode: a team round takes
    /// forced *matches*, and the player-level lists are refused rather than
    /// quietly dropped.
    #[tokio::test]
    async fn a_team_draft_takes_forced_matches_and_refuses_forced_boards() {
        let state = AppState::default();
        let id = create(&state, "Teams").await;
        send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, "/settings"),
                json!({ "teams": { "size": 2 } }),
            ),
        )
        .await;
        let mut players = Vec::new();
        for i in 0..8 {
            let (_, body) = send(
                router(state.clone()),
                json_req(
                    "POST",
                    &t(id, "/players"),
                    json!({ "last_name": format!("P{i}"), "rating": 2000 - i * 10 }),
                ),
            )
            .await;
            let list = body["tournament"]["players"].as_array().unwrap();
            players.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
        }
        for k in 0..4 {
            let (_, body) = send(
                router(state.clone()),
                json_req("POST", &t(id, "/teams"), json!({ "name": format!("T{k}") })),
            )
            .await;
            let team = body["tournament"]["teams"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string();
            for player in &players[k * 2..k * 2 + 2] {
                send(
                    router(state.clone()),
                    json_req(
                        "POST",
                        &t(id, &format!("/teams/{team}/members")),
                        json!({ "player_id": player }),
                    ),
                )
                .await;
            }
        }
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

        // A player-level forced board is refused, naming why.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, "/draft"),
                json!({ "forced_boards": [{ "player1": 1, "player2": 3 }] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("paired by team"));

        // A forced *match* is taken, and honoured when the round is confirmed.
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "PUT",
                &t(id, "/draft"),
                json!({ "forced_matches": [{ "team1": 1, "team2": 4 }] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["tournament"]["draft"]["forced_matches"],
            json!([{ "team1": 1, "team2": 4 }])
        );
        send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;

        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
        let boards = body["tournament"]["rounds"][0]["boards"]
            .as_array()
            .unwrap();
        // Teams 1 and 4 are board 1 and board 2 of the forced match, so their
        // two boards are the ones tagged `forced`.
        let forced: Vec<&serde_json::Value> = boards
            .iter()
            .filter(|b| b["source"]["kind"] == "forced")
            .collect();
        assert_eq!(forced.len(), 2, "{boards:#?}");
    }

    /// An individual tournament carries no team table at all, rather than an
    /// empty one that would read as "no teams yet".
    #[tokio::test]
    async fn an_individual_tournament_has_no_team_standings() {
        let state = AppState::default();
        let id = create(&state, "Solo").await;
        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert!(body.get("team_standings").is_none());
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
            body["tournament"]["rounds"][0]["boards"][0]["outcome"],
            json!({ "kind": "won", "winner": "player1" })
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

    /// A misspelled body field must be rejected, not ignored: silently dropping
    /// a typo'd `cup_size` would finalize round 1 with no cup bracket at all,
    /// and the referee would only notice rounds later.
    #[tokio::test]
    async fn prepare_round_rejects_an_unknown_body_field() {
        let state = AppState::default();
        let id = create(&state, "Cup").await;
        for name in ["Alice", "Bob"] {
            send(
                router(state.clone()),
                json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
            )
            .await;
        }
        // Axum's own body rejection, so the response is plain text, not our
        // JSON error envelope.
        let (status, body) = send_text(
            router(state.clone()),
            json_req("POST", &t(id, "/rounds/prepare"), json!({ "cupsize": 8 })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("cupsize"), "unhelpful rejection: {body}");

        // ...and nothing was mutated by the rejected call.
        let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
        assert_eq!(body["tournament"]["registration_finalized"], false);
        assert!(body["tournament"]["draft"].is_null());
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

    /// A save file with one player, as `saveTournament` writes it.
    fn saved_file(name: &str) -> serde_json::Value {
        let mut saved = Tournament::new(name).unwrap();
        saved
            .add_player(osp_core::NewPlayer {
                last_name: "Bob".into(),
                ..Default::default()
            })
            .unwrap();
        serde_json::to_value(&saved).unwrap()
    }

    #[tokio::test]
    async fn import_creates_a_tournament_from_a_saved_file() {
        let state = AppState::default();
        let saved = saved_file("Loaded Cup");
        let file_id = saved["id"].clone();

        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournaments/import",
                json!({ "tournament": saved }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = body["id"].as_str().unwrap().to_string();
        // The registry mints the id; the file's own is overwritten, so importing
        // the same file twice can't collide.
        assert_ne!(json!(id), file_id);
        // No password asked for, so no token to hand back.
        assert!(body.get("token").is_none());

        let (status, body) = send(
            router(state.clone()),
            get(&format!("/api/tournaments/{id}")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["name"], "Loaded Cup");
        assert_eq!(body["tournament"]["id"], id);
        assert_eq!(body["tournament"]["players"][0]["last_name"], "Bob");
    }

    #[tokio::test]
    async fn import_can_set_the_tournament_password() {
        let state = AppState::default();
        let (status, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournaments/import",
                json!({ "tournament": saved_file("Locked"), "password": "hunter2" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = body["id"].as_str().unwrap().to_string();
        // The importer gets a session token straight away, as when creating.
        let token = body["token"].as_str().unwrap().to_string();

        // The tournament really is protected...
        let (status, _) = send(
            router(state.clone()),
            get(&format!("/api/tournaments/{id}")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // ...and the returned token opens it.
        let (status, body) = send(
            router(state.clone()),
            get_with_token(&format!("/api/tournaments/{id}"), &token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tournament"]["name"], "Locked");
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

        // Transitions 1 and 2: prepare (which also finalizes registration, as a
        // single step), then start, round 1.
        send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
        send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;

        // A plain player edit is *not* a round-state-machine transition, so it
        // must not show up as an extra backup.
        let (status, _) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": "Carol" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, backups) = send(router(state.clone()), get(&t(id, "/backups"))).await;
        assert_eq!(status, StatusCode::OK);
        let backups = backups.as_array().unwrap();
        assert_eq!(backups.len(), 2);
        // Newest first.
        assert_eq!(backups[0]["label"], "round 1 started");
        assert_eq!(backups[1]["label"], "round 1 drafting");

        // Restore the "round 1 drafting" backup: back to 2 players and no
        // started round, but already finalized — the later edit is gone too.
        let backup_id = backups[1]["id"].as_str().unwrap();
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

    /// Import `file` and return the status plus the resulting registry listing,
    /// so a rejection can be checked to have registered *nothing*.
    async fn import(state: &AppState, file: serde_json::Value) -> (StatusCode, usize) {
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournaments/import",
                json!({ "tournament": file }),
            ),
        )
        .await;
        let (_, listed) = send(router(state.clone()), get("/api/tournaments")).await;
        (status, listed.as_array().unwrap().len())
    }

    #[tokio::test]
    async fn import_rejects_a_file_this_build_cannot_read_without_registering_it() {
        let state = AppState::default();
        let mut future = saved_file("X");
        future["format_version"] = json!(999);

        // The whole reason import is one request: a rejected file must leave no
        // half-created tournament behind for the referee to clean up.
        let (status, registered) = import(&state, future).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(registered, 0, "a rejected import must register nothing");
    }

    #[tokio::test]
    async fn import_rejects_a_blank_name_that_no_constructor_would_have_allowed() {
        let state = AppState::default();
        let mut blank = saved_file("X");
        blank["name"] = json!("   ");

        let (status, registered) = import(&state, blank).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(registered, 0);
    }

    #[tokio::test]
    async fn import_rejects_an_unknown_field_rather_than_dropping_it() {
        let state = AppState::default();
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                "/api/tournaments/import",
                json!({ "tournament": saved_file("X"), "pasword": "typo" }),
            ),
        )
        .await;
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
