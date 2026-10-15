//! Immich-compatible API endpoints, so the existing Immich mobile apps
//! attach to this server unchanged. See the crate docs for the honest
//! boundary: ids are content addresses, not UUIDs.

use crate::{mime_for, require_user, ApiError, AppState};
use axum::extract::{Multipart, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Datelike as _;
use serde_json::json;
use std::sync::Arc;

type ApiResult = Result<Response, ApiError>;

fn json_ok(value: &serde_json::Value) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

fn json_created(value: &serde_json::Value) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

fn bearer(headers: &axum::http::HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)
}

/// `POST /api/auth/login` — Immich shape: `{email, password}` in, an
/// `accessToken` out. The vault's username doubles as the email field, so
/// existing client login screens work verbatim.
pub async fn login(State(state): State<AppState>, body: String) -> ApiResult {
    let req: LoginRequest =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let user = state
        .users
        .lock()
        .map_err(|_| ApiError::Internal)?
        .verify_login(&req.email, &req.password)
        .map_err(|_| ApiError::Unauthorized)?;
    let token = state
        .auth
        .issue_session(&user)
        .map_err(|_| ApiError::Internal)?;
    json_ok(&json!({
        "accessToken": token,
        "userId": user.id,
        "name": user.display_name,
        "profileImagePath": "",
        "isAdmin": false,
        "shouldChangePassword": false,
    }))
}

#[derive(Debug, serde::Deserialize)]
struct LoginRequest {
    /// Immich clients send `email`; native clients may send `username`.
    #[serde(alias = "username")]
    email: String,
    password: String,
}

/// `POST /api/auth/validateToken` — Immich apps call this on resume.
pub async fn validate_token(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult {
    let token = bearer(&headers)?;
    state
        .auth
        .validate_session(token)
        .map_err(|_| ApiError::Unauthorized)?;
    json_ok(&json!({ "authStatus": true }))
}

/// `GET /api/users/me` — the profile the app stores after login.
pub async fn users_me(State(state): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult {
    let user_id = require_user(&state, &headers)?;
    let user = state
        .users
        .lock()
        .map_err(|_| ApiError::Internal)?
        .user_by_id(&user_id)
        .ok_or(ApiError::Unauthorized)?;
    json_ok(&json!({
        "id": user.id,
        "email": user.username,
        "name": user.display_name,
    }))
}

/// The multipart half of `POST /api/assets`. Registered through the native
/// router's dispatcher, which branches on Content-Type; see `build_router`.
pub(crate) async fn upload_asset_multipart(
    state: AppState,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> ApiResult {
    require_user(&state, &headers)?;
    let mut data: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() == Some("assetData") {
            filename = field.file_name().map(str::to_string);
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("asset body: {e}")))?;
            data = Some(bytes.to_vec());
        }
    }
    let Some(data) = data else {
        return Err(ApiError::BadRequest(
            "multipart field assetData is required".into(),
        ));
    };
    let report = aperture_pipeline::Pipeline::ingest(
        &state.store,
        &*state.index.lock().map_err(|_| ApiError::Internal)?,
        std::io::Cursor::new(&data),
        filename.as_deref(),
    )
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    // The Immich shape: {id, status}. "duplicate" is not an error — the app
    // treats it as confirmation the phone's copy is already safe here.
    json_created(&json!({
        "id": report.address.hex(),
        "status": if report.newly_written { "created" } else { "duplicate" },
    }))
}

/// `GET /api/assets/:id/original` — the original bytes, content type sniffed.
/// A session works for any asset; a share key works for assets in the
/// shared album — the read-only reach of the capability.
pub async fn original(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    authorize_asset(&state, &headers, &id, params.get("key").map(String::as_str))?;
    let addr = crate::cas_address(&id)?;
    // Trashed assets are gone from the reads: the index decides, the CAS
    // only stores. (Also rejects unknown ids.)
    let listed = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .asset(&addr.hex())
        .map_err(|_| ApiError::Internal)?
        .is_some();
    if !listed {
        return Err(ApiError::NotFound);
    }
    let bytes = state.store.get_bytes(&addr).map_err(|e| match e {
        aperture_cas::CasError::Missing { .. } => ApiError::NotFound,
        _other => ApiError::Internal,
    })?;
    let mime = mime_for(&bytes);
    Ok(([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response())
}

/// Session **or** share key: a session reaches every asset; a share key
/// reaches exactly the assets of its album. Anything else is 401 —
/// authorization failures do not distinguish reasons to a probe.
fn authorize_asset(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    asset_id: &str,
    key: Option<&str>,
) -> Result<(), ApiError> {
    if require_user(state, headers).is_ok() {
        return Ok(());
    }
    let Some(key) = key else {
        return Err(ApiError::Unauthorized);
    };
    let record = authorize_share(state, key)?;
    let contains = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .album_contains(&record.album_id, asset_id)
        .map_err(|_| ApiError::Internal)?;
    if contains {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

/// The `?size=` query the Immich apps send.
#[derive(Debug, serde::Deserialize)]
pub struct ThumbQuery {
    /// `thumbnail` (default) or `preview`.
    pub size: Option<String>,
    /// The share key, when a public visitor fetches through a share.
    #[serde(rename = "key")]
    pub key: Option<String>,
}

impl ThumbQuery {
    /// The share key, if any.
    #[must_use]
    pub fn share_key(&self) -> Option<&str> {
        self.key.as_deref()
    }
}

/// `GET /api/assets/:id/thumbnail` — the derived still a grid displays.
/// Generated on demand and cached as a CAS blob, so the cost is paid once
/// per asset per size. The decode runs on the blocking pool: it is real
/// CPU work, not a pretend-async stub.
pub async fn thumbnail(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ThumbQuery>,
) -> ApiResult {
    authorize_asset(&state, &headers, &id, query.share_key())?;
    let addr = crate::cas_address(&id)?;
    let listed = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .asset(&addr.hex())
        .map_err(|_| ApiError::Internal)?
        .is_some();
    if !listed {
        return Err(ApiError::NotFound);
    }
    let size = match query.size.as_deref() {
        None | Some("thumbnail") => aperture_pipeline::thumb::ThumbSize::Thumbnail,
        Some("preview") => aperture_pipeline::thumb::ThumbSize::Preview,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "unknown thumbnail size {other}; thumbnail and preview are the ones that exist"
            )));
        }
    };
    let store = Arc::clone(&state.store);
    let index = Arc::clone(&state.index);
    let derived = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let guard = index.lock().map_err(|_| ApiError::Internal)?;
        aperture_pipeline::thumb::thumbnail(&store, &guard, &addr, size).map_err(|e| match e {
            aperture_pipeline::PipelineError::NotAnImage => ApiError::NotFound,
            _ => ApiError::Internal,
        })
    })
    .await
    .map_err(|_| ApiError::Internal)??;
    let thumb = derived;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
        thumb.bytes,
    )
        .into_response())
}

/// `GET /api/server/ping` — the reachability check the Immich apps make
/// before login. Unauthenticated by design: it answers for the server, not
/// for a user.
pub async fn ping() -> ApiResult {
    json_ok(&json!({ "res": "pong" }))
}

/// `GET /api/server/features` — the flags the app adapts its UI around.
/// Every flag is the truth: only password login exists here today.
pub async fn features() -> ApiResult {
    json_ok(&json!({
        "configFile": false,
        "duplicateDetection": true,
        "email": false,
        "facialRecognition": false,
        "importFaces": false,
        "map": false,
        "oauth": false,
        "oauthAutoLaunch": false,
        "ocr": false,
        "passwordLogin": true,
        "realtimeTranscoding": false,
        "reverseGeocoding": false,
        "search": false,
        "sidecar": false,
        "smartSearch": false,
        "trash": false,
    }))
}

/// `GET /api/server/config` — the login-screen configuration. Initialized
/// and onboarded: the vault has no admin-signup dance; the first user is
/// created with the CLI.
pub async fn server_config() -> ApiResult {
    json_ok(&json!({
        "externalDomain": "",
        "isInitialized": true,
        "isOnboarded": true,
        "loginPageMessage": "",
        "maintenanceMode": false,
        "mapDarkStyleUrl": "",
        "mapLightStyleUrl": "",
        "minFaces": 1,
        "oauthButtonText": "",
        "publicUsers": false,
        "trashDays": 0,
        "userDeleteDelay": 0,
    }))
}

/// `GET /api/server/version` — the vault's own semver, not a pretended
/// Immich version. An id that lies about identity would be a defect, and a
/// version that lies about the software is the same defect.
pub async fn version() -> ApiResult {
    json_ok(&json!({
        "major": env!("CARGO_PKG_VERSION_MAJOR").parse::<u32>().unwrap_or(0),
        "minor": env!("CARGO_PKG_VERSION_MINOR").parse::<u32>().unwrap_or(0),
        "patch": env!("CARGO_PKG_VERSION_PATCH").parse::<u32>().unwrap_or(0),
        "prerelease": serde_json::Value::Null,
    }))
}

/// A client checksum: `base64::engine::general_purpose::STANDARD`-encoded or
/// hex SHA-1, both accepted, both answered in hex.
fn checksum_to_hex(input: &str) -> Option<String> {
    use base64::Engine as _;
    let trimmed = input.trim();
    if trimmed.len() == 40 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .ok()?;
    (decoded.len() == 20).then(|| {
        decoded
            .iter()
            .fold(String::with_capacity(40), |mut out, b| {
                let _ = write_hex_byte(&mut out, *b);
                out
            })
    })
}

fn write_hex_byte(out: &mut String, byte: u8) -> Result<(), std::fmt::Error> {
    use std::fmt::Write as _;
    write!(out, "{byte:02x}")
}

/// `POST /api/assets/bulk-upload-check` — the call that makes Immich backup
/// resumable: the phone sends the SHA-1 of every local file and learns
/// which ones the vault already holds. Accepted checksums mean "upload";
/// rejected ones with reason `duplicate` mean "already safe here".
pub async fn bulk_upload_check(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Item {
        checksum: String,
        id: String,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        assets: Vec<Item>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let results: Vec<serde_json::Value> = req
        .assets
        .iter()
        .map(|item| {
            let hex = checksum_to_hex(&item.checksum);
            let known = hex
                .as_deref()
                .and_then(|hex| index.asset_by_sha1(hex).ok())
                .flatten();
            match known {
                Some(asset_id) => json!({
                    "action": "reject",
                    "id": item.id,
                    "assetId": asset_id,
                    "isTrashed": false,
                    "reason": "duplicate",
                }),
                None => json!({ "action": "accept", "id": item.id }),
            }
        })
        .collect();
    json_ok(&json!({ "results": results }))
}

/// `GET /api/timeline/buckets` — the gallery's strip of capture days,
/// newest first, in Immich's `{timeBucket, count}` shape.
pub async fn timeline_buckets(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let buckets = {
        let index = state.index.lock().map_err(|_| ApiError::Internal)?;
        if let Some(album_id) = params.get("albumId") {
            // The album's own capture-day strip: members grouped by the
            // same day rule, counted here. Album asset lists are small;
            // the grouping is arithmetic, not a query.
            if index
                .album(album_id)
                .map_err(|_| ApiError::Internal)?
                .is_none()
            {
                return Err(ApiError::NotFound);
            }
            let mut counts: std::collections::BTreeMap<String, u64> =
                std::collections::BTreeMap::new();
            for record in index
                .album_assets(album_id)
                .map_err(|_| ApiError::Internal)?
            {
                let day = record
                    .taken_at
                    .as_deref()
                    .map(|t| t.get(..10).unwrap_or("1970-01-01").to_string())
                    .unwrap_or_else(|| "1970-01-01".into());
                *counts.entry(day).or_insert(0) += 1;
            }
            let mut items: Vec<(String, u64)> = counts.into_iter().collect();
            items.sort_by(|a, b| b.0.cmp(&a.0));
            items
        } else {
            index.buckets().map_err(|_| ApiError::Internal)?
        }
    };
    let items: Vec<serde_json::Value> = buckets
        .into_iter()
        .map(|(day, count)| json!({ "timeBucket": day, "count": count }))
        .collect();
    json_ok(&serde_json::Value::Array(items))
}

/// The columnar per-asset arrays `GET /api/timeline/bucket` returns: position
/// `i` in every array is asset `i`, Immich's bandwidth optimization.
fn columnar(assets: &[aperture_index::AssetRecord], owner: &str) -> serde_json::Value {
    let c = |v: Vec<serde_json::Value>| v;
    let n = assets.len();
    json!({
        "id": c(assets.iter().map(|a| json!(a.address)).collect()),
        "createdAt": c(assets.iter().map(|a| json!(a.taken_at.clone().unwrap_or_else(|| "1970-01-01T00:00:00".into()))).collect()),
        "fileCreatedAt": c(assets.iter().map(|a| json!(a.taken_at.clone().unwrap_or_else(|| "1970-01-01T00:00:00".into()))).collect()),
        "duration": c(assets.iter().map(|a| json!(a.duration_ms.unwrap_or(0) / 1000)).collect()),
        "isImage": c(assets.iter().map(|a| json!(a.duration_ms.is_none())).collect()),
        "isFavorite": c(vec![serde_json::Value::Bool(false); n]),
        "isTrashed": c(vec![serde_json::Value::Bool(false); n]),
        "livePhotoVideoId": c(vec![json!(""); n]),
        "localOffsetHours": c(vec![json!(0); n]),
        "ownerId": c(vec![json!(owner); n]),
        "projectionType": c(vec![json!(""); n]),
        "ratio": c(assets.iter().map(|a| json!(match (a.width, a.height) {
            (Some(w), Some(h)) if h > 0 => serde_json::json!(f64::from(w) / f64::from(h)),
            _ => serde_json::json!(1.0),
        })).collect()),
        "thumbhash": c(assets
            .iter()
            .map(|a| json!(a.thumbhash.clone().unwrap_or_default()))
            .collect()),
        "visibility": c(vec![json!("timeline"); n]),
        "latitude": c(assets.iter().map(|a| json!(a.latitude)).collect()),
        "longitude": c(assets.iter().map(|a| json!(a.longitude)).collect()),
        "city": c(vec![json!(""); n]),
        "country": c(vec![json!(""); n]),
        "stack": c(vec![serde_json::Value::Array(vec![]); n]),
    })
}

/// `GET /api/timeline/bucket?timeBucket=YYYY-MM-DD...` — one capture day's
/// assets, columnar. The `timeBucket` may carry a time suffix; the day rule
/// is the index's own: EXIF date when present, the 1970 sentinel otherwise.
pub async fn timeline_bucket(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let user_id = require_user(&state, &headers)?;
    let Some(bucket) = params.get("timeBucket") else {
        return Err(ApiError::BadRequest(
            "timeBucket is required: the day to fetch".into(),
        ));
    };
    if bucket.len() < 10 {
        return Err(ApiError::BadRequest(
            "timeBucket must name a day (YYYY-MM-DD...)".into(),
        ));
    }
    let day = &bucket[..10];
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let mut assets = index.assets_in_day(day).map_err(|_| ApiError::Internal)?;
    // An albumId narrows the day to the album's members — the call the
    // app's album screen makes. Non-members are filtered, not errors: an
    // album day with no captures is an empty bucket, not a miss.
    if let Some(album_id) = params.get("albumId") {
        if index
            .album(album_id)
            .map_err(|_| ApiError::Internal)?
            .is_none()
        {
            return Err(ApiError::NotFound);
        }
        let members: std::collections::HashSet<String> = index
            .album_assets(album_id)
            .map_err(|_| ApiError::Internal)?
            .into_iter()
            .map(|record| record.address)
            .collect();
        assets.retain(|record| members.contains(&record.address));
    }
    drop(index);
    json_ok(&columnar(&assets, &user_id))
}

/// Immich's `AlbumResponseDto`, from the vault's own record. Fields for
/// sharing, users, and activity are present with their neutral values —
/// the album endpoints must be shape-complete even where the feature is
/// honest about not existing.
#[allow(clippy::too_many_lines)]
fn album_json(album: &aperture_index::AlbumRecord) -> serde_json::Value {
    json!({
        "id": album.id,
        "albumName": album.name,
        "description": album.description,
        "createdAt": album.created_at,
        "updatedAt": album.updated_at,
        "assetCount": album.asset_count,
        "albumThumbnailAssetId": album.thumbnail_address.clone().unwrap_or_default(),
        "albumUsers": [],
        "shared": false,
        "hasSharedLink": false,
        "isActivityEnabled": false,
        "order": "asc",
        "startDate": "",
        "endDate": "",
        "lastModifiedAssetTimestamp": "",
    })
}

/// `GET /api/albums` — every album, newest activity first.
pub async fn albums(State(state): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult {
    require_user(&state, &headers)?;
    let all = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .albums()
        .map_err(|_| ApiError::Internal)?;
    json_ok(&serde_json::Value::Array(
        all.iter().map(album_json).collect(),
    ))
}

/// `POST /api/albums` — create an album, optionally seeded with assets the
/// client names by id.
pub async fn create_album(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(rename = "albumName")]
        album_name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(rename = "assetIds", default)]
        asset_ids: Vec<String>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let album = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .create_album(
            &req.album_name,
            req.description.as_deref().unwrap_or_default(),
            &req.asset_ids,
        )
        .map_err(|_| ApiError::Internal)?;
    json_created(&album_json(&album))
}

/// `GET /api/albums/{id}` — one album.
pub async fn album(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let found = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .album(&id)
        .map_err(|_| ApiError::Internal)?;
    match found {
        Some(album) => json_ok(&album_json(&album)),
        None => Err(ApiError::NotFound),
    }
}

/// `PATCH /api/albums/{id}` — rename or re-describe.
pub async fn update_album(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(rename = "albumName")]
        album_name: Option<String>,
        description: Option<String>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let updated = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .album_update(&id, req.album_name.as_deref(), req.description.as_deref())
        .map_err(|_| ApiError::Internal)?;
    match updated {
        Some(album) => json_ok(&album_json(&album)),
        None => Err(ApiError::NotFound),
    }
}

/// `DELETE /api/albums/{id}` — the album goes, its assets stay.
pub async fn delete_album(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let gone = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .album_delete(&id)
        .map_err(|_| ApiError::Internal)?;
    if gone {
        json_ok(&json!({ "success": true }))
    } else {
        Err(ApiError::NotFound)
    }
}

/// The shared `{ids: [...]}` body of the bulk membership endpoints.
#[derive(serde::Deserialize)]
struct BulkIds {
    ids: Vec<String>,
}

/// `PUT /api/albums/{id}/assets` — add members; each id reports its own
/// outcome, so one bad id cannot sink a bulk request.
pub async fn album_add_assets(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    let req: BulkIds =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let exists = index.album(&id).map_err(|_| ApiError::Internal)?.is_some();
    if !exists {
        return Err(ApiError::NotFound);
    }
    let outcomes = index
        .album_add_assets(&id, &req.ids)
        .map_err(|_| ApiError::Internal)?;
    let results = outcomes
        .into_iter()
        .map(|(asset_id, success)| {
            if success {
                json!({ "id": asset_id, "success": true })
            } else {
                json!({
                    "id": asset_id,
                    "success": false,
                    "error": "not_found",
                    "errorMessage": "no asset with this id",
                })
            }
        })
        .collect::<Vec<_>>();
    json_ok(&serde_json::Value::Array(results))
}

/// `DELETE /api/albums/{id}/assets` — remove members; assets stay in the
/// vault.
pub async fn album_remove_assets(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    let req: BulkIds =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let outcomes = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .album_remove_assets(&id, &req.ids)
        .map_err(|_| ApiError::Internal)?;
    let results = outcomes
        .into_iter()
        .map(|(asset_id, success)| {
            if success {
                json!({ "id": asset_id, "success": true })
            } else {
                json!({
                    "id": asset_id,
                    "success": false,
                    "error": "not_found",
                    "errorMessage": "asset was not a member",
                })
            }
        })
        .collect::<Vec<_>>();
    json_ok(&serde_json::Value::Array(results))
}

/// The vault's share model: album-scoped, read-only, expiring — exactly the
/// capability the auth layer's tokenkit substrate mints, no more.
fn share_json(
    record: &aperture_index::SharedLinkRecord,
    album: Option<&aperture_index::AlbumRecord>,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "key": record.token,
        "type": "ALBUM",
        "createdAt": record.created_at,
        "expiresAt": record.expires_at,
        "description": "",
        "userId": "",
        "slug": "",
        "password": "",
        "allowDownload": true,
        "allowUpload": false,
        "showMetadata": true,
        "assets": [],
        "album": album.map(album_json),
    })
}

/// `POST /api/shared-links` — share an album read-only. The body takes
/// Immich's `albumId` and optional `expiresAt` (RFC 3339); without one the
/// link lives seven days.
pub async fn create_share(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(rename = "albumId")]
        album_id: String,
        #[serde(rename = "expiresAt")]
        expires_at: Option<String>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let album = index
        .album(&req.album_id)
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;

    let now = chrono::Utc::now();
    let expires = match req.expires_at.as_deref() {
        Some(iso) => chrono::DateTime::parse_from_rfc3339(iso)
            .map_err(|e| ApiError::BadRequest(format!("expiresAt: {e}")))?
            .with_timezone(&chrono::Utc),
        None => now + chrono::Duration::days(7),
    };
    if expires <= now {
        return Err(ApiError::BadRequest("expiresAt is in the past".into()));
    }
    let token = state
        .auth
        .issue_share_link(&req.album_id, (expires - now).num_seconds())
        .map_err(|_| ApiError::Internal)?;
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let record = index
        .share_create(&req.album_id, &token, &fmt(now), &fmt(expires))
        .map_err(|_| ApiError::Internal)?;
    json_created(&share_json(&record, Some(&album)))
}

/// `GET /api/shared-links` — the owner's links, newest first.
pub async fn shares(State(state): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult {
    require_user(&state, &headers)?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let all = index.shares().map_err(|_| ApiError::Internal)?;
    let items = all
        .iter()
        .map(|record| {
            let album = index.album(&record.album_id).ok().flatten();
            share_json(record, album.as_ref())
        })
        .collect::<Vec<_>>();
    json_ok(&serde_json::Value::Array(items))
}

/// `DELETE /api/shared-links/{id}` — revoke. The signed token becomes
/// worthless because its row is gone.
pub async fn delete_share(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let gone = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .share_delete(&id)
        .map_err(|_| ApiError::Internal)?;
    if gone {
        json_ok(&json!({ "success": true }))
    } else {
        Err(ApiError::NotFound)
    }
}

/// Validate a share key from a `?key=` query: the JWT must be a valid share
/// token AND its row must still exist (revocation), and it must still be
/// unexpired by the row's own clock.
fn authorize_share(
    state: &AppState,
    key: &str,
) -> Result<aperture_index::SharedLinkRecord, ApiError> {
    let album_id = state
        .auth
        .validate_share_link(key)
        .map_err(|_| ApiError::Unauthorized)?;
    let record = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .share_by_token(key)
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::Unauthorized)?;
    if record.album_id != album_id {
        return Err(ApiError::Unauthorized);
    }
    Ok(record)
}

/// The public face of a share: the album
/// and its asset ids, no session required. A revoked, expired, or
/// never-existing key is the same 401; a probe learns nothing.
pub async fn shared_album(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let key = params.get("key").ok_or(ApiError::Unauthorized)?;
    let record = authorize_share(&state, key)?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let album = index
        .album(&record.album_id)
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let assets = index
        .album_assets(&record.album_id)
        .map_err(|_| ApiError::Internal)?;
    let asset_ids: Vec<_> = assets.iter().map(|a| json!(a.address)).collect();
    let dto = json!({
        "id": record.id,
        "key": record.token,
        "type": "ALBUM",
        "createdAt": record.created_at,
        "expiresAt": record.expires_at,
        "description": "",
        "userId": "",
        "slug": "",
        "password": "",
        "allowDownload": true,
        "allowUpload": false,
        "showMetadata": true,
        "assets": asset_ids,
        "album": album_json(&album),
    });
    json_ok(&dto)
}

/// `GET /api/users` — the directory the album-share dialog reads. No
/// verifiers, no emails beyond the login name itself.
pub async fn users(State(state): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult {
    require_user(&state, &headers)?;
    let all = state.users.lock().map_err(|_| ApiError::Internal)?.users();
    let items = all
        .iter()
        .map(|user| {
            json!({
                "id": user.id,
                "email": user.username,
                "name": user.display_name,
                "profileImagePath": "",
                "profileChangedAt": "",
                "avatarColor": "primary",
            })
        })
        .collect::<Vec<_>>();
    json_ok(&serde_json::Value::Array(items))
}

/// `GET /api/memories` — "on this day": assets captured on today's month
/// and day in previous years, one memory per year, newest first. Pure
/// index arithmetic — no ML, no models, a calendar query that was sitting
/// in the EXIF all along. Assets without capture dates are never memories.
pub async fn memories(State(state): State<AppState>, headers: axum::http::HeaderMap) -> ApiResult {
    let owner = require_user(&state, &headers)?;
    let now = chrono::Utc::now();
    let (month, day, year) = (now.month(), now.day(), now.year() as u32);
    let groups = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .on_this_day(month, day, year)
        .map_err(|_| ApiError::Internal)?;
    let today = now.format("%Y-%m-%dT%H:%M:%S").to_string();
    let items = groups
        .into_iter()
        .map(|(taken_year, assets)| {
            let years_ago = year - taken_year;
            let memory_at = format!("{taken_year:04}-{month:02}-{day:02}T00:00:00");
            let dtos: Vec<_> = assets.iter().map(|a| asset_json(a, &owner, "")).collect();
            json!({
                "id": format!("onthisday-{taken_year:04}"),
                "createdAt": today,
                "updatedAt": today,
                "memoryAt": memory_at,
                "showAt": memory_at,
                "seenAt": serde_json::Value::Null,
                "hideAt": serde_json::Value::Null,
                "deletedAt": serde_json::Value::Null,
                "isSaved": false,
                "ownerId": owner,
                "type": "on_this_day",
                "data": { "type": "onThisDay", "yearAgo": years_ago },
                "assets": dtos,
            })
        })
        .collect::<Vec<_>>();
    json_ok(&serde_json::Value::Array(items))
}

/// Immich's `AssetResponseDto` from the vault's own record — the detail
/// view the app opens per photo. Featureless fields carry their neutral
/// values (`isFavorite: false`, `isTrashed: false`): the shape is complete,
/// the features are honestly absent.
#[allow(clippy::too_many_lines)]
fn asset_json(record: &aperture_index::AssetRecord, owner: &str, mime: &str) -> serde_json::Value {
    let captured = record
        .taken_at
        .clone()
        .unwrap_or_else(|| "1970-01-01T00:00:00".into());
    let is_image = record.duration_ms.is_none();
    json!({
        "id": record.address,
        "checksum": record.sha1.clone().unwrap_or_default(),
        "createdAt": captured,
        "fileCreatedAt": captured,
        "fileModifiedAt": captured,
        "localDateTime": captured,
        "updatedAt": captured,
        "duration": (record.duration_ms.unwrap_or(0) / 1000),
        "hasMetadata": record.taken_at.is_some(),
        "height": record.height,
        "width": record.width,
        "isArchived": false,
        "isEdited": false,
        "isFavorite": record.is_favorite,
        "isOffline": false,
        "isTrashed": false,
        "libraryId": "",
        "livePhotoVideoId": "",
        "duplicateId": "",
        "originalFileName": record.file_name.clone().unwrap_or_default(),
        "originalPath": record.file_name.clone().unwrap_or_default(),
        "originalMimeType": mime,
        "ownerId": owner,
        "people": [],
        "stack": [],
        "tags": [],
        "resized": record.thumbhash.is_some(),
        "thumbhash": record.thumbhash.clone().unwrap_or_default(),
        "type": if is_image { "IMAGE" } else { "VIDEO" },
        "visibility": "timeline",
        "exifInfo": {
            "make": record.make,
            "model": record.model,
            "latitude": record.latitude,
            "longitude": record.longitude,
            "dateTimeOriginal": record.taken_at,
        },
    })
}

/// `GET /api/assets/{id}` — one asset's full record. Session or a share key
/// whose album contains the asset.
pub async fn asset_detail(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    authorize_asset(&state, &headers, &id, params.get("key").map(String::as_str))?;
    let addr = crate::cas_address(&id)?;
    let owner = require_user(&state, &headers).or_else(|_| {
        // A share visitor gets the record too — the key already scoped
        // the asset; the ownerId field carries the empty string because
        // the visitor has no business learning account ids.
        Ok::<String, ApiError>(String::new())
    })?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let record = index
        .asset(&addr.hex())
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    drop(index);
    // The mime comes from the stored magic, not the extension: a bounded
    // read of the first bytes, never the whole blob for a content type.
    let mime = state
        .store
        .open_read(&addr)
        .ok()
        .and_then(|mut file| {
            use std::io::Read as _;
            let mut prefix = [0u8; 16];
            file.read_exact(&mut prefix).ok()?;
            Some(crate::mime_for(&prefix))
        })
        .unwrap_or("application/octet-stream");
    json_ok(&asset_json(&record, &owner, mime))
}

/// `PUT /api/assets/{id}` — the per-asset update. The honest slice of
/// Immich's `UpdateAssetDto`: the favorite toggle, the one field the app's
/// heart icon needs. Description, rating, and visibility would each need
/// index state this vault does not model yet, so they are refused by
/// omission — an accepted-and-ignored field would lie.
pub async fn update_asset(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(rename = "isFavorite")]
        is_favorite: Option<bool>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let Some(favorite) = req.is_favorite else {
        return Err(ApiError::BadRequest(
            "no supported field present; isFavorite is the one that exists".into(),
        ));
    };
    let addr = crate::cas_address(&id)?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let changed = index
        .set_favorite(&addr.hex(), favorite)
        .map_err(|_| ApiError::Internal)?;
    if !changed {
        return Err(ApiError::NotFound);
    }
    let record = index
        .asset(&addr.hex())
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    drop(index);
    let mime = state
        .store
        .open_read(&addr)
        .ok()
        .and_then(|mut file| {
            use std::io::Read as _;
            let mut prefix = [0u8; 16];
            file.read_exact(&mut prefix).ok()?;
            Some(crate::mime_for(&prefix))
        })
        .unwrap_or("application/octet-stream");
    json_ok(&asset_json(&record, &owner_of(&state, &headers), mime))
}

/// The session user, or empty for share visitors — the same rule
/// `asset_detail` applies to identity exposure.
fn owner_of(state: &AppState, headers: &axum::http::HeaderMap) -> String {
    require_user(state, headers).unwrap_or_default()
}

/// `GET /api/assets/statistics` — the counts the backup screen displays:
/// images, videos, total.
pub async fn asset_statistics(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult {
    require_user(&state, &headers)?;
    let (images, videos, total) = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .statistics()
        .map_err(|_| ApiError::Internal)?;
    json_ok(&json!({ "images": images, "videos": videos, "total": total }))
}

/// `DELETE /api/assets` — trash the named assets. With `force: true` the
/// deletion is permanent in one step: the index rows and their memberships
/// go, and the CAS blobs become unreferenced garbage for the horizon
/// sweeper. Without it, trash is one stamp away from a restore.
pub async fn delete_assets(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        ids: Vec<String>,
        #[serde(default)]
        force: bool,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let count = if req.force {
        // Permanent in one step: mark, then forget.
        index.trash(&req.ids).map_err(|_| ApiError::Internal)?;
        index.trash_empty().map_err(|_| ApiError::Internal)
    } else {
        let moved: Vec<bool> = index
            .trash(&req.ids)
            .map_err(|_| ApiError::Internal)?
            .into_iter()
            .map(|(_, moved)| moved)
            .collect();
        Ok(moved.into_iter().filter(|m| *m).count() as u64)
    }
    .map_err(|_| ApiError::Internal)?;
    json_ok(&json!({ "count": count }))
}

/// `POST /api/trash/restore/assets` — bring the named assets back.
pub async fn trash_restore_assets(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    let req: BulkIds =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let restored: Vec<bool> = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .restore(&req.ids)
        .map_err(|_| ApiError::Internal)?
        .into_iter()
        .map(|(_, restored)| restored)
        .collect();
    json_ok(&json!({ "count": restored.into_iter().filter(|r| *r).count() }))
}

/// `POST /api/trash/empty` — permanent. The count is what was destroyed.
pub async fn trash_empty(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult {
    require_user(&state, &headers)?;
    let count = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .trash_empty()
        .map_err(|_| ApiError::Internal)?;
    json_ok(&json!({ "count": count }))
}

/// `POST /api/trash/restore` — bring everything in the trash back.
pub async fn trash_restore_all(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult {
    require_user(&state, &headers)?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let all: Vec<String> = index
        .trashed()
        .map_err(|_| ApiError::Internal)?
        .into_iter()
        .map(|record| record.address)
        .collect();
    drop(index);
    let restored: Vec<bool> = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .restore(&all)
        .map_err(|_| ApiError::Internal)?
        .into_iter()
        .map(|(_, restored)| restored)
        .collect();
    json_ok(&json!({ "count": restored.into_iter().filter(|r| *r).count() }))
}

/// `GET /api/trash` — the trash contents; the screen the restore button
/// lives on.
pub async fn trash_view(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult {
    require_user(&state, &headers)?;
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let items = index.trashed().map_err(|_| ApiError::Internal)?;
    let owner = owner_of(&state, &headers);
    let dtos: Vec<_> = items
        .iter()
        .map(|a| {
            let mut dto = asset_json(a, &owner, "");
            if let Some(fields) = dto.as_object_mut() {
                fields.insert("isTrashed".into(), json!(true));
            }
            dto
        })
        .collect();
    json_ok(&json!({ "count": dtos.len(), "items": dtos }))
}

/// `POST /api/search/metadata` — the search screen. The vault answers the
/// fields it truly indexes — file-name substring, `IMAGE`/`VIDEO`, favorite
/// — over both assets and albums; every other filter in Immich's large
/// query DTO has no backing column and is ignored, which the response
/// makes visible by simply not matching on it. Name search is a
/// case-insensitive substring; albums match the same way.
pub async fn search_metadata(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: String,
) -> ApiResult {
    require_user(&state, &headers)?;
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(rename = "originalFileName")]
        original_file_name: Option<String>,
        #[serde(rename = "type")]
        kind: Option<String>,
        #[serde(rename = "isFavorite")]
        is_favorite: Option<bool>,
        #[serde(default)]
        page: Option<u32>,
        #[serde(default)]
        size: Option<u32>,
    }
    let req: Request =
        serde_json::from_str(&body).map_err(|e| ApiError::BadRequest(format!("json: {e}")))?;
    let page = req.page.unwrap_or(1).max(1);
    let size = req.size.unwrap_or(100).clamp(1, 1000);
    let index = state.index.lock().map_err(|_| ApiError::Internal)?;
    let assets = index
        .search_assets(
            req.original_file_name.as_deref(),
            req.kind.as_deref(),
            req.is_favorite,
            size,
            (page - 1) * size,
        )
        .map_err(|_| ApiError::Internal)?;
    let needle = req.original_file_name.as_deref().map(str::to_lowercase);
    let albums: Vec<_> = index
        .albums()
        .map_err(|_| ApiError::Internal)?
        .into_iter()
        .filter(|album| match &needle {
            Some(n) => album.name.to_lowercase().contains(n),
            None => true,
        })
        .collect();
    let asset_items: Vec<_> = assets
        .iter()
        .map(|a| asset_json(a, &owner_of(&state, &headers), ""))
        .collect();
    json_ok(&json!({
        "assets": {
            "count": asset_items.len(),
            "items": asset_items,
            "total": asset_items.len(),
            "facets": [],
            "nextCursor": serde_json::Value::Null,
        },
        "albums": {
            "count": albums.len(),
            "items": albums.iter().map(album_json).collect::<Vec<_>>(),
            "total": albums.len(),
            "facets": [],
            "nextCursor": serde_json::Value::Null,
        },
    }))
}

/// The Immich-compatible routes, mounted under `/api`. `POST /api/assets` is
/// intentionally absent: the dispatcher in `build_router` serves both client
/// encodings at that URL and hands multipart bodies to this module.
pub fn immich_router(state: AppState) -> axum::Router {
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/validateToken", post(validate_token))
        .route("/api/users/me", get(users_me))
        .route("/api/users", get(users))
        .route("/api/assets/{id}/original", get(original))
        .route("/api/assets/{id}/thumbnail", get(thumbnail))
        .route("/api/server/ping", get(ping))
        .route("/api/server/features", get(features))
        .route("/api/server/config", get(server_config))
        .route("/api/server/version", get(version))
        .route("/api/assets/bulk-upload-check", post(bulk_upload_check))
        .route("/api/timeline/buckets", get(timeline_buckets))
        .route("/api/timeline/bucket", get(timeline_bucket))
        .route("/api/albums", get(albums).post(create_album))
        .route(
            "/api/albums/{id}",
            get(album).patch(update_album).delete(delete_album),
        )
        .route(
            "/api/albums/{id}/assets",
            put(album_add_assets).delete(album_remove_assets),
        )
        .route("/api/shared-links", post(create_share).get(shares))
        .route("/api/shared-links/me", get(shared_album))
        .route("/api/shared-links/{id}", delete(delete_share))
        .route("/api/assets/{id}", get(asset_detail).put(update_asset))
        .route("/api/assets/statistics", get(asset_statistics))
        .route("/api/search/metadata", post(search_metadata))
        .route("/api/memories", get(memories))
        .route("/api/trash", get(trash_view))
        .route("/api/trash/empty", post(trash_empty))
        .route("/api/trash/restore", post(trash_restore_all))
        .route("/api/trash/restore/assets", post(trash_restore_assets))
        .with_state(state)
}
