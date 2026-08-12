//! Passwords, bearer tokens and the optimistic-concurrency version header.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use osp_server::{router, AppState, AuthConfig};
use serde_json::json;

mod common;
use common::*;

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
