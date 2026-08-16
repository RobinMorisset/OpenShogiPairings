//! The public read-only projection and its reader page (see
//! `docs/archive/public-access.md`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use osp_server::{router, router_with_static, AppState, AuthConfig};
use serde_json::json;
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

mod common;
use common::*;

// --- Public read-only access (docs/archive/public-access.md, phase 1) -----------

/// Publish `id`, returning its capability key.
async fn publish(state: &AppState, id: Uuid) -> String {
    let (status, body) = send(
        router(state.clone()),
        json_req("PUT", &t(id, "/publication"), json!({ "published": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["published"], true);
    body["key"].as_str().unwrap().to_string()
}

/// [`publish`] for a server that has an admin password: a password-less
/// tournament falls back to the admin token (see `auth::require_tournament_auth`),
/// so publishing it needs that token rather than nothing.
async fn publish_as(state: &AppState, id: Uuid, token: &str) -> String {
    let (status, body) = send(
        router(state.clone()),
        json_req_auth(
            "PUT",
            &t(id, "/publication"),
            json!({ "published": true }),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["published"], true);
    body["key"].as_str().unwrap().to_string()
}

/// A tournament with two players and round 1 under way, one board decided.
async fn played_tournament(state: &AppState, name: &str) -> Uuid {
    let id = create(state, name).await;
    for who in ["Alice", "Bob"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": who })),
        )
        .await;
    }
    start_round(state, id).await;
    id
}

#[tokio::test]
async fn a_tournament_is_unpublished_until_the_referee_says_otherwise() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;

    let (status, body) = send(router(state.clone()), get(&t(id, "/publication"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["published"], false);
    assert!(body.get("key").is_none());

    // With no key minted, no key opens it — including the empty one.
    for query in ["", "?k=", "?k=whatever"] {
        let (status, _) = send(
            router(state.clone()),
            get(&t(id, &format!("/public{query}"))),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "an unpublished tournament must not be readable via {query:?}"
        );
    }
}

#[tokio::test]
async fn the_public_projection_drops_the_draft_and_the_referee_only_fields() {
    let state = AppState::default();
    let id = played_tournament(&state, "Public Cup").await;
    // Record a result, then open a draft for the next round: the played
    // round must be public, the round being hand-tuned must not.
    send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

    let key = publish(&state, id).await;
    let (status, body) = send(
        router(state.clone()),
        get(&t(id, &format!("/public?k={key}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The referee sees a draft; the public page must not.
    let (_, referee) = send(router(state.clone()), get(&t(id, ""))).await;
    assert!(referee["tournament"]["draft"].is_object());
    assert!(body["tournament"]["draft"].is_null());
    assert!(body["undo_label"].is_null());
    assert!(body["draft_cup_players"].is_null());

    // Everything the reader is there for is present, and identical to the
    // referee's own numbers.
    assert_eq!(body["tournament"]["name"], "Public Cup");
    assert_eq!(body["standings"], referee["standings"]);
    assert_eq!(
        body["tournament"]["rounds"],
        referee["tournament"]["rounds"]
    );
    assert_eq!(
        body["tournament"]["rounds"][0]["boards"][0]["record"],
        json!({ "kind": "short", "outcome": { "kind": "won", "winner": "player1" } })
    );
}

#[tokio::test]
async fn the_suggested_handicap_is_published_only_when_the_referee_shows_it() {
    let state = AppState::default();
    let id = create(&state, "Handicaps").await;
    for (who, rating) in [("Alice", 2200), ("Bob", 1400)] {
        send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": who, "rating": rating }),
            ),
        )
        .await;
    }
    start_round(&state, id).await;
    let key = publish(&state, id).await;
    let public = format!("/public?k={key}");

    // Default policy is `allowed`: the picker, no suggestion column. The
    // referee turned the suggestion off, so the public page must not
    // contradict the room.
    let (_, body) = send(router(state.clone()), get(&t(id, &public))).await;
    assert_eq!(body["suggested_handicaps"], json!([]));
    let (_, referee) = send(router(state.clone()), get(&t(id, ""))).await;
    assert!(referee["suggested_handicaps"][0][0].is_string());

    // Switch to `suggested` and it appears.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, "/settings"),
            json!({ "handicap_policy": { "kind": "enabled", "display": "suggested" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(router(state.clone()), get(&t(id, &public))).await;
    assert_eq!(
        body["suggested_handicaps"], referee["suggested_handicaps"],
        "the suggestion is the referee's own, not a second computation"
    );
}

#[tokio::test]
async fn a_rotated_key_revokes_the_old_link_and_unpublishing_closes_the_page() {
    let state = AppState::default();
    let id = played_tournament(&state, "Cup").await;
    let first = publish(&state, id).await;
    assert_eq!(
        send(
            router(state.clone()),
            get(&t(id, &format!("/public?k={first}")))
        )
        .await
        .0,
        StatusCode::OK
    );

    // Publishing again mints a fresh key: that is how a key is rotated, and
    // every link already handed out stops working.
    let second = publish(&state, id).await;
    assert_ne!(first, second);
    assert_eq!(
        send(
            router(state.clone()),
            get(&t(id, &format!("/public?k={first}")))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            router(state.clone()),
            get(&t(id, &format!("/public?k={second}")))
        )
        .await
        .0,
        StatusCode::OK
    );

    // Unpublishing closes it for everyone.
    let (status, body) = send(
        router(state.clone()),
        json_req("PUT", &t(id, "/publication"), json!({ "published": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["published"], false);
    assert_eq!(
        send(
            router(state.clone()),
            get(&t(id, &format!("/public?k={second}")))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_public_page_needs_no_password_but_grants_no_writes() {
    let state = AppState::default();
    let (id, token) = create_with_password(&state, "Locked", "secret").await;
    let key = {
        let (status, body) = send(
            router(state.clone()),
            json_req_auth(
                "PUT",
                &t(id, "/publication"),
                json!({ "published": true }),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body["key"].as_str().unwrap().to_string()
    };

    // The reader gets in with the key alone...
    let (status, body) = send(
        router(state.clone()),
        get(&t(id, &format!("/public?k={key}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["name"], "Locked");

    // ...and the key opens nothing else: the reader routes are their own
    // group, with no mutating handler reachable from them.
    for (method, path) in [
        ("GET", format!("?k={key}")),
        ("GET", format!("/publication?k={key}")),
        ("POST", format!("/players?k={key}")),
        ("POST", format!("/undo?k={key}")),
    ] {
        let (status, _) = send(
            router(state.clone()),
            json_req(method, &t(id, &path), json!({ "last_name": "Mallory" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} must still demand the password"
        );
    }
}

/// The sibling of the test above, for the tournament that has *no* password
/// of its own. On a host with an admin password — the marker for "strangers
/// can reach me" — such a tournament must fall back to the admin token, or
/// publishing it would hand every caller of `GET /api/tournaments` an id
/// that opens every write route on it.
#[tokio::test]
async fn a_passwordless_tournament_on_an_admin_host_is_not_writable_by_strangers() {
    let state = AppState {
        admin_auth: Some(AuthConfig::new("admin-secret")),
        ..Default::default()
    };
    let token = admin_login(&state, "admin-secret").await;

    let (status, body) = send(
        router(state.clone()),
        json_req_auth(
            "POST",
            "/api/tournaments",
            json!({ "name": "Paris Open" }),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    let key = publish_as(&state, id, &token).await;

    // Publishing is what puts the id in a stranger's hands.
    let (_, body) = send(router(state.clone()), get("/api/tournaments")).await;
    assert_eq!(body["tournaments"][0]["id"], id.to_string());

    // Knowing the id must not be enough to read or write the tournament.
    for (method, path) in [
        ("GET", ""),
        ("POST", "/players"),
        ("POST", "/undo"),
        ("PUT", "/publication"),
        ("DELETE", ""),
    ] {
        let (status, _) = send(
            router(state.clone()),
            json_req(method, &t(id, path), json!({ "last_name": "Mallory" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} must demand the admin token"
        );
    }

    // The reader path is untouched: the capability key still works, with no
    // password and no token anywhere.
    let (status, body) = send(
        router(state.clone()),
        get(&t(id, &format!("/public?k={key}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["name"], "Paris Open");

    // And the admin still gets in.
    let (status, _) = send(router(state.clone()), get_with_token(&t(id, ""), &token)).await;
    assert_eq!(status, StatusCode::OK);
}

/// The open server — every local and embedded one — keeps working with no
/// credentials at all. The fallback above must not gate it.
#[tokio::test]
async fn a_passwordless_tournament_on_an_open_host_stays_open() {
    let state = AppState::default();
    let id = create(&state, "Kitchen Table").await;
    let (status, _) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "Alice" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// Phase 2's static export reads the projection through the referee's own
/// credentials, because the deployment it exists for — the desktop app —
/// has no reachable reader endpoint to key into.
#[tokio::test]
async fn the_referee_can_read_the_projection_without_publishing_it() {
    let state = AppState::default();
    let (id, token) = create_with_password(&state, "Locked", "secret").await;
    send(
        router(state.clone()),
        json_req_auth(
            "POST",
            &t(id, "/players"),
            json!({ "last_name": "Alice" }),
            &token,
        ),
    )
    .await;

    // Never published: no key exists, and none is asked for.
    let (status, body) = send(
        router(state.clone()),
        get_with_token(&t(id, "/public-snapshot"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["name"], "Locked");
    assert!(body["undo_label"].is_null());
    assert!(body["tournament"]["draft"].is_null());

    // It is the referee's route, so it wants the referee's password — the
    // file it feeds is written by whoever is running the tournament.
    let (status, _) = send(router(state.clone()), get(&t(id, "/public-snapshot"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // And it is byte-for-byte the projection a reader would be served —
    // one filter, not two.
    let key = {
        let (_, body) = send(
            router(state.clone()),
            json_req_auth(
                "PUT",
                &t(id, "/publication"),
                json!({ "published": true }),
                &token,
            ),
        )
        .await;
        body["key"].as_str().unwrap().to_string()
    };
    let (_, reader) = send(
        router(state.clone()),
        get(&t(id, &format!("/public?k={key}"))),
    )
    .await;
    let (_, referee) = send(
        router(state.clone()),
        get_with_token(&t(id, "/public-snapshot"), &token),
    )
    .await;
    assert_eq!(reader, referee);
}

#[tokio::test]
async fn the_public_payload_is_cached_by_version_and_revalidates_with_an_etag() {
    let state = AppState::default();
    let id = played_tournament(&state, "Cup").await;
    let key = publish(&state, id).await;
    let public = t(id, &format!("/public?k={key}"));

    let response = router(state.clone()).oneshot(get(&public)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // A reader that already has this version is told so, without a body.
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(&public)
                .header("if-none-match", &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

    // Any change to the tournament invalidates it.
    send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(&public)
                .header("if-none-match", &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(response.headers().get("etag").unwrap(), etag.as_str());
}

/// Two servers booting over the same data directory reuse the same version
/// numbers for different states, so the `ETag` has to distinguish them —
/// otherwise a reader holding one from before a restart is answered "you
/// already have that" forever, with nothing anywhere saying why.
#[tokio::test]
async fn the_etag_survives_a_restart() {
    let a = AppState::default();
    let b = AppState::default();
    assert_ne!(a.boot_id, b.boot_id);
}

#[tokio::test]
async fn an_unauthenticated_picker_sees_only_published_tournaments() {
    let state = AppState {
        admin_auth: Some(AuthConfig::new("admin-secret")),
        ..Default::default()
    };
    let token = admin_login(&state, "admin-secret").await;
    let (status, body) = send(
        router(state.clone()),
        json_req_auth(
            "POST",
            "/api/tournaments",
            json!({ "name": "Private" }),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let private = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    let (_, body) = send(
        router(state.clone()),
        json_req_auth(
            "POST",
            "/api/tournaments",
            json!({ "name": "Open" }),
            &token,
        ),
    )
    .await;
    let published = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    publish_as(&state, published, &token).await;

    // A stranger who found the URL sees only what opted in — and is told
    // that is what they are seeing, rather than quietly shown a short list.
    let (_, body) = send(router(state.clone()), get("/api/tournaments")).await;
    assert_eq!(body["restricted"], true);
    let names: Vec<&str> = body["tournaments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Open"]);

    // The admin token brings back the full list.
    let (_, body) = send(
        router(state.clone()),
        get_with_token("/api/tournaments", &token),
    )
    .await;
    assert_eq!(body["restricted"], false);
    assert_eq!(body["tournaments"].as_array().unwrap().len(), 2);
    let _ = private;
}

/// The reader URL is printed on a QR code and handed to a roomful of
/// people, so it must answer a plain `200` with the app shell — not the
/// `404`-with-shell the catch-all fallback gives every unknown path.
#[tokio::test]
async fn the_reader_page_url_is_served_as_a_real_page() {
    let dir = std::env::temp_dir().join(format!("osp-public-spa-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><title>app</title>").unwrap();
    let app = router_with_static(AppState::default(), dir.clone());

    let (status, body) = send_text(
        app.clone(),
        get(&format!("/t/{}/public?k=whatever", Uuid::new_v4())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<title>app</title>"));

    std::fs::remove_dir_all(&dir).ok();
}

/// The reader stream carries the whole projection, not just the version —
/// that is what makes one mutation cost one serialization instead of a
/// hundred refetches — and it sends the current state on connect rather
/// than leaving a fresh reader staring at nothing until the next change.
#[tokio::test]
async fn the_reader_stream_pushes_the_projection_on_connect() {
    use tokio_stream::StreamExt;

    let state = AppState::default();
    let id = played_tournament(&state, "Streamed Cup").await;
    let key = publish(&state, id).await;

    let response = router(state.clone())
        .oneshot(get(&t(id, &format!("/public/events?k={key}"))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
        .await
        .expect("the first event arrives without waiting for a change")
        .expect("the stream is not empty")
        .unwrap();
    let text = String::from_utf8(chunk.to_vec()).unwrap();
    assert!(text.contains("event: state"), "{text}");
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("an event carries the payload itself");
    let payload: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(payload["tournament"]["name"], "Streamed Cup");
    // Same projection as the plain GET: the draft is absent here too.
    assert!(payload["tournament"]["draft"].is_null());
    assert!(payload["undo_label"].is_null());

    // And the stream, like the GET, is closed to a wrong key.
    let (status, _) = send(router(state.clone()), get(&t(id, "/public/events?k=wrong"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Rotating the key has to reach the connections that are already open.
///
/// A stream outlives the request that opened it, so checking the capability
/// key only at connect made "New link" revoke nothing for anyone already
/// watching: they kept receiving every result down a connection opened with
/// a key that no longer existed, for as long as the tab stayed open.
#[tokio::test]
async fn rotating_the_key_cuts_off_a_reader_who_is_already_connected() {
    use tokio_stream::StreamExt;

    let state = AppState::default();
    let id = played_tournament(&state, "Cup").await;
    let key = publish(&state, id).await;

    let response = router(state.clone())
        .oneshot(get(&t(id, &format!("/public/events?k={key}"))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    /// The next SSE frame, or a panic if the stream goes quiet.
    async fn frame(body: &mut axum::body::BodyDataStream) -> String {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
            .await
            .expect("an event arrives")
            .expect("the stream is still open")
            .unwrap();
        String::from_utf8(chunk.to_vec()).unwrap()
    }

    // Connected and receiving the state, as a reader with a valid key should.
    assert!(frame(&mut body).await.contains("event: state"));

    // The referee issues a new link. The reader is cut off *now* — not at
    // the next edit — and told why, rather than being left to reconnect
    // against a 404 forever.
    let rotated = publish(&state, id).await;
    assert_ne!(rotated, key);
    let goodbye = frame(&mut body).await;
    assert!(
        goodbye.contains("event: revoked"),
        "expected a revocation, got: {goodbye}"
    );

    // And a result recorded afterwards does not reach the old connection.
    send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    let after = tokio::time::timeout(std::time::Duration::from_secs(1), body.next()).await;
    match after {
        Err(_) => {}   // nothing more arrived: the stream is silent
        Ok(None) => {} // or it ended outright
        Ok(Some(chunk)) => {
            let text = String::from_utf8(chunk.unwrap().to_vec()).unwrap();
            assert!(
                !text.contains("event: state"),
                "a revoked reader must not receive the result: {text}"
            );
        }
    }
}

/// Unpublishing is the same promise as rotating, and must reach open
/// connections the same way.
#[tokio::test]
async fn unpublishing_cuts_off_a_reader_who_is_already_connected() {
    use tokio_stream::StreamExt;

    let state = AppState::default();
    let id = played_tournament(&state, "Cup").await;
    let key = publish(&state, id).await;

    let response = router(state.clone())
        .oneshot(get(&t(id, &format!("/public/events?k={key}"))))
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(first.to_vec())
        .unwrap()
        .contains("event: state"));

    let (status, _) = send(
        router(state.clone()),
        json_req("PUT", &t(id, "/publication"), json!({ "published": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let goodbye = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
        .await
        .expect("the reader is told, not left hanging")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(goodbye.to_vec())
        .unwrap()
        .contains("event: revoked"));
}

/// A server with no admin password is deliberately open — a desktop app, a
/// club laptop — and must keep listing everything, published or not.
#[tokio::test]
async fn an_open_server_lists_every_tournament_as_before() {
    let state = AppState::default();
    create(&state, "Cup A").await;
    create(&state, "Cup B").await;
    let (_, body) = send(router(state.clone()), get("/api/tournaments")).await;
    assert_eq!(body["restricted"], false);
    assert_eq!(body["tournaments"].as_array().unwrap().len(), 2);
}
