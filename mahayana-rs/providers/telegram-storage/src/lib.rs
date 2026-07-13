//! Encrypted persistence for the platform-neutral Telegram domain.
//!
//! The database never stores the supplied 256-bit master key. Android Keystore,
//! Apple Keychain, desktop credential vaults, and the web adapter are responsible
//! for key lifecycle and pass the unlocked key into this crate.

mod cipher;
mod sqlite;

pub use cipher::{EncryptedPayload, StorageCipher, StorageKey};
pub use sqlite::{EncryptedSqliteStore, PersistedTransition, StateSnapshot, StoredEvent};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage key must contain exactly 32 bytes")]
    InvalidKeyLength,
    #[error("encrypted payload is invalid or was produced with a different key")]
    DecryptionFailed,
    #[error("encrypted payload version {0} is not supported")]
    UnsupportedPayloadVersion(u8),
    #[error("database schema version {0} is newer than this application supports")]
    UnsupportedSchemaVersion(i64),
    #[error("snapshot revision conflict: expected {expected}, current {current}")]
    RevisionConflict { expected: u64, current: u64 },
    #[error("integer value is outside SQLite range")]
    IntegerOutOfRange,
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
