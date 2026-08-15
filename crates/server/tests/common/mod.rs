//! Helpers shared by the server's integration tests.
//!
//! Each test binary uses the subset it needs, so a helper that no test in
//! *this* binary calls is not a defect — hence the `dead_code` allowance, which
//! applies to this module and nothing else.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use osp_server::{router, AppState, TournamentRegistry};
use serde_json::json;
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

/// A state whose automatic backups go to a root of this test's own, under the
/// OS temp dir.
///
/// [`AppState::default`] keeps *no* backups, deliberately: a test that never
/// mentions them must not be able to write into the real per-user data
/// directory, which is what these tests did for as long as the registry
/// resolved its own default. So a test that exercises the backup or
/// deleted-tournament endpoints has to ask for a root — and gets a private
/// one, since both listings are per-root and a shared one would show each test
/// its neighbours' files.
pub fn state_with_backups() -> AppState {
    let root = std::env::temp_dir().join(format!("osp-test-backups-{}", Uuid::new_v4()));
    AppState {
        registry: Arc::new(TournamentRegistry::new(None, Some(root))),
        ..Default::default()
    }
}

/// Send one request through the router and return (status, parsed JSON body).
pub async fn send(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
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
pub async fn send_text(app: Router, req: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

pub fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// A `GET` carrying an `Authorization: Bearer <token>` header.
pub fn get_with_token(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

pub fn post_empty(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn json_req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A JSON request carrying an `Authorization: Bearer <token>` header.
pub fn json_req_auth(
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

pub fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// A `POST` carrying a raw text body (for the CSV import endpoint).
pub fn post_text(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "text/plain")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Create a tournament named `name` (no password), returning its id.
pub async fn create(state: &AppState, name: &str) -> Uuid {
    let (status, body) = send(
        router(state.clone()),
        json_req("POST", "/api/tournaments", json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
}

/// Create a password-protected tournament, returning (id, token).
pub async fn create_with_password(state: &AppState, name: &str, password: &str) -> (Uuid, String) {
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
pub fn t(id: Uuid, suffix: &str) -> String {
    format!("/api/tournaments/{id}{suffix}")
}

/// Prepare and confirm the next round with no customization.
pub async fn start_round(state: &AppState, id: Uuid) {
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
}

/// Log in as admin, asserting success, and return the bearer token.
pub async fn admin_login(state: &AppState, password: &str) -> String {
    let (status, body) = send(
        router(state.clone()),
        json_req("POST", "/api/admin/login", json!({ "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body["token"].as_str().unwrap().to_string()
}
