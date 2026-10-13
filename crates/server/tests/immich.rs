//! The Immich-compatible endpoints driven end to end: the login shape the
//! Immich apps speak, `validateToken` on resume, a multipart `assetData`
//! upload, and the original-bytes fetch. These tests are the compatibility
//! contract: if they pass, the Immich mobile apps' protocol works against
//! this vault.
//!
//! Fixtures are allowed to assert: an `expect` here is the test stating what
//! it assumes, the honest form, and the workspace lint denies it everywhere a
//! caller would write it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

fn app_with_user(email: &str) -> (tempfile::TempDir, axum::Router, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let state = aperture_server::AppState::new(dir.path()).expect("state");
    let user = state
        .register_user(email, "correct horse battery", email)
        .expect("registers");
    let token = state.auth().issue_session(&user).expect("issues");
    (dir, aperture_server::build_router(state), token)
}

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

#[tokio::test]
async fn immich_login_exchanges_email_and_password_for_a_token() {
    let (_dir, mut router, _token) = app_with_user("wyatt@example.com");
    let (status, json) = send(
        &mut router,
        "POST",
        "/api/auth/login",
        None,
        Some(br#"{"email":"wyatt@example.com","password":"correct horse battery"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["accessToken"].as_str().is_some(), "accessToken");
    // The id is opaque (a random hex), the display name is what was set at
    // registration; Immich clients treat both as data, not identity.
    assert_eq!(json["name"].as_str(), Some("wyatt@example.com"));
}

#[tokio::test]
async fn immich_login_rejects_bad_credentials_with_401() {
    let (_dir, mut router, _token) = app_with_user("wyatt@example.com");
    let (status, _) = send(
        &mut router,
        "POST",
        "/api/auth/login",
        None,
        Some(br#"{"email":"wyatt@example.com","password":"wrong"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn validate_token_accepts_a_live_session() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let (status, json) = send(
        &mut router,
        "POST",
        "/api/auth/validateToken",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["authStatus"], serde_json::Value::Bool(true));
}

#[tokio::test]
async fn users_me_returns_the_profile() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let (status, json) = send(&mut router, "GET", "/api/users/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["email"].as_str(), Some("wyatt@example.com"));
    assert_eq!(json["name"].as_str(), Some("wyatt@example.com"));
    // The id is the same opaque subject the login returned.
    let (_, login) = send(
        &mut router,
        "POST",
        "/api/auth/login",
        None,
        Some(br#"{"email":"wyatt@example.com","password":"correct horse battery"}"#.to_vec()),
    )
    .await;
    assert_eq!(json["id"], login["userId"], "stable subject across calls");
}

/// A PNG with known dimensions, for the upload tests.
fn png_bytes() -> Vec<u8> {
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"TSSE");
    png.extend_from_slice(&14u32.to_be_bytes());
    png.resize(png.len() + 13, 0);
    png
}

#[tokio::test]
async fn immich_multipart_upload_stores_the_asset() {
    let (_dir, router, token) = app_with_user("wyatt@example.com");
    let png = png_bytes();
    let boundary = "testboundary123";
    let body = multipart_body(boundary, "shot.png", &png);

    let request = Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(json["duplicate"], serde_json::Value::Bool(false));
    let address = json["id"].as_str().expect("id").to_string();

    // The original is fetchable through the Immich path by its content
    // address, sniffed as PNG.
    let uri = format!("/api/assets/{address}/original");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    assert_eq!(bytes, png, "the original bytes round-trip");
}

/// Build a multipart body with one `assetData` field.
fn multipart_body(boundary: &str, filename: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("--{boundary}\r\n").into_bytes();
    out.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"assetData\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(data);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// A genuinely decodable PNG, for the thumbnail tests — the `png_bytes`
/// fixture above is magic bytes plus junk, which is exactly what a
/// thumbnail endpoint must refuse.
fn real_png(width: u32, height: u32) -> Vec<u8> {
    let img = image::DynamicImage::new_rgb8(width, height);
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encodes");
    png
}

async fn upload_bytes(
    router: &mut axum::Router,
    token: &str,
    filename: &str,
    data: &[u8],
) -> String {
    let boundary = "testboundary123";
    let request = Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart_body(boundary, filename, data)))
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    json["id"].as_str().expect("id").to_string()
}

#[tokio::test]
async fn immich_thumbnail_serves_a_derived_still() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let address = upload_bytes(&mut router, &token, "shot.png", &real_png(1600, 900)).await;

    let uri = format!("/api/assets/{address}/thumbnail?size=preview");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .expect("content type")
        .to_string();
    assert_eq!(content_type, "image/jpeg");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    assert_eq!(&bytes[0..2], b"\xFF\xD8", "a real JPEG");

    // The second request is served from the cache — same bytes, no decode.
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    let again = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    assert_eq!(again, bytes, "cache hit reproduces the exact blob");
}

#[tokio::test]
async fn immich_thumbnail_defaults_to_the_small_size_and_refuses_unknown_ones() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let address = upload_bytes(&mut router, &token, "shot.png", &real_png(800, 600)).await;

    // No ?size= means thumbnail — the default Immich asks for first.
    let uri = format!("/api/assets/{address}/thumbnail");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);

    // Blurhash is an Immich feature this vault honestly does not have yet.
    let uri = format!("/api/assets/{address}/thumbnail?size=blurhash");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn immich_thumbnail_of_a_non_image_is_a_404() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    // The fake `png_bytes` fixture: magic header, junk body — not decodable.
    let address = upload_bytes(&mut router, &token, "fake.png", &png_bytes()).await;

    let uri = format!("/api/assets/{address}/thumbnail");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
