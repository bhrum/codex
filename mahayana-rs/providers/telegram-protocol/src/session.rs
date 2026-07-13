use std::collections::BTreeSet;
use thiserror::Error;

const FRACTION_SCALE: i128 = 1_i128 << 32;
const NANOS_PER_SECOND: i128 = 1_000_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MessageIdError {
    #[error("MTProto message id cannot be represented as a signed 64-bit value")]
    OutOfRange,
    #[error("server MTProto message id must be odd")]
    InvalidServerParity,
    #[error("MTProto message id is outside the accepted -300/+30 second time window")]
    OutsideTimeWindow,
    #[error("MTProto message id is a duplicate or older than the replay window")]
    Replay,
    #[error("fractional nanoseconds must be smaller than one second")]
    InvalidFraction,
}

#[derive(Debug, Clone)]
pub struct MessageIdGuard {
    last_client_message_id: Option<i64>,
    recent_server_message_ids: BTreeSet<i64>,
    replay_window_size: usize,
}

impl Default for MessageIdGuard {
    fn default() -> Self {
        Self::new(300)
    }
}

impl MessageIdGuard {
    pub fn new(replay_window_size: usize) -> Self {
        Self {
            last_client_message_id: None,
            recent_server_message_ids: BTreeSet::new(),
            replay_window_size: replay_window_size.max(1),
        }
    }

    pub fn generate_client_message_id(
        &mut self,
        unix_seconds: i64,
        fractional_nanos: u32,
    ) -> Result<i64, MessageIdError> {
        if fractional_nanos >= 1_000_000_000 {
            return Err(MessageIdError::InvalidFraction);
        }
        let mut candidate = i128::from(unix_seconds)
            .checked_mul(FRACTION_SCALE)
            .and_then(|value| {
                value.checked_add(i128::from(fractional_nanos) * FRACTION_SCALE / NANOS_PER_SECOND)
            })
            .ok_or(MessageIdError::OutOfRange)?;
        candidate &= !3_i128;
        if candidate & 0xffff_ffff == 0 {
            candidate = candidate.checked_add(4).ok_or(MessageIdError::OutOfRange)?;
        }
        if let Some(last) = self.last_client_message_id {
            candidate = candidate.max(
                i128::from(last)
                    .checked_add(4)
                    .ok_or(MessageIdError::OutOfRange)?,
            );
        }
        let candidate: i64 = candidate
            .try_into()
            .map_err(|_| MessageIdError::OutOfRange)?;
        self.last_client_message_id = Some(candidate);
        Ok(candidate)
    }

    pub fn validate_server_message_id(
        &mut self,
        message_id: i64,
        now_unix_seconds: i64,
    ) -> Result<(), MessageIdError> {
        if message_id.rem_euclid(2) != 1 {
            return Err(MessageIdError::InvalidServerParity);
        }
        let message_seconds = message_id >> 32;
        if message_seconds < now_unix_seconds.saturating_sub(300)
            || message_seconds > now_unix_seconds.saturating_add(30)
        {
            return Err(MessageIdError::OutsideTimeWindow);
        }
        if self.recent_server_message_ids.contains(&message_id)
            || (self.recent_server_message_ids.len() >= self.replay_window_size
                && self
                    .recent_server_message_ids
                    .first()
                    .is_some_and(|minimum| message_id <= *minimum))
        {
            return Err(MessageIdError::Replay);
        }
        self.recent_server_message_ids.insert(message_id);
        while self.recent_server_message_ids.len() > self.replay_window_size {
            self.recent_server_message_ids.pop_first();
        }
        Ok(())
    }

    pub fn recent_server_message_ids(&self) -> &BTreeSet<i64> {
        &self.recent_server_message_ids
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SequenceError {
    #[error("MTProto content-related sequence counter exhausted")]
    CounterExhausted,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionSequence {
    content_related_count: u32,
}

impl SessionSequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next(&mut self, content_related: bool) -> Result<i32, SequenceError> {
        let base = self
            .content_related_count
            .checked_mul(2)
            .ok_or(SequenceError::CounterExhausted)?;
        let sequence = base
            .checked_add(u32::from(content_related))
            .and_then(|value| i32::try_from(value).ok())
            .ok_or(SequenceError::CounterExhausted)?;
        if content_related {
            self.content_related_count = self
                .content_related_count
                .checked_add(1)
                .ok_or(SequenceError::CounterExhausted)?;
        }
        Ok(sequence)
    }

    pub fn content_related_count(&self) -> u32 {
        self.content_related_count
    }
}
