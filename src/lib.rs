//! Multi-user authentication for the Aperture vault.
//!
//! Built on the estate's published [`salting`] and [`tokenkit`] crates — the
//! same auth substrate `estate-integration` verifies interoperate — so the
//! vault's sessions and share links use mechanisms that are already
//! property-tested and audited elsewhere in the estate.
//!
//! # What this module decides
//!
//! - A **user record** is a salted password verifier plus a display name; the
//!   salt is per-user, so identical passwords produce different verifiers.
//! - A **session** is a capability token scoped to one user, with an expiry.
//! - A **share link** is a capability token scoped to an album, optionally
//!   expiring, that names no user: the recipient can view the album without
//!   an account.
//!
//! # What this module deliberately does not do
//!
//! - **No network.** The HTTP layer maps these decisions onto requests.
//! - **No password policy.** Length is checked; complexity theatre is not.
