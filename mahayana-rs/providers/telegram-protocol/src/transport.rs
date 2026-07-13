use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const FULL_FRAME_OVERHEAD: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportMode {
    Full,
    Abridged,
    Intermediate,
    PaddedIntermediate,
}

impl TransportMode {
    pub fn initial_header(self) -> &'static [u8] {
        match self {
            Self::Full => &[],
            Self::Abridged => &[0xef],
            Self::Intermediate => &[0xee, 0xee, 0xee, 0xee],
            Self::PaddedIntermediate => &[0xdd, 0xdd, 0xdd, 0xdd],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub payload: Vec<u8>,
    pub sequence_number: Option<u32>,
    pub quick_ack_token: Option<u32>,
    pub consumed_bytes: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransportError {
    #[error("MTProto transport payload must not be empty")]
    EmptyPayload,
    #[error("abridged MTProto payload length {0} is not divisible by four")]
    AbridgedPayloadAlignment(usize),
    #[error("MTProto transport frame length {length} exceeds maximum {maximum}")]
    FrameLengthLimit { length: usize, maximum: usize },
    #[error("MTProto transport frame length {0} is invalid")]
    InvalidFrameLength(usize),
    #[error("MTProto full transport sequence number exhausted")]
    SequenceExhausted,
    #[error(
        "MTProto full transport CRC mismatch: expected 0x{expected:08x}, found 0x{actual:08x}"
    )]
    CrcMismatch { expected: u32, actual: u32 },
    #[error("MTProto transport length cannot be represented on the wire")]
    LengthOutOfRange,
    #[error("quick ACK is not supported by the full MTProto transport")]
    QuickAckUnsupported,
    #[error("padded intermediate transport padding must contain 0-15 bytes")]
    InvalidPaddingLength,
}

#[derive(Debug, Clone)]
pub struct TransportFrameCodec {
    mode: TransportMode,
    next_sequence_number: u32,
    max_frame_bytes: usize,
}

impl TransportFrameCodec {
    pub fn new(mode: TransportMode) -> Self {
        Self {
            mode,
            next_sequence_number: 0,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    pub fn with_max_frame_bytes(mode: TransportMode, max_frame_bytes: usize) -> Self {
        Self {
            mode,
            next_sequence_number: 0,
            max_frame_bytes,
        }
    }

    pub fn mode(&self) -> TransportMode {
        self.mode
    }

    pub fn initial_header(&self) -> &'static [u8] {
        self.mode.initial_header()
    }

    pub fn encode(&mut self, payload: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.encode_internal(payload, &[], false)
    }

    pub fn encode_with_padding(
        &mut self,
        payload: &[u8],
        padding: &[u8],
    ) -> Result<Vec<u8>, TransportError> {
        self.encode_internal(payload, padding, false)
    }

    pub fn encode_requesting_quick_ack(
        &mut self,
        payload: &[u8],
        padding: &[u8],
    ) -> Result<Vec<u8>, TransportError> {
        self.encode_internal(payload, padding, true)
    }

    fn encode_internal(
        &mut self,
        payload: &[u8],
        padding: &[u8],
        quick_ack: bool,
    ) -> Result<Vec<u8>, TransportError> {
        if payload.is_empty() {
            return Err(TransportError::EmptyPayload);
        }
        let total_payload_length = payload
            .len()
            .checked_add(padding.len())
            .ok_or(TransportError::LengthOutOfRange)?;
        if total_payload_length > self.max_frame_bytes {
            return Err(TransportError::FrameLengthLimit {
                length: total_payload_length,
                maximum: self.max_frame_bytes,
            });
        }
        if self.mode != TransportMode::PaddedIntermediate && !padding.is_empty() {
            return Err(TransportError::InvalidFrameLength(total_payload_length));
        }
        if self.mode == TransportMode::PaddedIntermediate && padding.len() > 15 {
            return Err(TransportError::InvalidPaddingLength);
        }

        match self.mode {
            TransportMode::Full if quick_ack => Err(TransportError::QuickAckUnsupported),
            TransportMode::Full => self.encode_full(payload),
            TransportMode::Abridged => self.encode_abridged(payload, quick_ack),
            TransportMode::Intermediate | TransportMode::PaddedIntermediate => {
                let mut length: u32 = total_payload_length
                    .try_into()
                    .map_err(|_| TransportError::LengthOutOfRange)?;
                if quick_ack {
                    length |= 1 << 31;
                }
                let mut frame = Vec::with_capacity(4 + total_payload_length);
                frame.extend_from_slice(&length.to_le_bytes());
                frame.extend_from_slice(payload);
                frame.extend_from_slice(padding);
                Ok(frame)
            }
        }
    }

    pub fn decode(&self, input: &[u8]) -> Result<Option<DecodedFrame>, TransportError> {
        match self.mode {
            TransportMode::Full => self.decode_full(input),
            TransportMode::Abridged => self.decode_abridged(input),
            TransportMode::Intermediate | TransportMode::PaddedIntermediate => {
                self.decode_intermediate(input)
            }
        }
    }

    fn encode_full(&mut self, payload: &[u8]) -> Result<Vec<u8>, TransportError> {
        let frame_length = payload
            .len()
            .checked_add(FULL_FRAME_OVERHEAD)
            .ok_or(TransportError::LengthOutOfRange)?;
        let frame_length_u32: u32 = frame_length
            .try_into()
            .map_err(|_| TransportError::LengthOutOfRange)?;
        let sequence_number = self.next_sequence_number;
        self.next_sequence_number = self
            .next_sequence_number
            .checked_add(1)
            .ok_or(TransportError::SequenceExhausted)?;
        let mut frame = Vec::with_capacity(frame_length);
        frame.extend_from_slice(&frame_length_u32.to_le_bytes());
        frame.extend_from_slice(&sequence_number.to_le_bytes());
        frame.extend_from_slice(payload);
        let crc = crc32fast::hash(&frame);
        frame.extend_from_slice(&crc.to_le_bytes());
        Ok(frame)
    }

    fn encode_abridged(&self, payload: &[u8], quick_ack: bool) -> Result<Vec<u8>, TransportError> {
        if !payload.len().is_multiple_of(4) {
            return Err(TransportError::AbridgedPayloadAlignment(payload.len()));
        }
        let words = payload.len() / 4;
        if words >= 1 << 24 {
            return Err(TransportError::LengthOutOfRange);
        }
        let mut frame = Vec::with_capacity(payload.len() + 4);
        if words < 127 {
            frame.push((words as u8) | if quick_ack { 0x80 } else { 0 });
        } else {
            frame.push(if quick_ack { 0xff } else { 0x7f });
            frame.push((words & 0xff) as u8);
            frame.push(((words >> 8) & 0xff) as u8);
            frame.push(((words >> 16) & 0xff) as u8);
        }
        frame.extend_from_slice(payload);
        Ok(frame)
    }

    fn decode_full(&self, input: &[u8]) -> Result<Option<DecodedFrame>, TransportError> {
        let Some(length_bytes) = input.get(..4) else {
            return Ok(None);
        };
        let frame_length =
            u32::from_le_bytes(length_bytes.try_into().expect("four bytes")) as usize;
        if frame_length < FULL_FRAME_OVERHEAD {
            return Err(TransportError::InvalidFrameLength(frame_length));
        }
        let payload_length = frame_length - FULL_FRAME_OVERHEAD;
        self.validate_frame_length(payload_length)?;
        let Some(frame) = input.get(..frame_length) else {
            return Ok(None);
        };
        let sequence_number = u32::from_le_bytes(frame[4..8].try_into().expect("four bytes"));
        let crc_offset = frame_length - 4;
        let actual_crc = u32::from_le_bytes(
            frame[crc_offset..frame_length]
                .try_into()
                .expect("four bytes"),
        );
        let mut hasher = Hasher::new();
        hasher.update(&frame[..crc_offset]);
        let expected_crc = hasher.finalize();
        if actual_crc != expected_crc {
            return Err(TransportError::CrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }
        Ok(Some(DecodedFrame {
            payload: frame[8..crc_offset].to_vec(),
            sequence_number: Some(sequence_number),
            quick_ack_token: None,
            consumed_bytes: frame_length,
        }))
    }

    fn decode_abridged(&self, input: &[u8]) -> Result<Option<DecodedFrame>, TransportError> {
        let Some(&prefix) = input.first() else {
            return Ok(None);
        };
        if prefix & 0x80 != 0 {
            let Some(token_bytes) = input.get(..4) else {
                return Ok(None);
            };
            return Ok(Some(DecodedFrame {
                payload: Vec::new(),
                sequence_number: None,
                quick_ack_token: Some(u32::from_be_bytes(
                    token_bytes.try_into().expect("four bytes"),
                )),
                consumed_bytes: 4,
            }));
        }
        let (words, header_length) = if prefix < 0x7f {
            (usize::from(prefix), 1_usize)
        } else {
            let Some(length_bytes) = input.get(1..4) else {
                return Ok(None);
            };
            (
                usize::from(length_bytes[0])
                    | (usize::from(length_bytes[1]) << 8)
                    | (usize::from(length_bytes[2]) << 16),
                4_usize,
            )
        };
        let payload_length = words
            .checked_mul(4)
            .ok_or(TransportError::LengthOutOfRange)?;
        self.validate_frame_length(payload_length)?;
        let consumed_bytes = header_length
            .checked_add(payload_length)
            .ok_or(TransportError::LengthOutOfRange)?;
        let Some(payload) = input.get(header_length..consumed_bytes) else {
            return Ok(None);
        };
        Ok(Some(DecodedFrame {
            payload: payload.to_vec(),
            sequence_number: None,
            quick_ack_token: None,
            consumed_bytes,
        }))
    }

    fn decode_intermediate(&self, input: &[u8]) -> Result<Option<DecodedFrame>, TransportError> {
        let Some(length_bytes) = input.get(..4) else {
            return Ok(None);
        };
        let encoded_length = u32::from_le_bytes(length_bytes.try_into().expect("four bytes"));
        if encoded_length & (1 << 31) != 0 {
            return Ok(Some(DecodedFrame {
                payload: Vec::new(),
                sequence_number: None,
                quick_ack_token: Some(encoded_length),
                consumed_bytes: 4,
            }));
        }
        let payload_length = encoded_length as usize;
        self.validate_frame_length(payload_length)?;
        let consumed_bytes = 4_usize
            .checked_add(payload_length)
            .ok_or(TransportError::LengthOutOfRange)?;
        let Some(payload) = input.get(4..consumed_bytes) else {
            return Ok(None);
        };
        if self.mode == TransportMode::PaddedIntermediate
            && (8..=16).contains(&payload.len())
            && payload[..4] == [0xff; 4]
        {
            return Ok(Some(DecodedFrame {
                payload: Vec::new(),
                sequence_number: None,
                quick_ack_token: Some(u32::from_le_bytes(
                    payload[4..8].try_into().expect("four bytes"),
                )),
                consumed_bytes,
            }));
        }
        Ok(Some(DecodedFrame {
            payload: payload.to_vec(),
            sequence_number: None,
            quick_ack_token: None,
            consumed_bytes,
        }))
    }

    fn validate_frame_length(&self, payload_length: usize) -> Result<(), TransportError> {
        if payload_length == 0 {
            return Err(TransportError::EmptyPayload);
        }
        if payload_length > self.max_frame_bytes {
            return Err(TransportError::FrameLengthLimit {
                length: payload_length,
                maximum: self.max_frame_bytes,
            });
        }
        Ok(())
    }
}
