//! Transport-level concerns of the router itself: which origins CORS
//! answers, and serving the built SPA alongside the API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use osp_server::{router, router_with_static, AppState};
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

mod common;
use common::*;

/// `extra_origins` adds exactly what it is given, and nothing around it.
///
/// It exists so a second checkout — Vite on another port — can be answered.
/// The risk it must not become is "any port on localhost": on a server with no
/// admin password every route is open, so a page served by any other local
/// program could otherwise read and script the whole API. So the listed origin
/// is answered and its neighbours are not.
#[tokio::test]
async fn extra_origins_are_answered_exactly_as_listed() {
    let state = AppState {
        extra_origins: std::sync::Arc::from(["http://localhost:5174".to_string()]),
        ..AppState::default()
    };
    let allow_origin = |response: &axum::response::Response| {
        response
            .headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_string())
    };
    let ask = |origin: &'static str, state: AppState| async move {
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        allow_origin(&response)
    };

    // The one that was asked for, and the built-in ones alongside it.
    assert_eq!(
        ask("http://localhost:5174", state.clone()).await.as_deref(),
        Some("http://localhost:5174")
    );
    assert_eq!(
        ask("http://localhost:5173", state.clone()).await.as_deref(),
        Some("http://localhost:5173"),
        "listing an extra origin must not displace the app's own"
    );

    // Not the same host on another port, not the loopback spelling that was not
    // listed, and not a stranger who merely contains the string.
    for origin in [
        "http://localhost:5175",
        "http://127.0.0.1:5174",
        "https://localhost:5174",
        "http://localhost:5174.evil.example",
    ] {
        assert_eq!(
            ask(origin, state.clone()).await,
            None,
            "{origin} was never listed"
        );
    }

    // And with nothing configured, the default state answers no extras at all.
    assert_eq!(
        ask("http://localhost:5174", AppState::default()).await,
        None
    );
}

/// CORS names the app's own origins. It matters most on a server with no
/// admin password, where every route is open by design: `Any` would let a
/// page on an unrelated site read and script the whole API over
/// `127.0.0.1:3000`.
#[tokio::test]
async fn cors_answers_the_apps_own_origins_and_no_others() {
    let state = AppState::default();
    let allow_origin = |response: &axum::response::Response| {
        response
            .headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_string())
    };

    // The desktop webview and the Vite dev server are the app talking to
    // itself, and are answered.
    for origin in [
        "tauri://localhost",
        "http://tauri.localhost",
        "http://localhost:5173",
    ] {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            allow_origin(&response).as_deref(),
            Some(origin),
            "{origin} is one of ours"
        );
    }

    // A stranger's page gets no grant, so the browser will not hand it the
    // body of even an unauthenticated route — including one dressed up to
    // look like ours, since the `http:` origins match exactly.
    for origin in [
        "https://evil.example",
        "http://tauri.localhost.evil.example",
        "https://tauri.localhost",
    ] {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/tournaments")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allow_origin(&response), None, "{origin} is not ours");
    }

    // ...and its preflight for a mutation is refused, so the `DELETE` that
    // preflight stands for is never sent.
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri(t(Uuid::new_v4(), ""))
                .header("origin", "https://evil.example")
                .header("access-control-request-method", "DELETE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allow_origin(&response), None);
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
