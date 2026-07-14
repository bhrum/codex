use thiserror::Error;

pub const VECTOR_CONSTRUCTOR: u32 = 0x1cb5_c415;
pub const BOOL_TRUE_CONSTRUCTOR: u32 = 0x9972_75b5;
pub const BOOL_FALSE_CONSTRUCTOR: u32 = 0xbc79_9737;

const SHORT_BYTES_LIMIT: usize = 254;
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_VECTOR_LENGTH: usize = 1_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TlError {
    #[error("TL input ended at byte {offset}; needed {needed} more bytes")]
    UnexpectedEof { offset: usize, needed: usize },
    #[error("TL bytes prefix 255 is invalid")]
    InvalidBytesPrefix,
    #[error("TL byte string length {length} exceeds configured maximum {maximum}")]
    BytesLengthLimit { length: usize, maximum: usize },
    #[error("TL vector length {length} exceeds configured maximum {maximum}")]
    VectorLengthLimit { length: usize, maximum: usize },
    #[error("TL vector length is negative: {0}")]
    NegativeVectorLength(i32),
    #[error("unexpected TL constructor: expected 0x{expected:08x}, found 0x{actual:08x}")]
    UnexpectedConstructor { expected: u32, actual: u32 },
    #[error("0x{0:08x} is not a TL Bool constructor")]
    InvalidBoolConstructor(u32),
    #[error("TL length cannot be represented in the wire format")]
    LengthOutOfRange,
    #[error("TL string contains invalid UTF-8 at byte {offset}")]
    InvalidUtf8 { offset: usize },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TlWriter {
    bytes: Vec<u8>,
}

impl TlWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    pub fn write_i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i128_bytes(&mut self, value: &[u8; 16]) {
        self.bytes.extend_from_slice(value);
    }

    pub fn write_i256_bytes(&mut self, value: &[u8; 32]) {
        self.bytes.extend_from_slice(value);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u32(if value {
            BOOL_TRUE_CONSTRUCTOR
        } else {
            BOOL_FALSE_CONSTRUCTOR
        });
    }

    pub fn write_bytes(&mut self, value: &[u8]) -> Result<(), TlError> {
        let header_length = if value.len() < SHORT_BYTES_LIMIT {
            self.bytes.push(
                value
                    .len()
                    .try_into()
                    .map_err(|_| TlError::LengthOutOfRange)?,
            );
            1
        } else {
            if value.len() > 0x00ff_ffff {
                return Err(TlError::LengthOutOfRange);
            }
            self.bytes.push(254);
            self.bytes.push((value.len() & 0xff) as u8);
            self.bytes.push(((value.len() >> 8) & 0xff) as u8);
            self.bytes.push(((value.len() >> 16) & 0xff) as u8);
            4
        };
        self.bytes.extend_from_slice(value);
        let padding = padding_length(header_length + value.len());
        self.bytes.resize(self.bytes.len() + padding, 0);
        Ok(())
    }

    pub fn write_string(&mut self, value: &str) -> Result<(), TlError> {
        self.write_bytes(value.as_bytes())
    }

    pub fn write_vector_length(&mut self, length: usize) -> Result<(), TlError> {
        let length: i32 = length.try_into().map_err(|_| TlError::LengthOutOfRange)?;
        self.write_u32(VECTOR_CONSTRUCTOR);
        self.write_i32(length);
        Ok(())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug, Clone)]
pub struct TlReader<'a> {
    input: &'a [u8],
    offset: usize,
    max_bytes: usize,
    max_vector_length: usize,
}

impl<'a> TlReader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            offset: 0,
            max_bytes: DEFAULT_MAX_BYTES,
            max_vector_length: DEFAULT_MAX_VECTOR_LENGTH,
        }
    }

    pub fn with_limits(input: &'a [u8], max_bytes: usize, max_vector_length: usize) -> Self {
        Self {
            input,
            offset: 0,
            max_bytes,
            max_vector_length,
        }
    }

    pub fn read_i32(&mut self) -> Result<i32, TlError> {
        Ok(i32::from_le_bytes(self.take_array()?))
    }

    pub fn read_u32(&mut self) -> Result<u32, TlError> {
        Ok(u32::from_le_bytes(self.take_array()?))
    }

    pub fn read_i64(&mut self) -> Result<i64, TlError> {
        Ok(i64::from_le_bytes(self.take_array()?))
    }

    pub fn read_u64(&mut self) -> Result<u64, TlError> {
        Ok(u64::from_le_bytes(self.take_array()?))
    }

    pub fn read_f64(&mut self) -> Result<f64, TlError> {
        Ok(f64::from_le_bytes(self.take_array()?))
    }

    pub fn read_i128_bytes(&mut self) -> Result<[u8; 16], TlError> {
        self.take_array()
    }

    pub fn read_i256_bytes(&mut self) -> Result<[u8; 32], TlError> {
        self.take_array()
    }

    pub fn read_bool(&mut self) -> Result<bool, TlError> {
        match self.read_u32()? {
            BOOL_TRUE_CONSTRUCTOR => Ok(true),
            BOOL_FALSE_CONSTRUCTOR => Ok(false),
            constructor => Err(TlError::InvalidBoolConstructor(constructor)),
        }
    }

    pub fn read_bytes(&mut self) -> Result<&'a [u8], TlError> {
        let prefix = self.take(1)?[0];
        let (length, header_length) = match prefix {
            0..=253 => (usize::from(prefix), 1),
            254 => {
                let length_bytes = self.take(3)?;
                (
                    usize::from(length_bytes[0])
                        | (usize::from(length_bytes[1]) << 8)
                        | (usize::from(length_bytes[2]) << 16),
                    4,
                )
            }
            255 => return Err(TlError::InvalidBytesPrefix),
        };
        if length > self.max_bytes {
            return Err(TlError::BytesLengthLimit {
                length,
                maximum: self.max_bytes,
            });
        }
        let value = self.take(length)?;
        self.take(padding_length(header_length + length))?;
        Ok(value)
    }

    pub fn read_string(&mut self) -> Result<&'a str, TlError> {
        let start = self.offset;
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes).map_err(|_| TlError::InvalidUtf8 { offset: start })
    }

    pub fn read_vector_length(&mut self) -> Result<usize, TlError> {
        let constructor = self.read_u32()?;
        if constructor != VECTOR_CONSTRUCTOR {
            return Err(TlError::UnexpectedConstructor {
                expected: VECTOR_CONSTRUCTOR,
                actual: constructor,
            });
        }
        let length = self.read_i32()?;
        if length < 0 {
            return Err(TlError::NegativeVectorLength(length));
        }
        let length = length as usize;
        if length > self.max_vector_length {
            return Err(TlError::VectorLengthLimit {
                length,
                maximum: self.max_vector_length,
            });
        }
        Ok(length)
    }

    pub fn position(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }

    pub fn is_finished(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], TlError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(TlError::UnexpectedEof {
                offset: self.offset,
                needed: length,
            })?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or(TlError::UnexpectedEof {
                offset: self.offset,
                needed: end.saturating_sub(self.input.len()),
            })?;
        self.offset = end;
        Ok(value)
    }

    fn take_array<const LENGTH: usize>(&mut self) -> Result<[u8; LENGTH], TlError> {
        self.take(LENGTH)?
            .try_into()
            .map_err(|_| TlError::UnexpectedEof {
                offset: self.offset,
                needed: LENGTH,
            })
    }
}

fn padding_length(length: usize) -> usize {
    (4 - (length % 4)) % 4
}
