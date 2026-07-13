use crate::{handshake::KNOWN_DH_PRIME_HEX, tl::TlWriter};
use num_bigint_dig::BigUint;
use pbkdf2::pbkdf2_hmac;
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

pub const INPUT_CHECK_PASSWORD_SRP_CONSTRUCTOR: u32 = 0xd27f_f082;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordSrpParameters {
    pub srp_id: i64,
    pub salt1: Vec<u8>,
    pub salt2: Vec<u8>,
    pub generator: i32,
    pub prime: Vec<u8>,
    pub server_b: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordSrpProof {
    pub srp_id: i64,
    pub a: [u8; 256],
    pub m1: [u8; 32],
}

impl PasswordSrpProof {
    pub fn encode_input_check_password(&self) -> Result<Vec<u8>, SrpError> {
        let mut writer = TlWriter::new();
        writer.write_u32(INPUT_CHECK_PASSWORD_SRP_CONSTRUCTOR);
        writer.write_i64(self.srp_id);
        writer.write_bytes(&self.a)?;
        writer.write_bytes(&self.m1)?;
        Ok(writer.into_bytes())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SrpError {
    #[error("Telegram password must not be empty")]
    EmptyPassword,
    #[error("Telegram SRP prime or generator is not trusted")]
    InvalidPrimeOrGenerator,
    #[error("Telegram SRP server B is outside the safe range")]
    InvalidServerB,
    #[error("Telegram SRP random exponent is zero")]
    InvalidRandomExponent,
    #[error("operating system secure randomness is unavailable")]
    RandomnessUnavailable,
    #[error("TL encoding failed: {0}")]
    Tl(#[from] crate::tl::TlError),
}

pub fn compute_password_srp_proof(
    password: &str,
    parameters: &PasswordSrpParameters,
) -> Result<PasswordSrpProof, SrpError> {
    compute_password_srp_proof_with_random(password, parameters, |output| {
        getrandom::getrandom(output).map_err(|_| SrpError::RandomnessUnavailable)
    })
}

pub fn compute_password_srp_proof_with_random<F>(
    password: &str,
    parameters: &PasswordSrpParameters,
    mut fill_random: F,
) -> Result<PasswordSrpProof, SrpError>
where
    F: FnMut(&mut [u8]) -> Result<(), SrpError>,
{
    if password.is_empty() {
        return Err(SrpError::EmptyPassword);
    }
    validate_prime_generator(&parameters.prime, parameters.generator)?;
    if !(248..=256).contains(&parameters.server_b.len()) {
        return Err(SrpError::InvalidServerB);
    }
    let prime = BigUint::from_bytes_be(&parameters.prime);
    let server_b = BigUint::from_bytes_be(&parameters.server_b);
    if server_b == BigUint::from(0_u8) || server_b >= prime {
        return Err(SrpError::InvalidServerB);
    }

    let generator = BigUint::from(parameters.generator as u32);
    let generator_padded = padded_256(&generator);
    let server_b_padded = padded_256(&server_b);
    let password_hash = Zeroizing::new(password_hash(
        password.as_bytes(),
        &parameters.salt1,
        &parameters.salt2,
    ));
    let x = Zeroizing::new(BigUint::from_bytes_be(&password_hash[..]));

    let mut exponent_bytes = Zeroizing::new([0_u8; 256]);
    fill_random(&mut exponent_bytes[..])?;
    let exponent = Zeroizing::new(BigUint::from_bytes_be(&exponent_bytes[..]));
    if *exponent == BigUint::from(0_u8) {
        return Err(SrpError::InvalidRandomExponent);
    }
    let client_a = generator.modpow(&exponent, &prime);
    let client_a_padded = padded_256(&client_a);

    let u = BigUint::from_bytes_be(&sha256_parts(&[&client_a_padded, &server_b_padded]));
    let k = BigUint::from_bytes_be(&sha256_parts(&[&parameters.prime, &generator_padded]));
    let verifier = Zeroizing::new(generator.modpow(&x, &prime));
    let kv = Zeroizing::new((&k * &*verifier) % &prime);
    let base = Zeroizing::new((&server_b + &prime - &*kv) % &prime);
    let power = Zeroizing::new(&*exponent + (&u * &*x));
    let shared_secret = Zeroizing::new(base.modpow(&power, &prime));
    let shared_padded = Zeroizing::new(padded_256(&shared_secret));
    let session_key = Zeroizing::new(sha256_parts(&[&shared_padded[..]]));

    let mut prime_xor_generator = sha256_parts(&[&parameters.prime]);
    let generator_hash = sha256_parts(&[&generator_padded]);
    for (left, right) in prime_xor_generator.iter_mut().zip(generator_hash) {
        *left ^= right;
    }
    let salt1_hash = sha256_parts(&[&parameters.salt1]);
    let salt2_hash = sha256_parts(&[&parameters.salt2]);
    let m1 = sha256_parts(&[
        &prime_xor_generator,
        &salt1_hash,
        &salt2_hash,
        &client_a_padded,
        &server_b_padded,
        &session_key[..],
    ]);
    prime_xor_generator.zeroize();

    Ok(PasswordSrpProof {
        srp_id: parameters.srp_id,
        a: client_a_padded,
        m1,
    })
}

fn password_hash(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    let first = Zeroizing::new(sha256_parts(&[salt1, password, salt1]));
    let second = Zeroizing::new(sha256_parts(&[salt2, &first[..], salt2]));
    let mut pbkdf = Zeroizing::new([0_u8; 64]);
    pbkdf2_hmac::<Sha512>(&second[..], salt1, 100_000, &mut pbkdf[..]);
    sha256_parts(&[salt2, &pbkdf[..], salt2])
}

fn validate_prime_generator(prime: &[u8], generator: i32) -> Result<(), SrpError> {
    let expected = hex::decode(KNOWN_DH_PRIME_HEX).expect("known Telegram prime is valid hex");
    if prime != expected {
        return Err(SrpError::InvalidPrimeOrGenerator);
    }
    let prime = BigUint::from_bytes_be(prime);
    let residue = |modulus: u32| {
        (&prime % BigUint::from(modulus))
            .to_bytes_be()
            .last()
            .copied()
            .map(u32::from)
            .unwrap_or(0_u32)
    };
    let valid = match generator {
        2 => residue(8) == 7,
        3 => residue(3) == 2,
        4 => true,
        5 => matches!(residue(5), 1 | 4),
        6 => matches!(residue(24), 19 | 23),
        7 => matches!(residue(7), 3 | 5 | 6),
        _ => false,
    };
    if !valid {
        return Err(SrpError::InvalidPrimeOrGenerator);
    }
    Ok(())
}

fn padded_256(value: &BigUint) -> [u8; 256] {
    let bytes = value.to_bytes_be();
    let mut padded = [0_u8; 256];
    let start = padded.len().saturating_sub(bytes.len());
    padded[start..].copy_from_slice(&bytes[bytes.len().saturating_sub(256)..]);
    padded
}

fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}
