//! Session and share-link tokens, on estate `tokenkit`.
//!
//! Sessions and share links are both JWTs, separated by the `role` claim:
//! `user` tokens authenticate, `share` tokens grant album access without an
//! account. Each validator refuses the other kind — a share link must never
//! authenticate as a user, and a session must never be accepted as album
//! access.

use crate::{AuthError, User};
use salting::Pepper;
use tokenkit::claims::StandardClaims;
use tokenkit::service::{JwtAlgorithm, JwtConfig, JwtService};

/// Roles, as carried in the token's `role` claim. The two token kinds are
/// separated by this claim, and each validator refuses the other kind.
pub mod role {
    /// An authenticated user's session.
    pub const USER: &str = "user";
    /// A share link: album access without an account.
    pub const SHARE: &str = "share";
}

/// The issuer written into every token and required on validation.
pub const ISSUER: &str = "aperture-vault";

/// Every accepted audience. Sessions use the vault audience; share links use
/// the share audience.
pub const AUDIENCES: [&str; 2] = ["aperture-vault", "share"];

/// Session and share-link token issuance and validation.
pub struct Auth {
    service: JwtService,
    session_ttl: i64,
    share_ttl: i64,
}

impl Auth {
    /// Build the token service. The secret is the signing key for HMAC;
    /// losing it invalidates every outstanding session and share link, which
    /// is the correct failure mode for a vault.
    ///
    /// # Errors
    ///
    /// [`AuthError::Token`] when tokenkit refuses the configuration.
    pub fn new(
        secret: &str,
        session_ttl_secs: i64,
        share_ttl_secs: i64,
    ) -> Result<Self, AuthError> {
        let config = JwtConfig {
            algorithm: JwtAlgorithm::HS256,
            secret: secret.to_string(),
            der_private_key: None,
            public_key: None,
            der_public_key: None,
            issuer: Some(ISSUER.to_string()),
            audience: None,
            audiences: AUDIENCES.iter().map(|s| (*s).to_string()).collect(),
            leeway: Some(30),
            required_claims: vec!["exp".into(), "iss".into(), "sub".into()],
            validate_exp: true,
            validate_nbf: false,
            access_token_ttl: session_ttl_secs,
            refresh_token_ttl: share_ttl_secs,
            key_id: None,
            rotation_keys: Vec::new(),
        };
        Ok(Self {
            service: tokenkit::service::JwtService::new(config),
            session_ttl: session_ttl_secs,
            share_ttl: share_ttl_secs,
        })
    }

    /// Issue a session token for a user.
    ///
    /// # Errors
    ///
    /// [`AuthError::Token`] on encoding failure.
    pub fn issue_session(&self, user: &User) -> Result<String, AuthError> {
        self.issue(&user.id, role::USER, "aperture-vault", self.session_ttl)
    }

    /// Issue a share-link token for an album.
    ///
    /// # Errors
    ///
    /// [`AuthError::Token`] on encoding failure.
    pub fn issue_share_link(&self, album_id: &str, ttl_secs: i64) -> Result<String, AuthError> {
        self.issue(album_id, role::SHARE, "share", ttl_secs)
    }

    fn issue(&self, sub: &str, role: &str, aud: &str, ttl_secs: i64) -> Result<String, AuthError> {
        let now = chrono::Utc::now();
        let claims = StandardClaims {
            sub: Some(sub.to_string()),
            iss: Some(ISSUER.to_string()),
            aud: Some(aud.to_string()),
            exp: Some(now + chrono::Duration::seconds(ttl_secs)),
            iat: Some(now),
            jti: None,
            role: Some(role.to_string()),
            permissions: Vec::new(),
            extra: std::collections::HashMap::new(),
        };
        self.service
            .encode(&claims)
            .map_err(|e| AuthError::Token(e.to_string()))
    }

    /// Validate a session token, returning the user id.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidSession`] when the token is invalid, expired, or
    /// carries a non-session role — a share link must never authenticate as
    /// a user.
    pub fn validate_session(&self, token: &str) -> Result<String, AuthError> {
        let claims = self
            .service
            .validate(token)
            .map_err(|_| AuthError::InvalidSession)?;
        if claims.role.as_deref() != Some(role::USER) {
            return Err(AuthError::InvalidSession);
        }
        claims.sub.ok_or(AuthError::InvalidSession)
    }

    /// Validate a share-link token, returning the album id.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidShare`] when the token is invalid, expired, or is
    /// a session token masquerading as a share.
    pub fn validate_share_link(&self, token: &str) -> Result<String, AuthError> {
        let claims = self
            .service
            .validate(token)
            .map_err(|_| AuthError::InvalidShare)?;
        if claims.role.as_deref() != Some(role::SHARE) {
            return Err(AuthError::InvalidShare);
        }
        claims.sub.ok_or(AuthError::InvalidShare)
    }

    /// The session time-to-live this service was built with (seconds).
    #[must_use]
    pub fn session_ttl(&self) -> i64 {
        self.session_ttl
    }

    /// The share-link time-to-live this service was built with (seconds).
    #[must_use]
    pub fn share_ttl(&self) -> i64 {
        self.share_ttl
    }
}

/// The random-pepper generator salting ships, exposed as the source for
/// stable user ids — one randomness source across the crate, not several.
///
/// # Errors
///
/// salting's own error.
pub fn new_id() -> Result<String, crate::AuthError> {
    let pepper = Pepper::generate(16).map_err(|e| crate::AuthError::Store(format!("id: {e}")))?;
    let hex: String = pepper
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(hex[..16].to_string())
}
