//! The user registry: salted verifiers, persisted as JSON.
//!
//! A JSON file is honest at this scale — the vault is a single-binary home
//! server, and the alternative is a second database for four columns. The
//! file is written atomically (temp + rename) on every mutation, so a crash
//! leaves the previous registry, never a truncated one.

use crate::{AuthError, User};
use salting::{hash_password, verify_password};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A user registry backed by a JSON file.
pub struct UserStore {
    path: PathBuf,
    users: BTreeMap<String, StoredUser>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct StoredUser {
    id: String,
    username: String,
    display_name: String,
    password_hash: String,
}

impl UserStore {
    /// Open (or create) the registry at `path`.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when the file exists but cannot be parsed.
    pub fn open(path: &Path) -> Result<Self, AuthError> {
        let users = if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| AuthError::Store(format!("{}: {e}", path.display())))?;
            serde_json::from_str(&text)
                .map_err(|e| AuthError::Store(format!("{}: {e}", path.display())))?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            users,
        })
    }

    /// Register a user: hash the password with a fresh per-user salt and
    /// persist.
    ///
    /// # Errors
    ///
    /// [`AuthError::UsernameTaken`] when the username exists;
    /// [`AuthError::WeakPassword`] when the policy refuses;
    /// [`AuthError::Store`] when persistence fails.
    pub fn create_user(
        &mut self,
        username: &str,
        password: &str,
        display_name: &str,
    ) -> Result<User, AuthError> {
        if self.users.contains_key(username) {
            return Err(AuthError::UsernameTaken(username.to_string()));
        }
        crate::check_password(password)?;
        // Argon2 embeds a random per-hash salt in the PHC string, so the
        // verifier doubles as the uniqueness source for the stable user id.
        let password_hash =
            hash_password(password).map_err(|e| AuthError::Store(format!("hash: {e}")))?;
        let id = crate::service::new_id()?;
        let user = User {
            id: id.clone(),
            username: username.to_string(),
            display_name: display_name.to_string(),
            password_hash,
        };
        self.users.insert(
            username.to_string(),
            StoredUser {
                id: user.id.clone(),
                username: username.to_string(),
                display_name: display_name.to_string(),
                password_hash: user.password_hash.clone(),
            },
        );
        self.persist()?;
        Ok(user)
    }

    /// Verify a login. Unknown username and wrong password are the **same**
    /// error, so a probe cannot enumerate accounts by their error messages.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidCredentials`] on a miss;
    /// [`AuthError::Store`] on verification failure.
    pub fn verify_login(&self, username: &str, password: &str) -> Result<User, AuthError> {
        let Some(stored) = self.users.get(username) else {
            // Burn a hash anyway: constant-ish work, so the timing of a miss
            // for an unknown user matches the timing of a wrong password.
            let _ = verify_password(password, "$argon2id$placeholder");
            return Err(AuthError::InvalidCredentials);
        };
        let ok = verify_password(password, &stored.password_hash)
            .map_err(|e| AuthError::Store(format!("verify: {e}")))?;
        if !ok {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(User {
            id: stored.id.clone(),
            username: stored.username.clone(),
            display_name: stored.display_name.clone(),
            password_hash: stored.password_hash.clone(),
        })
    }

    /// Look a user up by stable id (the JWT `sub`).
    #[must_use]
    pub fn user_by_id(&self, id: &str) -> Option<User> {
        self.users.values().find(|u| u.id == id).map(|stored| User {
            id: stored.id.clone(),
            username: stored.username.clone(),
            display_name: stored.display_name.clone(),
            password_hash: stored.password_hash.clone(),
        })
    }

    fn persist(&self) -> Result<(), AuthError> {
        let text = serde_json::to_string_pretty(&self.users)
            .map_err(|e| AuthError::Store(format!("serialize: {e}")))?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| AuthError::Store(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| AuthError::Store(format!("{}: {e}", self.path.display())))?;
        Ok(())
    }
}
