//! The session registry: issued sessions, so they can be revoked.
//!
//! A JWT validates cryptographically forever, which is exactly why "log
//! out" needs state: the registry is the revocation list. A session token
//! is valid only while its row exists and its expiry has not passed. The
//! file follows the same atomic-write pattern as the user registry — a
//! crash leaves the previous registry, never a truncated one.

use crate::AuthError;
use std::collections::BTreeMap;

/// The issued-session registry, backed by a JSON file.
pub struct SessionStore {
    path: std::path::PathBuf,
    sessions: BTreeMap<String, StoredSession>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct StoredSession {
    /// The login name of the session's owner.
    username: String,
    /// Expiry as a Unix timestamp (seconds) — the JWT `exp`, mirrored so
    /// the registry can purge without validating tokens it has never seen.
    expires_at: i64,
}

impl SessionStore {
    /// Open (or create) the registry at `path`, dropping expired rows.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when the file exists but cannot be parsed.
    pub fn open(path: &Path) -> Result<Self, AuthError> {
        let sessions = if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| AuthError::Store(format!("{}: {e}", path.display())))?;
            serde_json::from_str(&text)
                .map_err(|e| AuthError::Store(format!("{}: {e}", path.display())))?
        } else {
            BTreeMap::new()
        };
        let mut store = Self {
            path: path.to_path_buf(),
            sessions,
        };
        store.purge_expired();
        store.persist()?;
        Ok(store)
    }

    /// Record a newly issued session: its `jti`, its owner, its expiry.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when persistence fails.
    pub fn create(
        &mut self,
        jti: &str,
        username: &str,
        expires_at_unix: i64,
    ) -> Result<(), AuthError> {
        self.sessions.insert(
            jti.to_string(),
            StoredSession {
                username: username.to_string(),
                expires_at: expires_at_unix,
            },
        );
        self.persist()
    }

    /// Whether a session id is still live: recorded and unexpired. A
    /// cryptographically valid token whose row is gone is a revoked
    /// session.
    #[must_use]
    pub fn active(&self, jti: &str) -> bool {
        self.sessions
            .get(jti)
            .is_some_and(|session| session.expires_at > unix_now())
    }

    /// The session's owner, when live.
    #[must_use]
    pub fn owner(&self, jti: &str) -> Option<String> {
        self.sessions
            .get(jti)
            .filter(|session| session.expires_at > unix_now())
            .map(|session| session.username.clone())
    }

    /// Revoke one session. Returns whether it was live.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when persistence fails.
    pub fn revoke(&mut self, jti: &str) -> Result<bool, AuthError> {
        let existed = self.sessions.remove(jti).is_some();
        if existed {
            self.persist()?;
        }
        Ok(existed)
    }

    /// Revoke every session of one user. Returns how many were live.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when persistence fails.
    pub fn revoke_all(&mut self, username: &str) -> Result<usize, AuthError> {
        let doomed: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.username == username)
            .map(|(jti, _)| jti.clone())
            .collect();
        for jti in &doomed {
            self.sessions.remove(jti);
        }
        if !doomed.is_empty() {
            self.persist()?;
        }
        Ok(doomed.len())
    }

    /// The live sessions of one user: `(jti, created_at_unix, expires_at_unix)`,
    /// oldest first. Creation time is approximated by the row's recorded
    /// expiry minus the standard session TTL at issue time — the registry
    /// stores one clock, not two.
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] on persistence failure after purging.
    pub fn live_for(&mut self, username: &str) -> Result<Vec<(String, i64, i64)>, AuthError> {
        self.purge_expired();
        self.persist()?;
        let mut live: Vec<(String, i64, i64)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.username == username)
            .map(|(jti, session)| {
                (
                    jti.clone(),
                    session.expires_at.saturating_sub(DEFAULT_TTL_SECS),
                    session.expires_at,
                )
            })
            .collect();
        live.sort_by_key(|(_, created, _)| *created);
        Ok(live)
    }

    /// Drop expired rows.
    fn purge_expired(&mut self) {
        let now = unix_now();
        self.sessions.retain(|_, session| session.expires_at > now);
    }

    fn persist(&self) -> Result<(), AuthError> {
        let text = serde_json::to_string_pretty(&self.sessions)
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

/// The session TTL the registry assumes for "created" approximations. Kept
/// in one place because the day it stops matching `Auth::new`'s default,
/// the created column drifts — the comment exists so the drift is caught
/// by a human who reads either constant.
pub(crate) const DEFAULT_TTL_SECS: i64 = 60 * 60 * 24 * 30;

use std::path::Path;
