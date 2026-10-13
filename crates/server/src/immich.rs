//! Immich-compatible API endpoints, so the existing Immich mobile apps
//! attach to this server unchanged. See the crate docs for the honest
//! boundary: ids are content addresses, not UUIDs.

use crate::{mime_for, require_user, ApiError, AppState};
use axum::extract::{Multipart, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

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
    json_created(&json!({
        "id": report.address.hex(),
        "duplicate": !report.newly_written,
    }))
}

/// `GET /api/assets/:id/original` — the original bytes, content type sniffed.
pub async fn original(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    require_user(&state, &headers)?;
    let addr = crate::cas_address(&id)?;
    let bytes = state.store.get_bytes(&addr).map_err(|e| match e {
        aperture_cas::CasError::Missing { .. } => ApiError::NotFound,
        _other => ApiError::Internal,
    })?;
    let mime = mime_for(&bytes);
    Ok(([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response())
}

/// The Immich-compatible routes, mounted under `/api`. `POST /api/assets` is
/// intentionally absent: the dispatcher in `build_router` serves both client
/// encodings at that URL and hands multipart bodies to this module.
pub fn immich_router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/validateToken", post(validate_token))
        .route("/api/users/me", get(users_me))
        .route("/api/assets/{id}/original", get(original))
        .with_state(state)
}
