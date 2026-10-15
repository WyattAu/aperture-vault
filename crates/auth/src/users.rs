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
    /// Consecutive password failures per login name, and the instant a
    /// lockout lifts. In-memory by design: it defends against an online
    /// guessing run, and an attacker who can restart the server has
    /// already won something worse than a password.
    failures: std::sync::Mutex<BTreeMap<String, FailureState>>,
}

#[derive(Debug, Default, Clone, Copy)]
struct FailureState {
    count: u32,
    locked_until_unix: Option<i64>,
}

/// Five failures in a row buys the attacker a quarter hour of silence.
const LOCKOUT_THRESHOLD: u32 = 5;
const LOCKOUT_SECS: i64 = 15 * 60;

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
            failures: std::sync::Mutex::new(BTreeMap::new()),
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
        {
            // The lockout gate. The error is the same one a wrong password
            // gets — a lockout that named itself would be an account
            // existence oracle. The timing differs (no hash burned), which
            // is the deliberate, documented residual.
            let failures = self
                .failures
                .lock()
                .map_err(|_| AuthError::Store("poisoned".into()))?;
            if let Some(state) = failures.get(username) {
                if let Some(until) = state.locked_until_unix {
                    if until > unix_now() {
                        return Err(AuthError::InvalidCredentials);
                    }
                }
            }
        }
        let Some(stored) = self.users.get(username) else {
            // Burn a hash anyway: constant-ish work, so the timing of a miss
            // for an unknown user matches the timing of a wrong password.
            let _ = verify_password(password, "$argon2id$placeholder");
            self.note_failure(username);
            return Err(AuthError::InvalidCredentials);
        };
        let ok = verify_password(password, &stored.password_hash)
            .map_err(|e| AuthError::Store(format!("verify: {e}")))?;
        if !ok {
            self.note_failure(username);
            return Err(AuthError::InvalidCredentials);
        }
        // A success clears the ledger: one good password forgives the
        // typos before it.
        self.failures
            .lock()
            .map_err(|_| AuthError::Store("poisoned".into()))?
            .remove(username);
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

    /// Every user, alphabetical by login name — the list the album-share
    /// dialog displays. Verifiers never leave; passwords are not invited.
    #[must_use]
    pub fn users(&self) -> Vec<User> {
        let mut all: Vec<User> = self
            .users
            .values()
            .map(|stored| User {
                id: stored.id.clone(),
                username: stored.username.clone(),
                display_name: stored.display_name.clone(),
                password_hash: stored.password_hash.clone(),
            })
            .collect();
        all.sort_by(|a, b| a.username.cmp(&b.username));
        all
    }

    /// Record a failure and arm the lockout at the threshold. Failures
    /// count consecutively: a success resets, a lockout resets the counter
    /// to zero when it lifts (the next failure starts a fresh series).
    fn note_failure(&self, username: &str) {
        let now = unix_now();
        let mut failures = match self.failures.lock() {
            Ok(f) => f,
            Err(_) => return,
        };
        let state = failures.entry(username.to_string()).or_default();
        if state.locked_until_unix.is_some_and(|until| until > now) {
            return; // already locked; the lockout clock is the punishment
        }
        state.count += 1;
        if state.count >= LOCKOUT_THRESHOLD {
            state.locked_until_unix = Some(now + LOCKOUT_SECS);
            state.count = 0;
        }
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

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
