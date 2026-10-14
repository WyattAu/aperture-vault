//! The Aperture vault HTTP server.
//!
//! A thin translation layer: every route maps an HTTP request onto a decision
//! the library crates have already made — auth on `aperture-auth`, storage on
//! `aperture-cas`, metadata on `aperture-probe`, queries on `aperture-index`,
//! composition on `aperture-pipeline`. No business logic lives here; the HTTP
//! layer's only job is to carry requests in and responses out faithfully.
//!
//! # Testing
//!
//! [`build_router`] is a pure function of the state, and the tests drive the
//! router through `tower::ServiceExt::oneshot` — no sockets, no ports, no
//! race conditions. The binary in `main.rs` only adds the bind and serve.

use aperture_auth::{Auth, UserStore};
use aperture_cas::Store;
use aperture_index::Index;
use axum::response::IntoResponse as _;
use std::path::Path;
use std::sync::Arc;

pub mod immich;

/// The shared application state, cheap to clone.
#[derive(Clone)]
pub struct AppState {
    store: Arc<Store>,
    // rusqlite's Connection is single-threaded by design; the mutex is the
    // honest wrapper for an embedded single-writer database, not a wart.
    index: Arc<std::sync::Mutex<Index>>,
    auth: Arc<Auth>,
    users: Arc<std::sync::Mutex<UserStore>>,
}

impl AppState {
    /// Assemble the state from the vault's parts. The token signing secret
    /// is generated per-install and persisted beside the index: restarting
    /// the server must not invalidate outstanding sessions.
    ///
    /// # Errors
    ///
    /// A string describing the failure — the server cannot start.
    pub fn new(vault_root: &Path) -> Result<Self, String> {
        let store =
            Store::open(vault_root.join("objects-root")).map_err(|e| format!("store: {e}"))?;
        let index = Index::open(&vault_root.join("index.db")).map_err(|e| format!("index: {e}"))?;
        let users =
            UserStore::open(&vault_root.join("users.json")).map_err(|e| format!("users: {e}"))?;
        let secret_path = vault_root.join("signing-secret");
        let secret = match std::fs::read_to_string(&secret_path) {
            Ok(s) => s,
            Err(_) => {
                let s = aperture_auth::service::new_id().map_err(|e| format!("secret: {e}"))?;
                std::fs::write(&secret_path, &s)
                    .map_err(|e| format!("{}: {e}", secret_path.display()))?;
                s
            }
        };
        let auth =
            Auth::new(&secret, 86_400 * 30, 86_400 * 365).map_err(|e| format!("auth: {e}"))?;
        Ok(Self {
            store: Arc::new(store),
            index: Arc::new(std::sync::Mutex::new(index)),
            auth: Arc::new(auth),
            users: Arc::new(std::sync::Mutex::new(users)),
        })
    }
    /// Register a user directly in the registry.
    ///
    /// # Errors
    ///
    /// Auth errors from the registry — taken username, weak password,
    /// persistence failure.
    pub fn register_user(
        &self,
        username: &str,
        password: &str,
        display_name: &str,
    ) -> Result<aperture_auth::User, aperture_auth::AuthError> {
        self.users
            .lock()
            .map_err(|_| aperture_auth::AuthError::Store("poisoned".into()))?
            .create_user(username, password, display_name)
    }

    /// Verify a login through the registry.
    ///
    /// # Errors
    ///
    /// `InvalidCredentials` on a miss; `Store` on persistence failure.
    pub fn verify_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<aperture_auth::User, aperture_auth::AuthError> {
        self.users
            .lock()
            .map_err(|_| aperture_auth::AuthError::Store("poisoned".into()))?
            .verify_login(username, password)
    }

    /// The token service, for callers issuing tokens outside the HTTP flow.
    #[must_use]
    pub fn auth(&self) -> &Auth {
        &self.auth
    }
}

/// The error type every handler returns.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request is unauthenticated or the token is invalid.
    #[error("unauthorized")]
    Unauthorized,
    /// The request body or parameters are malformed.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// The addressed asset is not in the vault.
    #[error("not found")]
    NotFound,
    /// Anything else.
    #[error("internal error")]
    Internal,
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let status = match &self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

type ApiResult = Result<axum::response::Response, ApiError>;

/// Build the application router.
pub fn build_router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};
    // Login is served by the Immich module: its `{email, password}` shape is
    // the one the Immich mobile apps speak, and `username` is accepted as an
    // alias for native clients. One endpoint, one truth.
    let native = axum::Router::new()
        .route("/api/timeline", get(timeline))
        .route("/api/assets", post(upload_dispatch))
        .with_state(state.clone());
    native.merge(crate::immich::immich_router(state))
}

/// Extract and validate the bearer token, returning the user id.
pub(crate) fn require_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<String, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    state
        .auth
        .validate_session(token)
        .map_err(|_| ApiError::Unauthorized)
}

#[derive(serde::Deserialize)]
struct TimelineParams {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn timeline(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<TimelineParams>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let entries = state
        .index
        .lock()
        .map_err(|_| ApiError::Internal)?
        .timeline(params.limit.unwrap_or(100), params.offset.unwrap_or(0))
        .map_err(|_| ApiError::Internal)?;
    Ok(axum::Json(entries).into_response())
}

#[derive(serde::Deserialize)]
struct UploadParams {
    filename: Option<String>,
}

/// `POST /api/assets` — one URL, two client encodings. Immich mobile apps
/// speak multipart with an `assetData` field; native clients send raw bytes
/// with a `?filename=` query. The dispatcher branches on Content-Type so
/// each client keeps its exact contract, and there is exactly one upload
/// route to reason about.
async fn upload_dispatch(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    params: axum::extract::Query<UploadParams>,
    request: axum::extract::Request,
) -> ApiResult {
    let is_multipart = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    if is_multipart {
        use axum::extract::FromRequest as _;
        let multipart = axum::extract::Multipart::from_request(request, &state)
            .await
            .map_err(|e| ApiError::BadRequest(format!("multipart: {e}")))?;
        return crate::immich::upload_asset_multipart(state, headers, multipart).await;
    }
    let body = axum::body::to_bytes(request.into_body(), MAX_UPLOAD_BYTES)
        .await
        .map_err(|e| ApiError::BadRequest(format!("body: {e}")))?;
    upload(axum::extract::State(state), headers, params, body).await
}

/// An upper bound on a single upload: 4 GiB, generous for a photo vault and
/// a deliberate refusal to let a client stream forever.
const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024 * 1024;

async fn upload(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<UploadParams>,
    body: axum::body::Bytes,
) -> ApiResult {
    require_user(&state, &headers)?;
    let report = aperture_pipeline::Pipeline::ingest(
        &state.store,
        &*state.index.lock().map_err(|_| ApiError::Internal)?,
        Cursor::new(&body),
        params.filename.as_deref(),
    )
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let body = serde_json::to_vec(&serde_json::json!({
        "address": report.address.hex(),
        "newlyWritten": report.newly_written,
        "indexed": report.indexed,
    }))
    .map_err(|_| ApiError::Internal)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

use std::io::Cursor;

/// Parse an asset id into its content address.
pub(crate) fn cas_address(id: &str) -> Result<aperture_cas::Address, ApiError> {
    aperture_cas::Address::parse_hex(id).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Sniff the content type from the leading bytes: photos and videos are
/// identified by magic, not by the name the uploader chose. Unrecognised
/// content is served as `application/octet-stream` — served, not refused,
/// because the vault stores what was uploaded.
pub(crate) fn mime_for(data: &[u8]) -> &'static str {
    match (data.first(), data.get(1), data.get(2), data.get(3)) {
        (Some(0xFF), Some(0xD8), _, _) => "image/jpeg",
        (Some(0x89), Some(0x50), Some(0x4E), Some(0x47)) => "image/png",
        (Some(f), Some(t), Some(_), Some(_))
            if *f == b'I'
                && *t == b'A'
                && *data.get(4).unwrap_or(&0) == b'A'
                && *data.get(5).unwrap_or(&0) == b'A' =>
        {
            "image/heic"
        }
        (Some(0x1A), Some(0x45), Some(0xDF), Some(0xA3)) => "video/webm",
        (Some(0x00), Some(0x00), Some(0x00), Some(0x18))
        | (Some(0x00), Some(0x00), Some(0x00), Some(0x20)) => "video/mp4",
        _ => "application/octet-stream",
    }
}
