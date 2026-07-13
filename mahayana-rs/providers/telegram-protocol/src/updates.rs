use crate::tl::{TlError, TlReader, TlWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const UPDATES_GET_STATE_CONSTRUCTOR: u32 = 0xedd4_882a;
pub const UPDATES_GET_DIFFERENCE_CONSTRUCTOR: u32 = 0x19c2_f763;
pub const UPDATES_STATE_CONSTRUCTOR: u32 = 0xa56c_2a3e;
pub const UPDATES_DIFFERENCE_EMPTY_CONSTRUCTOR: u32 = 0x5d75_a138;
pub const UPDATES_DIFFERENCE_TOO_LONG_CONSTRUCTOR: u32 = 0x4afe_8f6d;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub pts: i32,
    pub qts: i32,
    pub date: i32,
    pub seq: i32,
    pub unread_count: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifferenceRequest {
    pub state: UpdateState,
    pub pts_limit: Option<i32>,
    pub pts_total_limit: Option<i32>,
    pub qts_limit: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalDifference {
    Empty { date: i32, seq: i32 },
    TooLong { pts: i32 },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UpdateError {
    #[error("Telegram update limit must be positive")]
    InvalidLimit,
    #[error("unexpected Telegram update constructor 0x{0:08x}")]
    UnexpectedConstructor(u32),
    #[error("Telegram update response contains trailing bytes")]
    TrailingBytes,
    #[error("TL update encoding failed: {0}")]
    Tl(#[from] TlError),
}

pub fn build_updates_get_state() -> Vec<u8> {
    UPDATES_GET_STATE_CONSTRUCTOR.to_le_bytes().to_vec()
}

pub fn build_updates_get_difference(request: DifferenceRequest) -> Result<Vec<u8>, UpdateError> {
    for limit in [
        request.pts_limit,
        request.pts_total_limit,
        request.qts_limit,
    ]
    .into_iter()
    .flatten()
    {
        if limit <= 0 {
            return Err(UpdateError::InvalidLimit);
        }
    }
    let mut flags = 0_i32;
    if request.pts_total_limit.is_some() {
        flags |= 1;
    }
    if request.pts_limit.is_some() {
        flags |= 2;
    }
    if request.qts_limit.is_some() {
        flags |= 4;
    }
    let mut writer = TlWriter::new();
    writer.write_u32(UPDATES_GET_DIFFERENCE_CONSTRUCTOR);
    writer.write_i32(flags);
    writer.write_i32(request.state.pts);
    if let Some(limit) = request.pts_limit {
        writer.write_i32(limit);
    }
    if let Some(limit) = request.pts_total_limit {
        writer.write_i32(limit);
    }
    writer.write_i32(request.state.date);
    writer.write_i32(request.state.qts);
    if let Some(limit) = request.qts_limit {
        writer.write_i32(limit);
    }
    Ok(writer.into_bytes())
}

pub fn parse_update_state(input: &[u8]) -> Result<UpdateState, UpdateError> {
    let mut reader = TlReader::new(input);
    let constructor = reader.read_u32()?;
    if constructor != UPDATES_STATE_CONSTRUCTOR {
        return Err(UpdateError::UnexpectedConstructor(constructor));
    }
    let state = UpdateState {
        pts: reader.read_i32()?,
        qts: reader.read_i32()?,
        date: reader.read_i32()?,
        seq: reader.read_i32()?,
        unread_count: reader.read_i32()?,
    };
    if !reader.is_finished() {
        return Err(UpdateError::TrailingBytes);
    }
    Ok(state)
}

pub fn parse_terminal_difference(input: &[u8]) -> Result<TerminalDifference, UpdateError> {
    let mut reader = TlReader::new(input);
    let difference = match reader.read_u32()? {
        UPDATES_DIFFERENCE_EMPTY_CONSTRUCTOR => TerminalDifference::Empty {
            date: reader.read_i32()?,
            seq: reader.read_i32()?,
        },
        UPDATES_DIFFERENCE_TOO_LONG_CONSTRUCTOR => TerminalDifference::TooLong {
            pts: reader.read_i32()?,
        },
        constructor => return Err(UpdateError::UnexpectedConstructor(constructor)),
    };
    if !reader.is_finished() {
        return Err(UpdateError::TrailingBytes);
    }
    Ok(difference)
}
