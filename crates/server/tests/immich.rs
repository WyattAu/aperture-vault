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
use chrono::Datelike as _;
use std::io::Read as _;
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
    assert_eq!(json["status"], "created", "Immich's upload status shape");
    let address = json["id"].as_str().expect("id").to_string();

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

#[tokio::test]
async fn server_ping_pongs_without_authentication() {
    let (_dir, mut router, _token) = app_with_user("wyatt@example.com");
    let (status, json) = send(&mut router, "GET", "/api/server/ping", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["res"], "pong");
}

#[tokio::test]
async fn server_features_and_config_state_only_what_is_true() {
    let (_dir, mut router, _token) = app_with_user("wyatt@example.com");
    let (_, features) = send(&mut router, "GET", "/api/server/features", None, None).await;
    assert_eq!(features["passwordLogin"], serde_json::Value::Bool(true));
    assert_eq!(
        features["facialRecognition"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(features["oauth"], serde_json::Value::Bool(false));

    let (_, config) = send(&mut router, "GET", "/api/server/config", None, None).await;
    assert_eq!(config["isInitialized"], serde_json::Value::Bool(true));
    assert_eq!(config["publicUsers"], serde_json::Value::Bool(false));

    let (_, version) = send(&mut router, "GET", "/api/server/version", None, None).await;
    // The vault's own version, not a pretended Immich version.
    assert!(version["major"].is_u64());
    assert!(version["minor"].is_u64());
    assert!(version["patch"].is_u64());
}

#[tokio::test]
async fn bulk_upload_check_answers_duplicate_for_known_checksums() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let png = real_png(320, 200);
    let address = upload_bytes(&mut router, &token, "known.png", &png).await;

    // The SHA-1 the pipeline recorded, computed independently here.
    use sha1::Digest as _;
    let digest: Vec<u8> = {
        let mut h = sha1::Sha1::new();
        h.update(&png);
        h.finalize().to_vec()
    };
    let hex_form: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    use base64::Engine as _;
    let b64_form = base64::engine::general_purpose::STANDARD.encode(&digest);

    let body = serde_json::json!({
        "assets": [
            { "id": "client-1", "checksum": b64_form },
            { "id": "client-2", "checksum": hex_form.to_uppercase() },
            { "id": "client-3", "checksum": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" },
        ]
    })
    .to_string();
    let (status, json) = send(
        &mut router,
        "POST",
        "/api/assets/bulk-upload-check",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let results = json["results"].as_array().expect("results");
    assert_eq!(results.len(), 3);
    // Both encodings of the same checksum resolve to the stored asset.
    assert_eq!(results[0]["action"], "reject");
    assert_eq!(results[0]["reason"], "duplicate");
    assert_eq!(results[0]["assetId"], address);
    assert_eq!(results[1]["action"], "reject");
    assert_eq!(results[1]["reason"], "duplicate");
    // An unknown checksum means "upload it".
    assert_eq!(results[2]["action"], "accept");
}

#[tokio::test]
async fn timeline_buckets_and_bucket_match_the_immich_shapes() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let address = upload_bytes(&mut router, &token, "grid.png", &real_png(600, 400)).await;

    let (status, buckets) = send(
        &mut router,
        "GET",
        "/api/timeline/buckets",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = buckets.as_array().expect("array");
    assert!(!list.is_empty(), "the uploaded asset made a bucket");
    assert!(list[0]["timeBucket"].is_string());
    assert_eq!(list[0]["count"].as_u64(), Some(1));
    let day = list[0]["timeBucket"].as_str().expect("day").to_string();

    // The columnar day fetch: every field is an array, position-aligned.
    let uri = format!("/api/timeline/bucket?timeBucket={day}T00:00:00.000Z");
    let (status, bucket) = send(&mut router, "GET", &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{bucket}");
    let ids = bucket["id"].as_array().expect("id array");
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].as_str(), Some(address.as_str()));
    assert_eq!(bucket["isImage"][0], serde_json::Value::Bool(true));
    assert_eq!(bucket["visibility"][0], "timeline");
    // Before any thumbnail is derived, the placeholder column is honestly
    // empty; after generation it carries the base64 ThumbHash.
    assert_eq!(bucket["thumbhash"][0], "");
    let thumb_uri = format!("/api/assets/{address}/thumbnail");
    let request = Request::builder()
        .uri(&thumb_uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);
    let (status, bucket) = send(&mut router, "GET", &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let hash = bucket["thumbhash"][0].as_str().expect("hash").to_string();
    assert!(!hash.is_empty(), "a placeholder now exists");

    // Position alignment: every parallel array has the same length.
    for (key, value) in bucket.as_object().expect("object") {
        assert_eq!(
            value.as_array().map(|a| a.len()),
            Some(1),
            "column {key} is position-aligned with id"
        );
    }

    // A bucket without the required parameter is a 400, not a panic.
    let (status, _) = send(
        &mut router,
        "GET",
        "/api/timeline/bucket",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn albums_create_add_list_and_delete_with_their_assets_surviving() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let first = upload_bytes(&mut router, &token, "one.png", &real_png(200, 100)).await;
    let second = upload_bytes(&mut router, &token, "two.png", &real_png(100, 200)).await;

    // Create, seeded with one asset; the phantom id names nothing and is skipped.
    let body = serde_json::json!({
        "albumName": "Holiday",
        "description": "selected",
        "assetIds": [first, "phantom"],
    })
    .to_string();
    let (status, album) = send(
        &mut router,
        "POST",
        "/api/albums",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{album}");
    assert_eq!(album["albumName"], "Holiday");
    assert_eq!(album["assetCount"], 1);
    assert_eq!(album["albumThumbnailAssetId"], first);
    assert_eq!(album["shared"], false);
    let album_id = album["id"].as_str().expect("id").to_string();

    // Add the second asset; each id reports its own outcome.
    let body = serde_json::json!({ "ids": [second, "ghost"] }).to_string();
    let (status, results) = send(
        &mut router,
        "PUT",
        &format!("/api/albums/{album_id}/assets"),
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = results.as_array().expect("array");
    assert_eq!(results[0]["id"], second);
    assert_eq!(results[0]["success"], true);
    assert_eq!(results[1]["success"], false, "the ghost names no asset");

    // The listing shows both members and the full shape Immich reads.
    let (_, list) = send(&mut router, "GET", "/api/albums", Some(&token), None).await;
    let list = list.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["assetCount"], 2);
    assert_eq!(list[0]["albumUsers"], serde_json::json!([]));

    // Delete the album; its assets survive.
    let (status, _) = send(
        &mut router,
        "DELETE",
        &format!("/api/albums/{album_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{first}/original"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "assets outlive their album");
    let (status, list) = send(&mut router, "GET", "/api/albums", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list.as_array().expect("array").is_empty(),
        "the album is gone"
    );
}

#[tokio::test]
async fn album_endpoints_require_a_token() {
    let (_dir, mut router, _token) = app_with_user("wyatt@example.com");
    let (status, _) = send(&mut router, "GET", "/api/albums", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn share_links_expose_an_album_publicly_until_revoked() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let asset = upload_bytes(&mut router, &token, "shared.png", &real_png(300, 150)).await;
    let outside = upload_bytes(&mut router, &token, "private.png", &real_png(140, 90)).await;

    // Share the album that holds `asset`.
    let body = serde_json::json!({ "albumName": "Public", "assetIds": [asset] }).to_string();
    let (_, album) = send(
        &mut router,
        "POST",
        "/api/albums",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    let album_id = album["id"].as_str().expect("id").to_string();

    let body = serde_json::json!({ "albumId": album_id }).to_string();
    let (status, link) = send(
        &mut router,
        "POST",
        "/api/shared-links",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    assert_eq!(link["type"], "ALBUM");
    assert_eq!(link["album"]["albumName"], "Public");
    assert!(
        link["key"].as_str().is_some(),
        "the owner gets the key back"
    );
    let share_id = link["id"].as_str().expect("id").to_string();
    let key = link["key"].as_str().expect("key").to_string();

    // The public face: album + member ids, no session.
    let uri = format!("/api/shared-links/me?key={key}");
    let (status, shared) = send(&mut router, "GET", &uri, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["album"]["id"], album_id);
    assert_eq!(shared["assets"][0], asset);

    // A share key reaches members through the asset endpoints…
    let uri = format!("/api/assets/{asset}/thumbnail?key={key}");
    let request = Request::builder()
        .uri(&uri)
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);
    // …and nothing else.
    let uri = format!("/api/assets/{outside}/thumbnail?key={key}");
    let request = Request::builder()
        .uri(&uri)
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // A session token is not a share key, and a share key is not a session:
    // the roles are distinct by construction.
    let uri = format!("/api/assets/{asset}/thumbnail?key={token}");
    let request = Request::builder()
        .uri(&uri)
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The owner lists and revokes.
    let (_, list) = send(&mut router, "GET", "/api/shared-links", Some(&token), None).await;
    assert_eq!(list.as_array().expect("array").len(), 1);
    let (status, _) = send(
        &mut router,
        "DELETE",
        &format!("/api/shared-links/{share_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Revoked: the key no longer opens anything, asset or album.
    let uri = format!("/api/shared-links/me?key={key}");
    let (status, _) = send(&mut router, "GET", &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let uri = format!("/api/assets/{asset}/thumbnail?key={key}");
    let request = Request::builder()
        .uri(&uri)
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn asset_detail_returns_the_full_record_shaped_like_immich() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let png = real_png(420, 240);
    let address = upload_bytes(&mut router, &token, "detail.png", &png).await;

    // Derive the thumbnail so the record has a placeholder too.
    let uri = format!("/api/assets/{address}/thumbnail");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);

    let uri = format!("/api/assets/{address}");
    let (status, detail) = send(&mut router, "GET", &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["id"], address);
    assert_eq!(detail["type"], "IMAGE");
    assert_eq!(detail["width"], 420);
    assert_eq!(detail["height"], 240);
    assert_eq!(detail["isFavorite"], false);
    assert_eq!(detail["visibility"], "timeline");
    assert!(detail["thumbhash"].as_str().is_some_and(|h| !h.is_empty()));
    assert_eq!(detail["originalMimeType"], "image/png");
    // EXIF block carries what the probe found; none here, honestly null.
    assert_eq!(detail["exifInfo"]["make"], serde_json::Value::Null);

    // Unknown id: 404, not a panic.
    let zeros = "0".repeat(64);
    let (status, _) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{zeros}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No credentials: 401.
    let (status, _) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{address}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn memories_surface_assets_from_this_day_in_past_years() {
    let (dir, mut router, token) = app_with_user("wyatt@example.com");
    let asset = upload_bytes(&mut router, &token, "memory.png", &real_png(300, 150)).await;

    // Seed a capture date two years ago today, through the same index file
    // the router's AppState holds open (WAL allows the second connection).
    let today = chrono::Utc::now();
    let past = format!(
        "{:04}-{:02}-{:02}T14:30:00",
        today.year() - 2,
        today.month(),
        today.day()
    );
    rusqlite::Connection::open(dir.path().join("index.db"))
        .expect("opens")
        .execute(
            "UPDATE assets SET taken_at = ?1 WHERE address = ?2",
            rusqlite::params![past, asset],
        )
        .expect("seed");

    let (status, memories) = send(&mut router, "GET", "/api/memories", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{memories}");
    let list = memories.as_array().expect("array");
    assert_eq!(list.len(), 1, "one year group");
    let memory = &list[0];
    assert_eq!(memory["type"], "on_this_day");
    assert_eq!(memory["data"]["yearAgo"], 2);
    assert_eq!(memory["isSaved"], false);
    let assets = memory["assets"].as_array().expect("assets");
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0]["id"], asset);
    assert_eq!(assets[0]["localDateTime"], past);
}

#[tokio::test]
async fn memories_of_untagged_assets_are_empty_not_invented() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    upload_bytes(&mut router, &token, "no-exif.png", &real_png(120, 80)).await;
    let (status, memories) = send(&mut router, "GET", "/api/memories", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        memories.as_array().expect("array").is_empty(),
        "an asset without a capture date is not a memory; a guess would be a lie with a timestamp"
    );
}

#[tokio::test]
async fn favorite_toggles_and_statistics_count() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let a = upload_bytes(&mut router, &token, "a.png", &real_png(100, 100)).await;
    let _b = upload_bytes(&mut router, &token, "b.png", &real_png(110, 90)).await;

    let (status, stats) = send(
        &mut router,
        "GET",
        "/api/assets/statistics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["images"], 2);
    assert_eq!(stats["videos"], 0);
    assert_eq!(stats["total"], 2);

    // Star one asset; the detail and the response agree.
    let body = serde_json::json!({ "isFavorite": true }).to_string();
    let (status, updated) = send(
        &mut router,
        "PUT",
        &format!("/api/assets/{a}"),
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["isFavorite"], true);
    let (_, detail) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{a}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(detail["isFavorite"], true);
    let (_, other) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{_b}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(other["isFavorite"], false, "only the starred asset changed");

    // A body with no supported field is refused, not silently ignored.
    let (status, _) = send(
        &mut router,
        "PUT",
        &format!("/api/assets/{a}"),
        Some(&token),
        Some(br#"{"rating":5}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Statistics on an empty vault would be zeros; here the starred one
    // does not change counts.
    let (_, stats) = send(
        &mut router,
        "GET",
        "/api/assets/statistics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(stats["total"], 2);
}

#[tokio::test]
async fn search_filters_assets_by_name_kind_and_favorite() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let holiday = upload_bytes(&mut router, &token, "holiday-beach.png", &real_png(100, 60)).await;
    let _work = upload_bytes(&mut router, &token, "work-meeting.png", &real_png(110, 70)).await;
    let body = serde_json::json!({ "isFavorite": true }).to_string();
    let _ = send(
        &mut router,
        "PUT",
        &format!("/api/assets/{holiday}"),
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    let _ = send(
        &mut router,
        "POST",
        "/api/albums",
        Some(&token),
        Some(
            serde_json::json!({"albumName": "Holiday shots"})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;

    // Name substring, case-insensitive, matches assets AND albums.
    let body = serde_json::json!({ "originalFileName": "HOLIDAY" }).to_string();
    let (status, found) = send(
        &mut router,
        "POST",
        "/api/search/metadata",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let assets = found["assets"]["items"].as_array().expect("assets");
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0]["id"], holiday);
    let albums = found["albums"]["items"].as_array().expect("albums");
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0]["albumName"], "Holiday shots");

    // Favorite filter narrows to the starred asset.
    let body = serde_json::json!({ "isFavorite": true }).to_string();
    let (_, found) = send(
        &mut router,
        "POST",
        "/api/search/metadata",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(found["assets"]["count"], 1);
    assert_eq!(found["assets"]["items"][0]["id"], holiday);

    // Type filter: IMAGE matches both (both are stills), VIDEO would be empty.
    let body = serde_json::json!({ "type": "VIDEO" }).to_string();
    let (_, found) = send(
        &mut router,
        "POST",
        "/api/search/metadata",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(found["assets"]["count"], 0);

    // No filters: everything.
    let (_, found) = send(
        &mut router,
        "POST",
        "/api/search/metadata",
        Some(&token),
        Some(serde_json::json!({}).to_string().into_bytes()),
    )
    .await;
    assert_eq!(found["assets"]["count"], 2);
}

#[tokio::test]
async fn trash_hides_then_restores_then_destroys() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let a = upload_bytes(&mut router, &token, "doomed.png", &real_png(90, 90)).await;
    let b = upload_bytes(&mut router, &token, "kept.png", &real_png(95, 95)).await;

    // Trash one of two: it leaves the timeline, statistics, and search,
    // and the original endpoint reports gone.
    let body = serde_json::json!({ "ids": [a] }).to_string();
    let (status, result) = send(
        &mut router,
        "DELETE",
        "/api/assets",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["count"], 1);

    let (_, stats) = send(
        &mut router,
        "GET",
        "/api/assets/statistics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(stats["total"], 1, "trash is not owned data");

    let (_, search) = send(
        &mut router,
        "POST",
        "/api/search/metadata",
        Some(&token),
        Some(serde_json::json!({}).to_string().into_bytes()),
    )
    .await;
    assert_eq!(search["assets"]["count"], 1, "only the kept asset remains");

    let uri = format!("/api/assets/{a}/original");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "trashed is gone from the reads"
    );

    // The trash screen sees it; restore brings it back to the reads.
    let (_, trash) = send(&mut router, "GET", "/api/trash", Some(&token), None).await;
    assert_eq!(trash["count"], 1);
    assert_eq!(trash["items"][0]["isTrashed"], true);
    let (status, result) = send(
        &mut router,
        "POST",
        "/api/trash/restore/assets",
        Some(&token),
        Some(serde_json::json!({ "ids": [a] }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["count"], 1);
    let uri = format!("/api/assets/{a}/original");
    let request = Request::builder()
        .uri(&uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "restored is readable again"
    );

    // Trash both, empty: gone everywhere, permanently.
    let body = serde_json::json!({ "ids": [a, b] }).to_string();
    let _ = send(
        &mut router,
        "DELETE",
        "/api/assets",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    let (status, result) = send(&mut router, "POST", "/api/trash/empty", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["count"], 2);
    let (_, stats) = send(
        &mut router,
        "GET",
        "/api/assets/statistics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(stats["total"], 0);
    let (_, trash) = send(&mut router, "GET", "/api/trash", Some(&token), None).await;
    assert_eq!(trash["count"], 0);
}

#[tokio::test]
async fn album_scoped_timeline_serves_the_album_screen() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let inside = upload_bytes(&mut router, &token, "in-album.png", &real_png(120, 80)).await;
    let outside = upload_bytes(&mut router, &token, "not-in.png", &real_png(80, 120)).await;

    let body = serde_json::json!({ "albumName": "Scoped", "assetIds": [inside] }).to_string();
    let (_, album) = send(
        &mut router,
        "POST",
        "/api/albums",
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    let album_id = album["id"].as_str().expect("id").to_string();

    // The album's own bucket strip: one bucket, one asset.
    let uri = format!("/api/timeline/buckets?albumId={album_id}");
    let (status, buckets) = send(&mut router, "GET", &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let list = buckets.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["count"], 1);
    let day = list[0]["timeBucket"].as_str().expect("day").to_string();

    // The bucket fetch narrowed by albumId: the member in, the outsider out.
    let uri = format!("/api/timeline/bucket?albumId={album_id}&timeBucket={day}T00:00:00.000Z");
    let (status, bucket) = send(&mut router, "GET", &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let ids = bucket["id"].as_array().expect("ids");
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].as_str(), Some(inside.as_str()));
    let rendered = serde_json::to_string(&bucket).expect("serializes");
    assert!(
        !rendered.contains(&outside),
        "the outsider is not in the album's bucket"
    );

    // An unknown album is a 404, not an empty bucket.
    let (status, _) = send(
        &mut router,
        "GET",
        "/api/timeline/buckets?albumId=nope",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn users_listing_shows_the_directory_without_secrets() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let (status, list) = send(&mut router, "GET", "/api/users", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["email"], "wyatt@example.com");
    assert_eq!(items[0]["name"], "wyatt@example.com");
    let raw = serde_json::to_string(&list).expect("serializes");
    assert!(
        !raw.contains("argon2"),
        "password verifiers never leave the store"
    );
}

#[tokio::test]
async fn map_markers_serve_coordinates_and_refuse_unsupported_updates() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let address = upload_bytes(&mut router, &token, "geo.png", &real_png(90, 90)).await;

    // Synthetic fixtures carry no EXIF, so the marker set is honestly empty.
    let (status, markers) = send(&mut router, "GET", "/api/map/markers", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(markers.as_array().expect("array").is_empty());

    // Description updates work and surface in the detail's EXIF block.
    let body = serde_json::json!({ "description": "sunset over the harbor" }).to_string();
    let (status, updated) = send(
        &mut router,
        "PUT",
        &format!("/api/assets/{address}"),
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let (_, detail) = send(
        &mut router,
        "GET",
        &format!("/api/assets/{address}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(detail["exifInfo"]["description"], "sunset over the harbor");

    // Both supported fields in one call is fine.
    let body = serde_json::json!({ "isFavorite": true, "description": "starred" }).to_string();
    let (status, updated) = send(
        &mut router,
        "PUT",
        &format!("/api/assets/{address}"),
        Some(&token),
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["isFavorite"], true);
    assert_eq!(updated["exifInfo"]["description"], "starred");

    // Unsupported fields are refused by name, never ignored.
    let body = serde_json::json!({ "rating": 5 }).to_string();
    // Errors are plain text, so the body is read raw, not through the JSON
    // helper.
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/assets/{address}"))
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let text = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    let message = String::from_utf8(text).expect("utf8");
    assert!(
        message.contains("rating"),
        "the refusal names the field: {message}"
    );

    // The feature flag now says the map exists.
    let (_, features) = send(&mut router, "GET", "/api/server/features", None, None).await;
    assert_eq!(features["map"], serde_json::Value::Bool(true));
}

#[tokio::test]
async fn download_archive_delivers_a_real_zip_of_originals() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    let first = upload_bytes(&mut router, &token, "first-shot.png", &real_png(80, 80)).await;
    let second = upload_bytes(&mut router, &token, "second-shot.png", &real_png(60, 60)).await;

    let body = serde_json::json!({
        "assetIds": [first, second],
        "archiveName": "my-pictures",
    })
    .to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/api/download/archive")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("Content-Disposition")
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"my-pictures.zip\"")
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();

    // A real unzipper reads it, and both originals are inside under their
    // display names with byte-exact content.
    let mut reader = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("the archive opens");
    assert_eq!(reader.len(), 2);
    let mut got = Vec::new();
    reader
        .by_name("first-shot.png")
        .expect("entry")
        .read_to_end(&mut got)
        .expect("reads");
    assert_eq!(got, real_png(80, 80), "byte-exact original");

    // Duplicate display names do not overwrite each other.
    let twin = upload_bytes(&mut router, &token, "first-shot.png", &real_png(70, 70)).await;
    let body = serde_json::json!({ "assetIds": [first, twin] }).to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/api/download/archive")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .expect("builds");
    let response = router.clone().oneshot(request).await.expect("responds");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    let mut reader = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("opens");
    assert_eq!(reader.len(), 2, "both members survive the name collision");
    assert!(reader.by_name("first-shot.png").is_ok());
    assert!(reader.by_name("first-shot (1).png").is_ok());

    // An empty selection is refused.
    let (status, _) = send(
        &mut router,
        "POST",
        "/api/download/archive",
        Some(&token),
        Some(
            serde_json::json!({ "assetIds": [] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_media_types_and_statistics_state_the_truth() {
    let (_dir, mut router, token) = app_with_user("wyatt@example.com");
    upload_bytes(&mut router, &token, "still.png", &real_png(100, 100)).await;

    let (status, media) = send(
        &mut router,
        "GET",
        "/api/server/media-types",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(media["image"], serde_json::json!(["jpg", "jpeg", "png"]));
    assert_eq!(media["video"], serde_json::json!(["mp4"]));
    assert_eq!(media["sidecar"], serde_json::json!([]));

    let (_, stats) = send(
        &mut router,
        "GET",
        "/api/server/statistics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(stats["photos"], 1);
    assert_eq!(stats["videos"], 0);
    assert!(
        stats["usage"].as_u64().is_some_and(|u| u > 0),
        "bytes were uploaded"
    );
    assert_eq!(stats["usageByUser"], serde_json::json!([]));
}
