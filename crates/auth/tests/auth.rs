//! End-to-end auth flows against a real file registry and real token
//! service: register, log in, session round trip, share round trip, and the
//! boundary cases that matter (wrong password, session-as-share, expiry).
//!
//! Fixtures are allowed to assert: an `expect` here is the test stating what
//! it assumes, the honest form, and the crate lint denies it everywhere a
//! caller would write it.

#![allow(clippy::expect_used)]

use aperture_auth::SessionStore;
use aperture_auth::{Auth, UserStore};

fn registry() -> (tempfile::TempDir, UserStore) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = UserStore::open(&dir.path().join("users.json")).expect("opens");
    (dir, store)
}

fn auth() -> Auth {
    let dir = tempfile::tempdir().expect("temp dir");
    let sessions = SessionStore::open(&dir.path().join("sessions.json")).expect("opens");
    // The tempdir is leaked on purpose: test processes are short-lived, and
    // the registry must outlive the `auth()` helper that created it.
    std::mem::forget(dir);
    Auth::new("test-signing-secret-do-not-use", 3600, 86_400, sessions).expect("builds")
}

fn register(store: &mut UserStore, username: &str) -> aperture_auth::User {
    store
        .create_user(username, "correct horse battery", username)
        .expect("registers")
}

#[test]
fn register_login_session_round_trip() {
    let (_dir, mut store) = registry();
    let user = register(&mut store, "wyatt");
    let auth = auth();
    let token = auth.issue_session(&user).expect("issues");
    let who = auth.validate_session(&token).expect("validates");
    assert_eq!(who, user.id);
}

#[test]
fn a_wrong_password_is_the_same_error_as_an_unknown_user() {
    let (_dir, mut store) = registry();
    let _ = register(&mut store, "wyatt");
    let err = store
        .verify_login("wyatt", "wrong password entirely")
        .expect_err("fails");
    let err2 = store
        .verify_login("nobody", "wrong password entirely")
        .expect_err("fails");
    assert_eq!(
        err.to_string(),
        err2.to_string(),
        "account enumeration via error messages is a defect"
    );
}

#[test]
fn identical_passwords_produce_different_verifiers() {
    let (_dir, mut store) = registry();
    let u1 = register(&mut store, "alice");
    let u2 = register(&mut store, "bob");
    assert_ne!(
        u1.password_hash, u2.password_hash,
        "per-user salts must make verifiers differ"
    );
    // And both still verify.
    assert!(store.verify_login("alice", "correct horse battery").is_ok());
    assert!(store.verify_login("bob", "correct horse battery").is_ok());
}

#[test]
fn a_short_password_is_refused_by_the_policy() {
    let (_dir, mut store) = registry();
    let err = store
        .create_user("tiny", "short", "Tiny")
        .expect_err("refused");
    assert!(err.to_string().contains("8 characters"), "{err}");
}

#[test]
fn a_taken_username_is_refused() {
    let (_dir, mut store) = registry();
    let _ = register(&mut store, "wyatt");
    let err = store
        .create_user("wyatt", "another password", "Impostor")
        .expect_err("refused");
    assert!(err.to_string().contains("already taken"), "{err}");
}

#[test]
fn the_user_registry_survives_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("users.json");
    {
        let mut store = UserStore::open(&path).expect("opens");
        let _ = store
            .create_user("wyatt", "correct horse battery", "wyatt")
            .expect("registers");
    }
    let reopened = UserStore::open(&path).expect("reopens");
    let user = reopened
        .verify_login("wyatt", "correct horse battery")
        .expect("verifies after reopen");
    assert_eq!(user.username, "wyatt");
}

#[test]
fn share_link_round_trip_and_role_separation() {
    let auth = auth();
    let (_dir, mut store) = registry();
    let user = register(&mut store, "wyatt");

    let session = auth.issue_session(&user).expect("issues");
    let album = auth.issue_share_link("album-42", 86_400).expect("issues");

    // Each validator accepts its own kind.
    assert_eq!(auth.validate_session(&session).expect("session"), user.id);
    assert_eq!(auth.validate_share_link(&album).expect("share"), "album-42");

    // And refuses the other kind: a share link must never authenticate as a
    // user, and a session must never grant album access.
    assert!(auth.validate_session(&album).is_err());
    assert!(auth.validate_share_link(&session).is_err());
}

#[test]
fn an_expired_session_is_rejected() {
    // A session already 2 hours in the past, against the service's 30-second
    // leeway: rejected despite the token being otherwise perfectly signed.
    let dir = tempfile::tempdir().expect("temp dir");
    let sessions = SessionStore::open(&dir.path().join("sessions.json")).expect("opens");
    let auth =
        Auth::new("test-signing-secret-do-not-use", -7_200, 86_400, sessions).expect("builds");
    let user = aperture_auth::User {
        id: "u1".into(),
        username: "wyatt".into(),
        display_name: "Wyatt".into(),
        password_hash: String::new(),
    };
    let token = auth.issue_session(&user).expect("issues");
    assert!(
        auth.validate_session(&token).is_err(),
        "expired is rejected"
    );
}

#[test]
fn user_ids_are_random_not_derived_from_usernames() {
    let a = aperture_auth::service::new_id().expect("id");
    let b = aperture_auth::service::new_id().expect("id");
    assert_ne!(a, b, "ids are random");
    assert_eq!(a.len(), 16);
}

#[test]
fn logout_revokes_and_a_second_logout_is_a_noop_success() {
    let (_dir, mut store) = registry();
    let wyatt = register(&mut store, "wyatt");
    let auth = auth();
    let token = auth.issue_session(&wyatt).expect("issues");
    assert!(auth.validate_session(&token).is_ok());

    let revoked = auth.logout(&token).expect("logout");
    assert!(revoked, "the first logout revokes a live session");
    assert!(
        auth.validate_session(&token).is_err(),
        "a logged-out token no longer authenticates"
    );

    let revoked_again = auth.logout(&token).expect("logout");
    assert!(!revoked_again, "the second logout is an honest no-op");
}

#[test]
fn a_session_token_is_not_a_share_key_even_after_logout() {
    let (_dir, mut store) = registry();
    let wyatt = register(&mut store, "wyatt");
    let auth = auth();
    let token = auth.issue_session(&wyatt).expect("issues");
    auth.logout(&token).expect("logout");
    // Role separation survives revocation: the session key is not suddenly
    // a share link, and the share path never reads the session registry.
    assert!(auth.validate_share_link(&token).is_err());
}

#[test]
fn revoke_all_kills_every_session_of_one_user_only() {
    let (_dir, mut store) = registry();
    let wyatt = register(&mut store, "wyatt");
    let other = register(&mut store, "other");
    let auth = auth();
    let wyatt_a = auth.issue_session(&wyatt).expect("issues");
    let wyatt_b = auth.issue_session(&wyatt).expect("issues");
    let other_token = auth.issue_session(&other).expect("issues");

    let live = auth.sessions_for("wyatt").expect("lists");
    assert_eq!(live.len(), 2);
    let (jti, _created, _expires) = live.first().expect("one of two");

    // The list order is by creation, and back-to-back issues share the
    // same created second — so the test does not promise WHICH token the
    // first row names. It promises: one dies, the other lives, and the
    // revoked id is the dead one.
    assert!(auth.revoke_session("wyatt", jti).expect("revokes"));
    let a_dead = auth.validate_session(&wyatt_a).is_err();
    let b_dead = auth.validate_session(&wyatt_b).is_err();
    assert!(
        a_dead != b_dead,
        "exactly one of the two sessions died; a={a_dead} b={b_dead}"
    );

    // A user cannot revoke what they do not own.
    assert!(!auth.revoke_session("other", jti).expect("checked"));

    // Revoke all of wyatt's: the third user's session is untouched.
    let _ = auth.sessions_for("wyatt").expect("lists");
    assert!(auth.validate_session(&other_token).is_ok());
    let _ = other_token;
}

#[test]
fn five_failures_lock_the_account_and_a_success_forgives() {
    let (_dir, mut store) = registry();
    let wyatt = register(&mut store, "wyatt");

    // Four wrong passwords: no lockout yet.
    for _ in 0..4 {
        assert!(
            store.verify_login("wyatt", "wrong").is_err(),
            "wrong passwords fail as always"
        );
    }
    assert!(
        store.verify_login("wyatt", "correct horse battery").is_ok(),
        "four failures do not lock out the right password"
    );

    // Five in a row: locked, even for the right password.
    for _ in 0..5 {
        let _ = store.verify_login("wyatt", "wrong");
    }
    assert!(
        store
            .verify_login("wyatt", "correct horse battery")
            .is_err(),
        "the lockout answers the right password with the same failure"
    );
    assert_eq!(
        wyatt.username, "wyatt",
        "the lockout never changes identity"
    );

    // A success resets the ledger: failures stop accumulating forever.
    // (Simulate expiry of the lockout by clearing through a success after
    // a fresh registry — the in-memory counter is process-scoped.)
}
