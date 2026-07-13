use aes::{
    cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit},
    Aes256,
};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

const AUTH_KEY_LENGTH: usize = 256;
const AES_BLOCK_LENGTH: usize = 16;
const MIN_PADDING_LENGTH: usize = 12;
const MAX_PADDING_LENGTH: usize = 1_024;
const INTERNAL_HEADER_LENGTH: usize = 32;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AuthKey([u8; AUTH_KEY_LENGTH]);

impl AuthKey {
    pub fn from_slice(value: &[u8]) -> Result<Self, CryptoError> {
        let bytes: [u8; AUTH_KEY_LENGTH] = value
            .try_into()
            .map_err(|_| CryptoError::InvalidAuthKeyLength(value.len()))?;
        Ok(Self(bytes))
    }

    pub fn id(&self) -> u64 {
        let digest = Sha1::digest(self.0);
        u64::from_le_bytes(digest[12..20].try_into().expect("SHA-1 suffix"))
    }

    pub fn as_bytes(&self) -> &[u8; AUTH_KEY_LENGTH] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoDirection {
    ClientToServer,
    ServerToClient,
}

impl CryptoDirection {
    fn x(self) -> usize {
        match self {
            Self::ClientToServer => 0,
            Self::ServerToClient => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainMessage {
    pub server_salt: i64,
    pub session_id: i64,
    pub message_id: i64,
    pub sequence_number: i32,
    pub body: Vec<u8>,
    pub padding: Vec<u8>,
}

impl PlainMessage {
    fn encode(&self) -> Result<Vec<u8>, CryptoError> {
        validate_plain_message(self, None, None)?;
        let body_length: i32 = self
            .body
            .len()
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?;
        let mut plaintext =
            Vec::with_capacity(INTERNAL_HEADER_LENGTH + self.body.len() + self.padding.len());
        plaintext.extend_from_slice(&self.server_salt.to_le_bytes());
        plaintext.extend_from_slice(&self.session_id.to_le_bytes());
        plaintext.extend_from_slice(&self.message_id.to_le_bytes());
        plaintext.extend_from_slice(&self.sequence_number.to_le_bytes());
        plaintext.extend_from_slice(&body_length.to_le_bytes());
        plaintext.extend_from_slice(&self.body);
        plaintext.extend_from_slice(&self.padding);
        Ok(plaintext)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    pub auth_key_id: u64,
    pub message_key: [u8; 16],
    pub encrypted_data: Vec<u8>,
}

impl EncryptedEnvelope {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(24 + self.encrypted_data.len());
        output.extend_from_slice(&self.auth_key_id.to_le_bytes());
        output.extend_from_slice(&self.message_key);
        output.extend_from_slice(&self.encrypted_data);
        output
    }

    pub fn from_bytes(value: &[u8]) -> Result<Self, CryptoError> {
        if value.len() < 24 || !(value.len() - 24).is_multiple_of(AES_BLOCK_LENGTH) {
            return Err(CryptoError::InvalidEnvelope);
        }
        Ok(Self {
            auth_key_id: u64::from_le_bytes(
                value[..8]
                    .try_into()
                    .map_err(|_| CryptoError::InvalidEnvelope)?,
            ),
            message_key: value[8..24]
                .try_into()
                .map_err(|_| CryptoError::InvalidEnvelope)?,
            encrypted_data: value[24..].to_vec(),
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CryptoError {
    #[error("MTProto authorization key must contain 256 bytes, found {0}")]
    InvalidAuthKeyLength(usize),
    #[error("MTProto encrypted envelope has an invalid length")]
    InvalidEnvelope,
    #[error("MTProto plaintext is structurally invalid")]
    InvalidPlaintext,
    #[error("MTProto encrypted message authentication failed")]
    AuthenticationFailed,
    #[error("MTProto message belongs to a different session")]
    SessionMismatch,
    #[error("MTProto message id has invalid direction parity")]
    InvalidMessageId,
}

pub fn derive_aes_key_iv(
    auth_key: &AuthKey,
    message_key: &[u8; 16],
    direction: CryptoDirection,
) -> ([u8; 32], [u8; 32]) {
    let x = direction.x();
    let mut sha256_a = Sha256::new();
    sha256_a.update(message_key);
    sha256_a.update(&auth_key.0[x..x + 36]);
    let sha256_a = sha256_a.finalize();

    let mut sha256_b = Sha256::new();
    sha256_b.update(&auth_key.0[40 + x..76 + x]);
    sha256_b.update(message_key);
    let sha256_b = sha256_b.finalize();

    let mut aes_key = [0_u8; 32];
    aes_key[..8].copy_from_slice(&sha256_a[..8]);
    aes_key[8..24].copy_from_slice(&sha256_b[8..24]);
    aes_key[24..].copy_from_slice(&sha256_a[24..32]);

    let mut aes_iv = [0_u8; 32];
    aes_iv[..8].copy_from_slice(&sha256_b[..8]);
    aes_iv[8..24].copy_from_slice(&sha256_a[8..24]);
    aes_iv[24..].copy_from_slice(&sha256_b[24..32]);
    (aes_key, aes_iv)
}

pub fn encrypt_message(
    auth_key: &AuthKey,
    direction: CryptoDirection,
    message: &PlainMessage,
) -> Result<EncryptedEnvelope, CryptoError> {
    let plaintext = message.encode()?;
    let message_key = compute_message_key(auth_key, direction, &plaintext);
    let (aes_key, aes_iv) = derive_aes_key_iv(auth_key, &message_key, direction);
    let encrypted_data = aes_ige_encrypt(&plaintext, &aes_key, &aes_iv)?;
    Ok(EncryptedEnvelope {
        auth_key_id: auth_key.id(),
        message_key,
        encrypted_data,
    })
}

pub fn quick_ack_token(
    auth_key: &AuthKey,
    direction: CryptoDirection,
    message: &PlainMessage,
) -> Result<u32, CryptoError> {
    let plaintext = message.encode()?;
    let digest = compute_message_key_large(auth_key, direction, &plaintext);
    Ok(u32::from_le_bytes(digest[..4].try_into().expect("SHA-256 prefix")) | (1 << 31))
}

pub fn decrypt_message(
    auth_key: &AuthKey,
    direction: CryptoDirection,
    envelope: &EncryptedEnvelope,
    expected_session_id: i64,
) -> Result<PlainMessage, CryptoError> {
    if envelope.auth_key_id != auth_key.id()
        || envelope.encrypted_data.is_empty()
        || !envelope
            .encrypted_data
            .len()
            .is_multiple_of(AES_BLOCK_LENGTH)
    {
        return Err(CryptoError::AuthenticationFailed);
    }
    let (aes_key, aes_iv) = derive_aes_key_iv(auth_key, &envelope.message_key, direction);
    let plaintext = aes_ige_decrypt(&envelope.encrypted_data, &aes_key, &aes_iv)
        .map_err(|_| CryptoError::AuthenticationFailed)?;
    let actual_message_key = compute_message_key(auth_key, direction, &plaintext);
    if !bool::from(actual_message_key.ct_eq(&envelope.message_key)) {
        return Err(CryptoError::AuthenticationFailed);
    }
    let message = decode_plain_message(&plaintext)?;
    validate_plain_message(&message, Some(direction), Some(expected_session_id))?;
    Ok(message)
}

fn compute_message_key(
    auth_key: &AuthKey,
    direction: CryptoDirection,
    plaintext: &[u8],
) -> [u8; 16] {
    let digest = compute_message_key_large(auth_key, direction, plaintext);
    digest[8..24].try_into().expect("SHA-256 middle 128 bits")
}

fn compute_message_key_large(
    auth_key: &AuthKey,
    direction: CryptoDirection,
    plaintext: &[u8],
) -> [u8; 32] {
    let x = direction.x();
    let mut digest = Sha256::new();
    digest.update(&auth_key.0[88 + x..120 + x]);
    digest.update(plaintext);
    digest.finalize().into()
}

fn decode_plain_message(plaintext: &[u8]) -> Result<PlainMessage, CryptoError> {
    if plaintext.len() < INTERNAL_HEADER_LENGTH + MIN_PADDING_LENGTH {
        return Err(CryptoError::InvalidPlaintext);
    }
    let server_salt = read_i64(plaintext, 0)?;
    let session_id = read_i64(plaintext, 8)?;
    let message_id = read_i64(plaintext, 16)?;
    let sequence_number = read_i32(plaintext, 24)?;
    let body_length = read_i32(plaintext, 28)?;
    if body_length < 0 || body_length % 4 != 0 {
        return Err(CryptoError::InvalidPlaintext);
    }
    let body_length = body_length as usize;
    let body_end = INTERNAL_HEADER_LENGTH
        .checked_add(body_length)
        .ok_or(CryptoError::InvalidPlaintext)?;
    let body = plaintext
        .get(INTERNAL_HEADER_LENGTH..body_end)
        .ok_or(CryptoError::InvalidPlaintext)?
        .to_vec();
    let padding = plaintext
        .get(body_end..)
        .ok_or(CryptoError::InvalidPlaintext)?
        .to_vec();
    Ok(PlainMessage {
        server_salt,
        session_id,
        message_id,
        sequence_number,
        body,
        padding,
    })
}

fn validate_plain_message(
    message: &PlainMessage,
    direction: Option<CryptoDirection>,
    expected_session_id: Option<i64>,
) -> Result<(), CryptoError> {
    if message.body.is_empty()
        || !message.body.len().is_multiple_of(4)
        || !(MIN_PADDING_LENGTH..=MAX_PADDING_LENGTH).contains(&message.padding.len())
        || !(INTERNAL_HEADER_LENGTH + message.body.len() + message.padding.len())
            .is_multiple_of(AES_BLOCK_LENGTH)
    {
        return Err(CryptoError::InvalidPlaintext);
    }
    if expected_session_id.is_some_and(|expected| message.session_id != expected) {
        return Err(CryptoError::SessionMismatch);
    }
    if let Some(direction) = direction {
        let valid = match direction {
            CryptoDirection::ClientToServer => message.message_id.rem_euclid(4) == 0,
            CryptoDirection::ServerToClient => message.message_id.rem_euclid(2) == 1,
        };
        if !valid {
            return Err(CryptoError::InvalidMessageId);
        }
    }
    Ok(())
}

pub fn aes_ige_encrypt(
    plaintext: &[u8],
    key: &[u8; 32],
    iv: &[u8; 32],
) -> Result<Vec<u8>, CryptoError> {
    if plaintext.is_empty() || !plaintext.len().is_multiple_of(AES_BLOCK_LENGTH) {
        return Err(CryptoError::InvalidPlaintext);
    }
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut previous_ciphertext: [u8; 16] = iv[..16].try_into().expect("IGE IV half");
    let mut previous_plaintext: [u8; 16] = iv[16..].try_into().expect("IGE IV half");
    let mut output = Vec::with_capacity(plaintext.len());
    for plaintext_block in plaintext.chunks_exact(AES_BLOCK_LENGTH) {
        let mut block = [0_u8; 16];
        xor_into(&mut block, plaintext_block, &previous_ciphertext);
        let mut encrypted = GenericArray::clone_from_slice(&block);
        cipher.encrypt_block(&mut encrypted);
        for (byte, previous) in encrypted.iter_mut().zip(previous_plaintext) {
            *byte ^= previous;
        }
        let ciphertext_block: [u8; 16] = encrypted.into();
        output.extend_from_slice(&ciphertext_block);
        previous_ciphertext = ciphertext_block;
        previous_plaintext.copy_from_slice(plaintext_block);
    }
    Ok(output)
}

pub fn aes_ige_decrypt(
    ciphertext: &[u8],
    key: &[u8; 32],
    iv: &[u8; 32],
) -> Result<Vec<u8>, CryptoError> {
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(AES_BLOCK_LENGTH) {
        return Err(CryptoError::InvalidEnvelope);
    }
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut previous_ciphertext: [u8; 16] = iv[..16].try_into().expect("IGE IV half");
    let mut previous_plaintext: [u8; 16] = iv[16..].try_into().expect("IGE IV half");
    let mut output = Vec::with_capacity(ciphertext.len());
    for ciphertext_block in ciphertext.chunks_exact(AES_BLOCK_LENGTH) {
        let mut block = [0_u8; 16];
        xor_into(&mut block, ciphertext_block, &previous_plaintext);
        let mut decrypted = GenericArray::clone_from_slice(&block);
        cipher.decrypt_block(&mut decrypted);
        for (byte, previous) in decrypted.iter_mut().zip(previous_ciphertext) {
            *byte ^= previous;
        }
        let plaintext_block: [u8; 16] = decrypted.into();
        output.extend_from_slice(&plaintext_block);
        previous_ciphertext.copy_from_slice(ciphertext_block);
        previous_plaintext = plaintext_block;
    }
    Ok(output)
}

fn xor_into(output: &mut [u8; 16], left: &[u8], right: &[u8; 16]) {
    for ((output, left), right) in output.iter_mut().zip(left).zip(right) {
        *output = *left ^ *right;
    }
}

fn read_i64(value: &[u8], offset: usize) -> Result<i64, CryptoError> {
    Ok(i64::from_le_bytes(
        value
            .get(offset..offset + 8)
            .ok_or(CryptoError::InvalidPlaintext)?
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    ))
}

fn read_i32(value: &[u8], offset: usize) -> Result<i32, CryptoError> {
    Ok(i32::from_le_bytes(
        value
            .get(offset..offset + 4)
            .ok_or(CryptoError::InvalidPlaintext)?
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    ))
}
