//! WEBX-3: password-hashing port + default Argon2id implementation.
//!
//! The point of the port is that a business crate keeps **no** hashing code: it
//! injects `Arc<dyn IPasswordHasher>`, and the framework's default implementation
//! runs Argon2id on a blocking thread. These tests pin the attack-relevant
//! contract: malformed stored hashes are typed errors, never panics, and an old
//! (weaker-parameter) hash still verifies while flagging `needs_rehash`.

use webx_core::error::Error;
use webx_core::security::IPasswordHasher;
use webx_host::password_argon2::{Argon2Params, Argon2PasswordHasher};

/// Host startup must register the default hasher, so business code only injects
/// `Arc<dyn IPasswordHasher>` and writes no Argon2 code.
#[tokio::test]
async fn host_registers_the_default_argon2_hasher_in_di() {
    let host = webx_host::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .try_build()
        .expect("host must build");

    let hasher = host
        .provider()
        .get::<dyn IPasswordHasher>()
        .expect("the default password hasher must be a host service");
    let phc = hasher.hash("pw").await.expect("hashing must work");
    assert!(phc.starts_with("$argon2id$"), "got {phc}");
}

/// `use_password_hasher` replaces the default registration (the "configure only"
/// path Travel needs).
#[tokio::test]
async fn use_password_hasher_replaces_the_default_registration() {
    let host = webx_host::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_password_hasher(std::sync::Arc::new(Argon2PasswordHasher::with_params(
            Argon2Params::new(65536, 3, 1),
        )))
        .try_build()
        .expect("host must build");

    let hasher = host
        .provider()
        .get::<dyn IPasswordHasher>()
        .expect("the configured hasher must be registered");
    let phc = hasher.hash("pw").await.expect("hashing must work");
    assert!(
        phc.contains("m=65536,t=3,p=1"),
        "the replacement hasher's parameters must be used, got {phc}"
    );
}

#[tokio::test]
async fn hash_verify_and_needs_rehash_round_trip() {
    let hasher = Argon2PasswordHasher::default();
    let phc = hasher.hash("s3cret-password").await.expect("hash must succeed");

    assert!(
        phc.starts_with("$argon2id$"),
        "the default algorithm must be Argon2id, got {phc}"
    );
    assert!(!hasher.needs_rehash(&phc), "a fresh hash must not need rehash");
    assert!(hasher.verify("s3cret-password", &phc).await.unwrap());
    assert!(
        !hasher.verify("wrong-password", &phc).await.unwrap(),
        "a wrong password is Ok(false), not an error"
    );
}

#[tokio::test]
async fn each_hash_uses_a_fresh_salt() {
    let hasher = Argon2PasswordHasher::default();
    let first = hasher.hash("same-password").await.unwrap();
    let second = hasher.hash("same-password").await.unwrap();

    assert_ne!(first, second, "the salt must be random per hash");
    assert!(hasher.verify("same-password", &first).await.unwrap());
    assert!(hasher.verify("same-password", &second).await.unwrap());
}

#[tokio::test]
async fn configured_parameters_are_reflected_in_the_phc_string() {
    let hasher = Argon2PasswordHasher::with_params(Argon2Params::new(8704, 3, 2));
    let phc = hasher.hash("pw").await.unwrap();

    assert!(phc.contains("m=8704"), "got {phc}");
    assert!(phc.contains("t=3"), "got {phc}");
    assert!(phc.contains("p=2"), "got {phc}");
    assert!(hasher.verify("pw", &phc).await.unwrap());
    assert!(!hasher.needs_rehash(&phc));
}

#[tokio::test]
async fn outdated_or_foreign_parameters_need_rehash_but_still_verify() {
    let legacy = Argon2PasswordHasher::with_params(Argon2Params::new(8192, 1, 1));
    let current = Argon2PasswordHasher::default();

    let legacy_phc = legacy.hash("pw").await.unwrap();
    assert!(!legacy.needs_rehash(&legacy_phc));
    assert!(
        current.needs_rehash(&legacy_phc),
        "hashes made with weaker parameters must be upgraded on next login"
    );
    assert!(
        current.verify("pw", &legacy_phc).await.unwrap(),
        "an old hash must keep verifying; otherwise the migration locks users out"
    );
}

/// Unknown algorithm prefix: a typed server-side error, not `Ok(false)` (which
/// would read as "wrong password") and not a panic.
#[tokio::test]
async fn unknown_algorithm_is_a_typed_error() {
    let hasher = Argon2PasswordHasher::default();

    for stored in [
        "$bcrypt$v=2$t=1$c29tZXNhbHQ$aGFzaA",
        "$scrypt$ln=16,r=8,p=1$c2FsdA$aGFzaA",
        "$md5$abcdef",
    ] {
        let err = hasher
            .verify("pw", stored)
            .await
            .expect_err("an unsupported algorithm must be an Err");
        assert!(
            matches!(err, Error::PasswordHash(_)),
            "expected a typed PasswordHash error for {stored}, got {err:?}"
        );
        assert_eq!(err.status_code(), 500, "stored-hash problems are server-side");
    }
}

/// Malformed PHC strings are stored-data corruption: typed `Err`, never panic.
#[tokio::test]
async fn malformed_phc_is_a_typed_error_not_a_panic() {
    let hasher = Argon2PasswordHasher::default();

    for stored in [
        "",
        "not-a-phc-string",
        "$argon2id$",
        "$argon2id$v=19$m=notanumber$c2FsdA$aGFzaA",
        "$argon2id$v=19",
    ] {
        let err = hasher
            .verify("pw", stored)
            .await
            .expect_err("a malformed stored hash must be an Err");
        assert!(
            matches!(err, Error::PasswordHash(_)),
            "expected a typed PasswordHash error for {stored:?}, got {err:?}"
        );
    }
}

#[tokio::test]
async fn needs_rehash_is_true_for_malformed_or_foreign_hashes() {
    let hasher = Argon2PasswordHasher::default();

    assert!(
        hasher.needs_rehash("not-a-phc-string"),
        "a hash that cannot be parsed cannot be trusted: rehash"
    );
    assert!(
        hasher.needs_rehash("$bcrypt$v=2$t=1$c29tZXNhbHQ$aGFzaA"),
        "a foreign algorithm must be rehashed into the configured one"
    );
}

/// Invalid Argon2 parameters are configuration errors reported as a typed Err —
/// construction and hashing must not panic.
#[tokio::test]
async fn invalid_parameters_are_a_typed_error() {
    let hasher = Argon2PasswordHasher::with_params(Argon2Params::new(1, 0, 0));
    let err = hasher
        .hash("pw")
        .await
        .expect_err("invalid Argon2 parameters must be an Err");
    assert!(matches!(err, Error::PasswordHash(_)), "got {err:?}");
}
