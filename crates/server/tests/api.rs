//! The router driven end-to-end through `oneshot` — real handlers, real
//! SQLite, real CAS on disk, no sockets. A failing request here is a failing
//! API, not a failing test harness.
//!
//! Fixtures are allowed to assert: an `expect` here is the test stating what
//! it assumes, the honest form, and the workspace lint denies it everywhere a
//! caller would write it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

/// A fresh vault with a registered user; returns the router and credentials.
fn vault_with_user(name: &str) -> (tempfile::TempDir, axum::Router, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let state = aperture_server::AppState::new(dir.path()).expect("state");
    let user = state
        .register_user(name, "correct horse battery", name)
        .expect("registers");
    let token = state.auth().issue_session(&user).expect("issues");
    (dir, aperture_server::build_router(state), token)
}

/// Send one request through the router and collect the JSON response.
async fn send(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder().method(method).uri(uri);
    let builder = match token {
        Some(t) => builder.header("Authorization", format!("Bearer {t}")),
        None => builder,
    };
    let request = builder
        .body(Body::from(body.unwrap_or_default()))
        .expect("request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Send one request and collect the raw bytes.
async fn send_raw(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    token: &str,
) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, bytes)
}

/// A PNG with known dimensions, for the upload tests.
fn png_bytes() -> Vec<u8> {
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&640u32.to_be_bytes());
    png.extend_from_slice(&480u32.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0]);
    png.extend_from_slice(&[0, 0, 0, 0]);
    png
}

#[tokio::test]
async fn login_rejects_bad_credentials_with_401() {
    let (_dir, mut router, _token) = vault_with_user("wyatt");
    let (status, _) = send(
        &mut router,
        "POST",
        "/api/auth/login",
        None,
        Some(br#"{"email":"wyatt","password":"wrong"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_accepts_good_credentials_and_returns_a_token() {
    let (_dir, mut router, _token) = vault_with_user("wyatt");
    let (status, json) = send(
        &mut router,
        "POST",
        "/api/auth/login",
        None,
        Some(br#"{"email":"wyatt","password":"correct horse battery"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["accessToken"].as_str().is_some(),
        "a token is returned"
    );
    assert!(json["userId"].as_str().is_some());
}

#[tokio::test]
async fn the_timeline_requires_a_token() {
    let (_dir, mut router, _token) = vault_with_user("wyatt");
    let (status, _) = send(&mut router, "GET", "/api/timeline", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_garbage_token_is_rejected() {
    let (_dir, mut router, _token) = vault_with_user("wyatt");
    let (status, _) = send(&mut router, "GET", "/api/timeline", Some("nonsense"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_then_timeline_then_fetch_round_trips() {
    let (_dir, mut router, token) = vault_with_user("wyatt");
    let png = png_bytes();

    // Upload.
    let (status, body) = send(
        &mut router,
        "POST",
        "/api/assets?filename=shot.png",
        Some(&token),
        Some(png.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["newlyWritten"], serde_json::Value::Bool(true));
    let address = body["address"].as_str().expect("address").to_string();

    // Re-upload the identical bytes: the CAS deduplicates.
    let (status, body) = send(
        &mut router,
        "POST",
        "/api/assets?filename=shot.png",
        Some(&token),
        Some(png.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["newlyWritten"], serde_json::Value::Bool(false));

    // Timeline: one asset, on the sentinel day (no EXIF in this fixture).
    let (status, timeline) = send(
        &mut router,
        "GET",
        "/api/timeline?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    if let Some(arr) = timeline.as_array() {
        assert_eq!(arr.len(), 1, "{timeline}");
    } else {
        panic!("timeline response is not an array: {timeline}");
    }

    // Fetch by address: the original bytes are served, sniffed as PNG.
    let (status, bytes) = send_raw(
        &mut router,
        "GET",
        &format!("/api/assets/{address}"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, png, "the original bytes are served");
}

#[tokio::test]
async fn an_unknown_address_is_a_404() {
    let (_dir, mut router, token) = vault_with_user("wyatt");
    let (status, _) = send_raw(
        &mut router,
        "GET",
        &format!("/api/assets/{}", "ab".repeat(32)),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
