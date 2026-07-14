use crate::StorageError;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::aead::KeyInit;
use chacha20poly1305::aead::Payload;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::XNonce;
use hmac::Hmac;
use hmac::Mac;
use rand_core::OsRng;
use rand_core::RngCore;
use sha2::Sha256;
use zeroize::Zeroize;
use zeroize::ZeroizeOnDrop;

const PAYLOAD_VERSION: u8 = 1;
const NONCE_LENGTH: usize = 24;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct StorageKey([u8; 32]);

impl StorageKey {
    pub fn from_slice(value: &[u8]) -> Result<Self, StorageError> {
        let bytes: [u8; 32] = value
            .try_into()
            .map_err(|_| StorageError::InvalidKeyLength)?;
        Ok(Self(bytes))
    }

    pub fn expose_for_platform_bridge(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedPayload(pub Vec<u8>);

#[derive(Clone)]
pub struct StorageCipher {
    key: StorageKey,
}

impl StorageCipher {
    pub fn new(key: StorageKey) -> Self {
        Self { key }
    }

    pub fn encrypt(
        &self,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<EncryptedPayload, StorageError> {
        let cipher = XChaCha20Poly1305::new((&self.key.0).into());
        let mut nonce_bytes = [0_u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| StorageError::DecryptionFailed)?;
        let mut output = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
        output.push(PAYLOAD_VERSION);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(EncryptedPayload(output))
    }

    pub(crate) fn blind_index_token(&self, namespace: &[u8], token: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key.0)
            .expect("HMAC-SHA256 accepts a key of any size");
        mac.update(namespace);
        mac.update(&[0]);
        mac.update(token);
        mac.finalize().into_bytes().into()
    }

    pub fn decrypt(
        &self,
        payload: &EncryptedPayload,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, StorageError> {
        let Some((&version, rest)) = payload.0.split_first() else {
            return Err(StorageError::DecryptionFailed);
        };
        if version != PAYLOAD_VERSION {
            return Err(StorageError::UnsupportedPayloadVersion(version));
        }
        if rest.len() <= NONCE_LENGTH {
            return Err(StorageError::DecryptionFailed);
        }
        let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LENGTH);
        let cipher = XChaCha20Poly1305::new((&self.key.0).into());
        cipher
            .decrypt(
                XNonce::from_slice(nonce_bytes),
                Payload {
                    msg: ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| StorageError::DecryptionFailed)
    }
}
