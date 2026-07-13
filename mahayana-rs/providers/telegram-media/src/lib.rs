//! Deterministic, resumable upload/download scheduling shared by every client.
//!
//! Network and file-system adapters perform actual I/O. This crate owns the
//! ordering, progress, pause/resume, retry, concurrency, and integrity rules.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub String);

impl TransferId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn is_valid(&self) -> bool {
        !self.0.trim().is_empty() && self.0.len() <= 128
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaFileId(pub String);

impl MediaFileId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferPriority {
    Background,
    Normal,
    UserInitiated,
    Realtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum TransferState {
    Queued,
    Running,
    Paused,
    Failed { code: String, retryable: bool },
    Completed { sha256: Option<String> },
    Canceled,
}

impl TransferState {
    fn occupies_slot(&self) -> bool {
        matches!(self, Self::Running)
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Canceled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTransfer {
    pub id: TransferId,
    pub file_id: MediaFileId,
    pub direction: TransferDirection,
    pub priority: TransferPriority,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub chunk_size: u32,
    pub expected_sha256: Option<String>,
    pub state: TransferState,
    pub enqueue_order: u64,
    pub attempt: u32,
}

impl MediaTransfer {
    pub fn next_chunk(&self) -> Option<ChunkPlan> {
        if self.state.is_terminal() || self.completed_bytes >= self.total_bytes {
            return None;
        }
        let remaining = self.total_bytes - self.completed_bytes;
        Some(ChunkPlan {
            offset: self.completed_bytes,
            length: remaining.min(u64::from(self.chunk_size)) as u32,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkPlan {
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum TransferCommand {
    Enqueue {
        id: TransferId,
        file_id: MediaFileId,
        direction: TransferDirection,
        priority: TransferPriority,
        total_bytes: u64,
        chunk_size: u32,
        expected_sha256: Option<String>,
    },
    Start {
        id: TransferId,
    },
    RecordChunk {
        id: TransferId,
        offset: u64,
        bytes_written: u32,
    },
    Pause {
        id: TransferId,
    },
    Resume {
        id: TransferId,
    },
    Fail {
        id: TransferId,
        code: String,
        retryable: bool,
    },
    Retry {
        id: TransferId,
    },
    Complete {
        id: TransferId,
        actual_sha256: Option<String>,
    },
    Cancel {
        id: TransferId,
    },
    SetPriority {
        id: TransferId,
        priority: TransferPriority,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum TransferEvent {
    Enqueued {
        transfer: MediaTransfer,
    },
    Started {
        id: TransferId,
    },
    ChunkRecorded {
        id: TransferId,
        completed_bytes: u64,
    },
    Paused {
        id: TransferId,
    },
    Resumed {
        id: TransferId,
    },
    Failed {
        id: TransferId,
        code: String,
        retryable: bool,
    },
    Retried {
        id: TransferId,
        attempt: u32,
    },
    Completed {
        id: TransferId,
        sha256: Option<String>,
    },
    Canceled {
        id: TransferId,
    },
    PriorityChanged {
        id: TransferId,
        priority: TransferPriority,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransferError {
    #[error("transfer id is empty or too long")]
    InvalidTransferId,
    #[error("transfer {0:?} already exists")]
    DuplicateTransfer(TransferId),
    #[error("transfer {0:?} does not exist")]
    TransferNotFound(TransferId),
    #[error("total size and chunk size must both be greater than zero")]
    InvalidSize,
    #[error("SHA-256 must contain exactly 64 hexadecimal characters")]
    InvalidSha256,
    #[error("no transfer slot is available")]
    NoAvailableSlot,
    #[error("command {command} is invalid while transfer is {state:?}")]
    InvalidTransition {
        state: TransferState,
        command: &'static str,
    },
    #[error("expected chunk offset {expected}, received {actual}")]
    UnexpectedChunkOffset { expected: u64, actual: u64 },
    #[error("chunk size must be positive and cannot exceed remaining bytes")]
    InvalidChunkLength,
    #[error("transfer has {completed} of {total} bytes and cannot complete yet")]
    IncompleteTransfer { completed: u64, total: u64 },
    #[error("SHA-256 mismatch: expected {expected}, found {actual}")]
    IntegrityMismatch { expected: String, actual: String },
    #[error("retry attempt counter exhausted")]
    AttemptCounterExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferQueue {
    transfers: BTreeMap<TransferId, MediaTransfer>,
    max_concurrent: usize,
    next_enqueue_order: u64,
}

impl TransferQueue {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            transfers: BTreeMap::new(),
            max_concurrent: max_concurrent.max(1),
            next_enqueue_order: 0,
        }
    }

    pub fn transfers(&self) -> &BTreeMap<TransferId, MediaTransfer> {
        &self.transfers
    }

    pub fn transfer(&self, id: &TransferId) -> Option<&MediaTransfer> {
        self.transfers.get(id)
    }

    pub fn running_count(&self) -> usize {
        self.transfers
            .values()
            .filter(|transfer| transfer.state.occupies_slot())
            .count()
    }

    pub fn next_ready(&self) -> Option<&MediaTransfer> {
        if self.running_count() >= self.max_concurrent {
            return None;
        }
        self.transfers
            .values()
            .filter(|transfer| matches!(transfer.state, TransferState::Queued))
            .max_by_key(|transfer| (transfer.priority, std::cmp::Reverse(transfer.enqueue_order)))
    }

    pub fn execute(
        &mut self,
        command: TransferCommand,
    ) -> Result<Vec<TransferEvent>, TransferError> {
        let events = self.decide(command)?;
        for event in &events {
            self.apply(event.clone());
        }
        Ok(events)
    }

    pub fn decide(&self, command: TransferCommand) -> Result<Vec<TransferEvent>, TransferError> {
        let event = match command {
            TransferCommand::Enqueue {
                id,
                file_id,
                direction,
                priority,
                total_bytes,
                chunk_size,
                expected_sha256,
            } => {
                if !id.is_valid() {
                    return Err(TransferError::InvalidTransferId);
                }
                if self.transfers.contains_key(&id) {
                    return Err(TransferError::DuplicateTransfer(id));
                }
                if total_bytes == 0 || chunk_size == 0 {
                    return Err(TransferError::InvalidSize);
                }
                let expected_sha256 = expected_sha256
                    .map(|value| normalize_sha256(&value))
                    .transpose()?;
                TransferEvent::Enqueued {
                    transfer: MediaTransfer {
                        id,
                        file_id,
                        direction,
                        priority,
                        total_bytes,
                        completed_bytes: 0,
                        chunk_size,
                        expected_sha256,
                        state: TransferState::Queued,
                        enqueue_order: self.next_enqueue_order,
                        attempt: 0,
                    },
                }
            }
            TransferCommand::Start { id } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Queued) {
                    return Err(invalid_transition(&transfer.state, "start"));
                }
                if self.running_count() >= self.max_concurrent {
                    return Err(TransferError::NoAvailableSlot);
                }
                TransferEvent::Started { id }
            }
            TransferCommand::RecordChunk {
                id,
                offset,
                bytes_written,
            } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Running) {
                    return Err(invalid_transition(&transfer.state, "recordChunk"));
                }
                if offset != transfer.completed_bytes {
                    return Err(TransferError::UnexpectedChunkOffset {
                        expected: transfer.completed_bytes,
                        actual: offset,
                    });
                }
                let next = transfer
                    .completed_bytes
                    .checked_add(u64::from(bytes_written))
                    .filter(|next| bytes_written > 0 && *next <= transfer.total_bytes)
                    .ok_or(TransferError::InvalidChunkLength)?;
                TransferEvent::ChunkRecorded {
                    id,
                    completed_bytes: next,
                }
            }
            TransferCommand::Pause { id } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Running) {
                    return Err(invalid_transition(&transfer.state, "pause"));
                }
                TransferEvent::Paused { id }
            }
            TransferCommand::Resume { id } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Paused) {
                    return Err(invalid_transition(&transfer.state, "resume"));
                }
                if self.running_count() >= self.max_concurrent {
                    return Err(TransferError::NoAvailableSlot);
                }
                TransferEvent::Resumed { id }
            }
            TransferCommand::Fail {
                id,
                code,
                retryable,
            } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Running) {
                    return Err(invalid_transition(&transfer.state, "fail"));
                }
                TransferEvent::Failed {
                    id,
                    code,
                    retryable,
                }
            }
            TransferCommand::Retry { id } => {
                let transfer = self.require(&id)?;
                if !matches!(
                    transfer.state,
                    TransferState::Failed {
                        retryable: true,
                        ..
                    }
                ) {
                    return Err(invalid_transition(&transfer.state, "retry"));
                }
                let attempt = transfer
                    .attempt
                    .checked_add(1)
                    .ok_or(TransferError::AttemptCounterExhausted)?;
                TransferEvent::Retried { id, attempt }
            }
            TransferCommand::Complete { id, actual_sha256 } => {
                let transfer = self.require(&id)?;
                if !matches!(transfer.state, TransferState::Running) {
                    return Err(invalid_transition(&transfer.state, "complete"));
                }
                if transfer.completed_bytes != transfer.total_bytes {
                    return Err(TransferError::IncompleteTransfer {
                        completed: transfer.completed_bytes,
                        total: transfer.total_bytes,
                    });
                }
                let actual_sha256 = actual_sha256
                    .map(|value| normalize_sha256(&value))
                    .transpose()?;
                if let Some(expected) = &transfer.expected_sha256 {
                    let actual =
                        actual_sha256
                            .clone()
                            .ok_or_else(|| TransferError::IntegrityMismatch {
                                expected: expected.clone(),
                                actual: "missing".to_string(),
                            })?;
                    if &actual != expected {
                        return Err(TransferError::IntegrityMismatch {
                            expected: expected.clone(),
                            actual,
                        });
                    }
                }
                TransferEvent::Completed {
                    id,
                    sha256: actual_sha256,
                }
            }
            TransferCommand::Cancel { id } => {
                let transfer = self.require(&id)?;
                if transfer.state.is_terminal() {
                    return Err(invalid_transition(&transfer.state, "cancel"));
                }
                TransferEvent::Canceled { id }
            }
            TransferCommand::SetPriority { id, priority } => {
                let transfer = self.require(&id)?;
                if transfer.state.is_terminal() {
                    return Err(invalid_transition(&transfer.state, "setPriority"));
                }
                TransferEvent::PriorityChanged { id, priority }
            }
        };
        Ok(vec![event])
    }

    pub fn apply(&mut self, event: TransferEvent) {
        match event {
            TransferEvent::Enqueued { transfer } => {
                self.next_enqueue_order = self
                    .next_enqueue_order
                    .max(transfer.enqueue_order.saturating_add(1));
                self.transfers.insert(transfer.id.clone(), transfer);
            }
            TransferEvent::Started { id } | TransferEvent::Resumed { id } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.state = TransferState::Running;
                }
            }
            TransferEvent::ChunkRecorded {
                id,
                completed_bytes,
            } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.completed_bytes = completed_bytes;
                }
            }
            TransferEvent::Paused { id } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.state = TransferState::Paused;
                }
            }
            TransferEvent::Failed {
                id,
                code,
                retryable,
            } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.state = TransferState::Failed { code, retryable };
                }
            }
            TransferEvent::Retried { id, attempt } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.attempt = attempt;
                    transfer.state = TransferState::Queued;
                }
            }
            TransferEvent::Completed { id, sha256 } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.state = TransferState::Completed { sha256 };
                }
            }
            TransferEvent::Canceled { id } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.state = TransferState::Canceled;
                }
            }
            TransferEvent::PriorityChanged { id, priority } => {
                if let Some(transfer) = self.transfers.get_mut(&id) {
                    transfer.priority = priority;
                }
            }
        }
    }

    fn require(&self, id: &TransferId) -> Result<&MediaTransfer, TransferError> {
        self.transfers
            .get(id)
            .ok_or_else(|| TransferError::TransferNotFound(id.clone()))
    }
}

fn normalize_sha256(value: &str) -> Result<String, TransferError> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || hex::decode(&value).is_err() {
        return Err(TransferError::InvalidSha256);
    }
    Ok(value)
}

fn invalid_transition(state: &TransferState, command: &'static str) -> TransferError {
    TransferError::InvalidTransition {
        state: state.clone(),
        command,
    }
}
