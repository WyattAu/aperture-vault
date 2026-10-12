//! Multi-user authentication for the Aperture vault.
//!
//! Built on the estate's published `salting` (Argon2 password verifiers,
//! per-user salts) and `tokenkit` (JWT sessions and capability tokens)
//! crates — the same auth substrate `estate-integration` verifies
//! interoperate. The vault's auth decisions are therefore exercised by
//! proofs and audits that already exist, rather than re-implemented here.
//!
//! # What this module decides
//!
//! - A **user record** is a salted Argon2 verifier plus a display name. The
//!   salt is per-user and random, so identical passwords produce different
//!   verifiers — verified by test.
//! - A **session** is a JWT with `role: "user"`, `sub: user-id`, and an
//!   expiry.
//! - A **share link** is a JWT with `role: "share"`, `sub: album-id`, and an
//!   expiry. It names no user: the recipient views the album without an
//!   account.
//!
//! The two token kinds are separated by the `role` claim, and each validator
//! refuses the other kind — a share link must never authenticate as a user.
//!
//! # What this module deliberately does not do
//!
//! - **No network.** The HTTP layer maps these decisions onto requests.
//! - **No password complexity policy.** Length is the property that actually
//!   resists offline guessing (`MIN_PASSWORD_LENGTH`); character-class
//!   theatre is not implemented, and this is a decision, not an omission.

pub mod service;
pub mod users;

pub use service::{Auth, ISSUER};
pub use users::UserStore;

/// The minimum accepted password length. Length is the property that
/// actually resists offline guessing; character-class theatre is not a
/// policy, and this is a decision, not an omission.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// One registered user.
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    /// Stable identifier (random hex), the JWT `sub`.
    pub id: String,
    /// Login name, unique.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Argon2 verifier in PHC format (salt embedded).
    pub password_hash: String,
}

/// Validate a password against the vault's policy.
///
/// # Errors
///
/// [`AuthError::WeakPassword`] with the user-facing message.
pub fn check_password(password: &str) -> Result<(), AuthError> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(AuthError::WeakPassword(format!(
            "password must be at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    Ok(())
}

/// Errors from the auth layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The username is already taken.
    #[error("username already taken: {0}")]
    UsernameTaken(String),
    /// The username is unknown, or the password is wrong — deliberately the
    /// same error, so a probe cannot enumerate accounts.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// The password does not meet the policy.
    #[error("weak password: {0}")]
    WeakPassword(String),
    /// The token is not a valid session.
    #[error("invalid session token")]
    InvalidSession,
    /// The token is not a valid share link.
    #[error("invalid share token")]
    InvalidShare,
    /// The user store could not be read or written.
    #[error("user store: {0}")]
    Store(String),
    /// Token issuance or validation failed.
    #[error("token: {0}")]
    Token(String),
}
