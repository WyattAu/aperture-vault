//! End-to-end auth flows against a real file registry and real token
//! service: register, log in, session round trip, share round trip, and the
//! boundary cases that matter (wrong password, session-as-share, expiry).
//!
//! Fixtures are allowed to assert: an `expect` here is the test stating what
//! it assumes, the honest form, and the crate lint denies it everywhere a
//! caller would write it.

#![allow(clippy::expect_used)]

use aperture_auth::{Auth, UserStore};

fn registry() -> (tempfile::TempDir, UserStore) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = UserStore::open(&dir.path().join("users.json")).expect("opens");
    (dir, store)
}

fn auth() -> Auth {
    Auth::new("test-signing-secret-do-not-use", 3600, 86_400).expect("builds")
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
    let auth = Auth::new("test-signing-secret-do-not-use", -7_200, 86_400).expect("builds");
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
