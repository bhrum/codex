//! Platform-neutral messaging domain used by every Fabushi client.
//!
//! The crate intentionally contains no Flutter, Android, Apple, desktop, web,
//! or Telegram transport code. Platform adapters consume the same commands and
//! events, while protocol implementations live behind a separate boundary.

pub mod domain;
pub mod engine;
pub mod feature;

pub use domain::*;
pub use engine::{Command, EngineError, Event, TelegramEngine, TelegramState};
pub use feature::{Feature, FeatureDomain, MigrationStatus, Platform, RustLayer, FEATURE_CATALOG};
