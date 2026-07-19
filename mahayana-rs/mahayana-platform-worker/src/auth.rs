use argon2::Algorithm;
use argon2::Argon2;
use argon2::Params;
use argon2::Version;
use argon2::password_hash::PasswordHash;
use argon2::password_hash::PasswordHasher;
use argon2::password_hash::PasswordVerifier;
use argon2::password_hash::SaltString;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pbkdf2::pbkdf2_hmac;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

const DEFAULT_PBKDF2_ITERATIONS: u32 = 100_000;
const MAX_PBKDF2_ITERATIONS: u32 = 2_000_000;
const ARGON2_MEMORY_KIB: u32 = 19 * 1024;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_LANES: u32 = 1;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthCryptoError {
    #[error("invalid password credential encoding")]
    InvalidCredentialEncoding,
    #[error("unsupported password algorithm")]
    UnsupportedPasswordAlgorithm,
    #[error("password credential parameters are outside the accepted range")]
    InvalidCredentialParameters,
    #[error("password hashing failed")]
    PasswordHash,
}

pub fn verify_pbkdf2_sha256(
    password: &str,
    encoded_salt: &str,
    encoded_hash: &str,
    iterations: Option<i64>,
    algorithm: Option<&str>,
) -> Result<bool, AuthCryptoError> {
    if algorithm
        .filter(|algorithm| !algorithm.eq_ignore_ascii_case("PBKDF2-SHA256"))
        .is_some()
    {
        return Err(AuthCryptoError::UnsupportedPasswordAlgorithm);
    }
    let iterations = iterations.unwrap_or(i64::from(DEFAULT_PBKDF2_ITERATIONS));
    let iterations = u32::try_from(iterations)
        .ok()
        .filter(|iterations| (1..=MAX_PBKDF2_ITERATIONS).contains(iterations))
        .ok_or(AuthCryptoError::InvalidCredentialParameters)?;
    let salt = decode_base64_url(encoded_salt)?;
    let expected = decode_base64_url(encoded_hash)?;
    if salt.len() < 8 || expected.len() != 32 {
        return Err(AuthCryptoError::InvalidCredentialParameters);
    }

    let mut derived = vec![0_u8; expected.len()];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut derived);
    Ok(constant_time_eq(&derived, &expected))
}

pub fn hash_password_argon2id(password: &str, salt: &[u8]) -> Result<String, AuthCryptoError> {
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_LANES, Some(32))
        .map_err(|_| AuthCryptoError::InvalidCredentialParameters)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::encode_b64(salt).map_err(|_| AuthCryptoError::PasswordHash)?;
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthCryptoError::PasswordHash)
}

pub fn verify_argon2id(password: &str, encoded_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded_hash) else {
        return false;
    };
    hash.algorithm.as_str() == "argon2id"
        && Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn new_password_salt() -> [u8; 16] {
    *Uuid::new_v4().as_bytes()
}

pub fn new_refresh_token() -> String {
    let first = Uuid::new_v4().simple().to_string();
    let second = Uuid::new_v4().simple().to_string();
    format!("mrt_{first}{second}")
}

pub fn hash_refresh_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_base64_url(value: &str) -> Result<Vec<u8>, AuthCryptoError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .map_err(|_| AuthCryptoError::InvalidCredentialEncoding)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_the_existing_javascript_pbkdf2_format() {
        let salt = URL_SAFE_NO_PAD.encode(b"0123456789abcdef");
        let mut expected = [0_u8; 32];
        pbkdf2_hmac::<Sha256>(
            b"correct horse",
            b"0123456789abcdef",
            100_000,
            &mut expected,
        );
        let expected = URL_SAFE_NO_PAD.encode(expected);

        assert_eq!(
            verify_pbkdf2_sha256(
                "correct horse",
                &salt,
                &expected,
                Some(100_000),
                Some("PBKDF2-SHA256"),
            ),
            Ok(true)
        );
        assert_eq!(
            verify_pbkdf2_sha256(
                "wrong",
                &salt,
                &expected,
                Some(100_000),
                Some("PBKDF2-SHA256"),
            ),
            Ok(false)
        );
    }

    #[test]
    fn argon2_upgrade_round_trips_without_changing_legacy_credentials() {
        let hash = hash_password_argon2id("upgrade me", b"0123456789abcdef").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_argon2id("upgrade me", &hash));
        assert!(!verify_argon2id("wrong", &hash));
    }

    #[test]
    fn refresh_tokens_are_opaque_and_only_their_hash_is_persistable() {
        let token = new_refresh_token();
        assert!(token.starts_with("mrt_"));
        assert_eq!(token.len(), 68);
        let hash = hash_refresh_token(&token);
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(&token));
    }
}
