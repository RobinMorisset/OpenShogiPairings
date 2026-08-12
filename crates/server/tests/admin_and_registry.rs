//! The admin password, and the registry it gates: creating, listing,
//! deleting and restoring tournaments.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use osp_server::{router, AppState, AuthConfig, TournamentRegistry};
use serde_json::json;
use uuid::Uuid;

mod common;
use common::*;

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
    assert!(list["tournaments"].as_array().unwrap().is_empty());

    // Wrong password at login → 401.
    let (status, _) = send(
        router(state.clone()),
        json_req("POST", "/api/admin/login", json!({ "password": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The bin is gated by the same password: it names tournaments somebody
    // deliberately deleted, and restoring out of it mints one.
    let (status, _) = send(router(state.clone()), get("/api/tournaments/deleted")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Log in, then use the token to create a tournament.
    let token = admin_login(&state, "admin-secret").await;
    let (status, _) = send(
        router(state.clone()),
        json_req_auth("POST", "/api/tournaments", json!({ "name": "Cup" }), &token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        router(state.clone()),
        get_with_token("/api/tournaments/deleted", &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A state whose backups root is this test's alone: the deleted-tournament
/// listing is per *root*, so on the shared test root these tests would see
/// each other's bins.
fn state_with_own_bin() -> AppState {
    let root = std::env::temp_dir().join(format!("osp-bin-{}", Uuid::new_v4()));
    AppState {
        registry: Arc::new(TournamentRegistry::with_retention(
            None,
            Some(root),
            std::time::Duration::from_secs(30 * 24 * 60 * 60),
        )),
        ..Default::default()
    }
}

/// A `DELETE` carrying an `Authorization: Bearer <token>` header.
fn delete_with_token(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn a_deleted_tournament_is_listed_and_can_be_restored() {
    let state = state_with_own_bin();
    let id = create(&state, "Doomed Cup").await;
    let (status, _) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "Alice" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        router(state.clone()),
        delete(&format!("/api/tournaments/{id}")),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The bin holds it, with the backup taken as it was deleted — the player
    // added after the last round transition is in there, which is the whole
    // point of taking that one.
    let (status, bin) = send(router(state.clone()), get("/api/tournaments/deleted")).await;
    assert_eq!(status, StatusCode::OK);
    let entries = bin.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], id.to_string());
    assert_eq!(entries[0]["name"], "Doomed Cup");
    assert_eq!(entries[0]["has_password"], false);
    let backups = entries[0]["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0]["label"], "deleted");
    // Nothing wrong with it, so the picker offers to restore it: `problem`
    // is what turns that button off.
    assert!(entries[0].get("problem").is_none());

    // A password for a tournament that had none is refused rather than
    // silently protecting the restored copy with something never set on it.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &format!("/api/tournaments/deleted/{id}/restore"),
            json!({ "password": "invented" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Restoring brings it back as *itself*, under the same id.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &format!("/api/tournaments/deleted/{id}/restore"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let restored = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    assert_eq!(restored, id, "a restored tournament is the same tournament");
    assert!(body["token"].is_null(), "it had no password");
    let (status, view) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tournament"]["name"], "Doomed Cup");
    assert_eq!(view["tournament"]["players"].as_array().unwrap().len(), 1);

    // It is not deleted any more, so it is out of the bin — and its backups
    // are its own again, the ones taken before the deletion included.
    let (_, bin) = send(router(state.clone()), get("/api/tournaments/deleted")).await;
    assert!(bin.as_array().unwrap().is_empty());
    let (status, listing) = send(router(state.clone()), get(&t(id, "/backups"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["backups"].as_array().unwrap().len(), 1);
    assert_eq!(listing["backups"][0]["label"], "deleted");

    // Restoring it again would overwrite the tournament now in use.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &format!("/api/tournaments/deleted/{id}/restore"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // An id that was never deleted is a 404, not an empty restore.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &format!("/api/tournaments/deleted/{}/restore", Uuid::new_v4()),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn restoring_a_protected_tournament_needs_the_password_it_had() {
    let state = state_with_own_bin();
    let (id, token) = create_with_password(&state, "Locked Cup", "secret").await;
    let (status, _) = send(
        router(state.clone()),
        delete_with_token(&format!("/api/tournaments/{id}"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, bin) = send(router(state.clone()), get("/api/tournaments/deleted")).await;
    assert_eq!(bin[0]["has_password"], true);

    // No password, and the wrong one, are both refused — the hash outlived
    // the tournament precisely so the bin can't be used to strip a password.
    // 403 rather than 401: nothing is wrong with the caller's session, so
    // the client asks for this tournament's password instead of signing out.
    for body in [json!({}), json!({ "password": "nope" })] {
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                &format!("/api/tournaments/deleted/{id}/restore"),
                body,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // The right one restores it — still protected, by the same password.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &format!("/api/tournaments/deleted/{id}/restore"),
            json!({ "password": "secret" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let restored = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    assert_eq!(restored, id);
    let restored_token = body["token"].as_str().expect("a session token");
    let (status, _) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, view) = send(
        router(state.clone()),
        get_with_token(&t(id, ""), restored_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tournament"]["name"], "Locked Cup");

    // The token it was deleted with is dead: restoring rebuilds the auth,
    // so an old bearer token cannot come back to life with the tournament.
    let (status, _) = send(router(state.clone()), get_with_token(&t(id, ""), &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
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

    let names: Vec<String> = state
        .registry
        .list(false)
        .into_iter()
        .map(|s| s.name)
        .collect();
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
    assert!(list["tournaments"].as_array().unwrap().is_empty());

    // Deleting again is a 404, not a panic.
    let (status, _) = send(router(state), delete(&t(id, ""))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
