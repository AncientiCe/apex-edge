//! The hub's session/API-token signing secret.
//!
//! Every device session and admin API token is an HS256 JWT signed with this secret, so a
//! guessable value means anyone on the LAN can mint a token. The hub must never fall back
//! to a built-in string: without `APEX_EDGE_AUTH_SESSION_SIGNING_SECRET` it loads a random
//! secret from a key file, generating it on first boot, so paired devices survive restarts.

use apex_edge_api::{resolve_session_signing_secret, AuthSettings, SigningSecretSource};

const OLD_PUBLIC_DEFAULT: &str = "dev-hub-secret";

fn temp_key_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("apex-edge-secret-{}", uuid::Uuid::new_v4()));
    dir.join(name)
}

#[test]
fn an_explicit_secret_wins_and_no_key_file_is_written() {
    let path = temp_key_path("session.key");
    let (secret, source) =
        resolve_session_signing_secret(Some("operator-chosen-secret".into()), &path).unwrap();
    assert_eq!(secret, "operator-chosen-secret");
    assert_eq!(source, SigningSecretSource::Env);
    assert!(!path.exists());
}

#[test]
fn without_a_secret_one_is_generated_once_and_reused_across_restarts() {
    let path = temp_key_path("session.key");

    let (first, source) = resolve_session_signing_secret(None, &path).unwrap();
    assert_eq!(source, SigningSecretSource::FileGenerated);
    assert_ne!(first, OLD_PUBLIC_DEFAULT);
    assert!(first.len() >= 64, "at least 32 random bytes, hex encoded");
    assert!(path.exists());

    let (second, source) = resolve_session_signing_secret(None, &path).unwrap();
    assert_eq!(source, SigningSecretSource::FileLoaded);
    assert_eq!(second, first, "a restart must not log every register out");
}

#[test]
fn a_blank_env_value_is_treated_as_unset() {
    let path = temp_key_path("session.key");
    let (secret, source) = resolve_session_signing_secret(Some("   ".into()), &path).unwrap();
    assert_eq!(source, SigningSecretSource::FileGenerated);
    assert_ne!(secret.trim(), "");
}

#[test]
fn a_truncated_key_file_is_refused_rather_than_used_as_a_weak_key() {
    let path = temp_key_path("session.key");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "abc\n").unwrap();

    let err = resolve_session_signing_secret(None, &path).unwrap_err();
    assert!(err.to_string().contains("session.key"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_generated_key_file_is_readable_only_by_the_hub_user() {
    use std::os::unix::fs::PermissionsExt;
    let path = temp_key_path("session.key");
    resolve_session_signing_secret(None, &path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn default_settings_never_carry_a_well_known_secret() {
    let a = AuthSettings::default();
    let b = AuthSettings::default();
    assert_ne!(a.session_signing_secret, OLD_PUBLIC_DEFAULT);
    assert_ne!(
        a.session_signing_secret, b.session_signing_secret,
        "random per instance, so no two hubs share a default"
    );
}
