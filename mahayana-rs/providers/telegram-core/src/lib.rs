//! Platform-neutral messaging domain used by every Fabushi client.
//!
//! The crate intentionally contains no Flutter, Android, Apple, desktop, web,
//! or Telegram transport code. Platform adapters consume the same commands and
//! events, while protocol implementations live behind a separate boundary.

pub mod domain;
pub mod engine;
pub mod feature;

pub use domain::*;
pub use engine::Command;
pub use engine::EngineError;
pub use engine::Event;
pub use engine::TelegramEngine;
pub use engine::TelegramState;
pub use feature::Feature;
pub use feature::FeatureDomain;
pub use feature::MigrationStatus;
pub use feature::Platform;
pub use feature::RustLayer;
pub use feature::FEATURE_CATALOG;
