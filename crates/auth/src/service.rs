//! Session and share-link tokens, on estate `tokenkit`.
//!
//! Sessions and share links are both JWTs, separated by the `role` claim:
//! `user` tokens authenticate, `share` tokens grant album access without an
//! account. Each validator refuses the other kind — a share link must never
//! authenticate as a user, and a session must never be accepted as album
//! access.

use crate::sessions::SessionStore;
use crate::{AuthError, User};
use salting::Pepper;
use std::sync::Mutex;
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
///
/// Sessions are revocable: every issued session carries a `jti` recorded in
/// the [`SessionStore`], and validation requires the row to still be live.
/// The JWT proves authenticity; the registry decides liveness.
pub struct Auth {
    service: JwtService,
    session_ttl: i64,
    share_ttl: i64,
    sessions: Mutex<SessionStore>,
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
        sessions: SessionStore,
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
            sessions: Mutex::new(sessions),
        })
    }

    /// Issue a session token for a user.
    ///
    /// # Errors
    ///
    /// [`AuthError::Token`] on encoding failure.
    pub fn issue_session(&self, user: &User) -> Result<String, AuthError> {
        let jti = crate::service::new_id()?;
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(self.session_ttl);
        self.sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?
            .create(&jti, &user.username, expires_at.timestamp())?;
        self.issue_with_jti(
            &user.id,
            role::USER,
            "aperture-vault",
            self.session_ttl,
            jti,
        )
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
        let jti = claims.jti.ok_or(AuthError::InvalidSession)?;
        let live = self
            .sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?
            .active(&jti);
        if !live {
            return Err(AuthError::InvalidSession);
        }
        claims.sub.ok_or(AuthError::InvalidSession)
    }

    /// The `jti` of a live session token — which row in the registry is
    /// "me". Every other session in the list is *not* the caller's.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidSession`] when the token is not a live session.
    pub fn session_jti(&self, token: &str) -> Result<String, AuthError> {
        let claims = self
            .service
            .validate(token)
            .map_err(|_| AuthError::InvalidSession)?;
        if claims.role.as_deref() != Some(role::USER) {
            return Err(AuthError::InvalidSession);
        }
        let jti = claims.jti.ok_or(AuthError::InvalidSession)?;
        if !self
            .sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?
            .active(&jti)
        {
            return Err(AuthError::InvalidSession);
        }
        Ok(jti)
    }

    /// Revoke the session a token names. Returns whether it was live —
    /// logging out twice is not an error, it is a no-op that succeeded.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidSession`] when the token is not a live session;
    /// [`AuthError::Store`] on registry failure.
    pub fn logout(&self, token: &str) -> Result<bool, AuthError> {
        let claims = self
            .service
            .validate(token)
            .map_err(|_| AuthError::InvalidSession)?;
        if claims.role.as_deref() != Some(role::USER) {
            return Err(AuthError::InvalidSession);
        }
        let jti = claims.jti.ok_or(AuthError::InvalidSession)?;
        self.sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?
            .revoke(&jti)
    }

    /// The live sessions of one user, oldest first:
    /// `(jti, created_at, expires_at)` as local timestamps.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] on registry failure.
    pub fn sessions_for(&self, username: &str) -> Result<Vec<(String, String, String)>, AuthError> {
        let fmt = |unix: i64| {
            chrono::DateTime::from_timestamp(unix, 0)
                .map(|utc| utc.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default()
        };
        self.sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?
            .live_for(username)
            .map(|live| {
                live.into_iter()
                    .map(|(jti, created, expires)| (jti, fmt(created), fmt(expires)))
                    .collect()
            })
    }

    /// Revoke one of a user's own sessions. Returns whether it was live.
    /// A user can only ever revoke their own: the registry is queried by
    /// the owner, never by id alone.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] on registry failure.
    pub fn revoke_session(&self, username: &str, jti: &str) -> Result<bool, AuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| AuthError::Store("session registry poisoned".into()))?;
        let owned = sessions
            .live_for(username)
            .is_ok_and(|live| live.iter().any(|(owned_jti, _, _)| owned_jti == jti));
        if owned {
            sessions.revoke(jti)
        } else {
            Ok(false)
        }
    }

    /// Issue with an explicit `jti`, shared by `issue_session`.
    fn issue_with_jti(
        &self,
        sub: &str,
        role: &str,
        aud: &str,
        ttl_secs: i64,
        jti: String,
    ) -> Result<String, AuthError> {
        let now = chrono::Utc::now();
        let claims = StandardClaims {
            sub: Some(sub.to_string()),
            iss: Some(ISSUER.to_string()),
            aud: Some(aud.to_string()),
            exp: Some(now + chrono::Duration::seconds(ttl_secs)),
            iat: Some(now),
            jti: Some(jti),
            role: Some(role.to_string()),
            permissions: Vec::new(),
            extra: std::collections::HashMap::new(),
        };
        self.service
            .encode(&claims)
            .map_err(|e| AuthError::Token(e.to_string()))
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
