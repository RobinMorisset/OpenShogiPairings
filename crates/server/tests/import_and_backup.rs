//! Importing a saved file, and the automatic backups taken on round
//! transitions.

use axum::http::StatusCode;
use osp_core::Tournament;
use osp_server::{router, AppState};
use serde_json::json;

mod common;
use common::*;

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

    let (status, listing) = send(router(state.clone()), get(&t(id, "/backups"))).await;
    assert_eq!(status, StatusCode::OK);
    // The listing says where the files are, so the referee can reach them
    // outside the app; the directory is this tournament's own.
    let directory = listing["directory"].as_str().expect("a backups directory");
    assert!(
        directory.ends_with(&id.to_string()),
        "the backups directory is the tournament's own: {directory}"
    );
    let backups = listing["backups"].as_array().unwrap();
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
    (status, listed["tournaments"].as_array().unwrap().len())
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
