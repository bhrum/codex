//! Native MTProto network transport.
//!
//! This crate owns sockets and connection retry policy. Wire framing,
//! cryptography, and authorization state remain in `telegram-protocol` so the
//! same protocol implementation can be reused by native and Web transports.

use std::io::Read;
use std::io::Write;
use std::io::{self};
use std::net::Shutdown;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use fabushi_telegram_protocol::decrypt_message;
use fabushi_telegram_protocol::encrypt_message;
use fabushi_telegram_protocol::AuthKey;
use fabushi_telegram_protocol::AuthKeyHandshake;
use fabushi_telegram_protocol::CryptoDirection;
use fabushi_telegram_protocol::CryptoError;
use fabushi_telegram_protocol::DcDirectory;
use fabushi_telegram_protocol::DcError;
use fabushi_telegram_protocol::DcPurpose;
use fabushi_telegram_protocol::DhGenAction;
use fabushi_telegram_protocol::EncryptedEnvelope;
use fabushi_telegram_protocol::HandshakeError;
use fabushi_telegram_protocol::MessageIdError;
use fabushi_telegram_protocol::MessageIdGuard;
use fabushi_telegram_protocol::PlainMessage;
use fabushi_telegram_protocol::PlaintextEnvelope;
use fabushi_telegram_protocol::SequenceError;
use fabushi_telegram_protocol::SessionSequence;
use fabushi_telegram_protocol::TransportError;
use fabushi_telegram_protocol::TransportFrameCodec;
use fabushi_telegram_protocol::TransportMode;
use thiserror::Error;
use zeroize::Zeroizing;

const DEFAULT_READ_CHUNK_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_DH_RETRIES: usize = 3;
const DEFAULT_MAX_SERVICE_MESSAGES: usize = 8;
const PING_CONSTRUCTOR: u32 = 0x7abe_77ec;
const PONG_CONSTRUCTOR: u32 = 0x3477_73c5;
const NEW_SESSION_CREATED_CONSTRUCTOR: u32 = 0x9ec2_0908;
const MESSAGE_CONTAINER_CONSTRUCTOR: u32 = 0x73f1_f8dc;
const BAD_SERVER_SALT_CONSTRUCTOR: u32 = 0xedab_447b;
const BAD_MSG_NOTIFICATION_CONSTRUCTOR: u32 = 0xa7ef_f811;
const MSGS_ACK_CONSTRUCTOR: u32 = 0x62d6_b459;
const RPC_RESULT_CONSTRUCTOR: u32 = 0xf35c_6d01;
const FUTURE_SALTS_CONSTRUCTOR: u32 = 0xae50_0895;
const DEFAULT_MAX_RPC_MESSAGES: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct NetworkConfig {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub transport_mode: TransportMode,
    pub prefer_ipv6: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(15),
            write_timeout: Duration::from_secs(15),
            transport_mode: TransportMode::Abridged,
            prefer_ipv6: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedEndpoint {
    pub dc_id: i32,
    pub address: SocketAddr,
}

pub struct EstablishedSession {
    pub endpoint: ConnectedEndpoint,
    pub transport: TcpTransport,
    pub auth_key: AuthKey,
    pub server_salt: i64,
    pub server_time: i32,
    message_ids: MessageIdGuard,
    sequence: SessionSequence,
    session_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingResult {
    pub ping_id: i64,
    pub response_message_id: i64,
    pub server_salt: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcResult {
    pub request_message_id: i64,
    pub response_message_id: i64,
    pub body: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("Telegram data center routing failed: {0}")]
    Directory(#[from] DcError),
    #[error("all Telegram data center connection attempts failed: {attempts:?}")]
    ConnectFailed { attempts: Vec<String> },
    #[error("Telegram TCP transport I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("Telegram transport frame failed: {0}")]
    Transport(#[from] TransportError),
    #[error("Telegram auth-key handshake failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("Telegram message id failed validation: {0}")]
    MessageId(#[from] MessageIdError),
    #[error("Telegram session sequence failed: {0}")]
    Sequence(#[from] SequenceError),
    #[error("Telegram encrypted message failed: {0}")]
    Crypto(#[from] CryptoError),
    #[error("system clock is earlier than the Unix epoch")]
    InvalidSystemClock,
    #[error("Telegram closed the TCP connection")]
    ConnectionClosed,
    #[error("Telegram returned too many dh_gen_retry responses")]
    DhRetryLimit,
    #[error("operating system secure randomness is unavailable")]
    RandomnessUnavailable,
    #[error("Telegram service message is malformed")]
    InvalidServiceMessage,
    #[error("Telegram pong does not match the sent ping")]
    PingMismatch,
    #[error("Telegram did not return a pong before the service-message limit")]
    PongTimeout,
    #[error("Telegram rejected encrypted message {message_id} with error code {code}")]
    BadServerMessage { message_id: i64, code: i32 },
    #[error("Telegram did not return an RPC result before the message limit")]
    RpcTimeout,
}

pub struct TcpTransport {
    stream: TcpStream,
    codec: TransportFrameCodec,
    read_buffer: Vec<u8>,
    initialized: bool,
}

impl TcpTransport {
    pub fn connect(
        address: SocketAddr,
        mode: TransportMode,
        config: &NetworkConfig,
    ) -> Result<Self, NetworkError> {
        let stream = TcpStream::connect_timeout(&address, config.connect_timeout)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(config.read_timeout))?;
        stream.set_write_timeout(Some(config.write_timeout))?;
        Ok(Self::from_stream(stream, mode))
    }

    pub fn from_stream(stream: TcpStream, mode: TransportMode) -> Self {
        Self {
            stream,
            codec: TransportFrameCodec::new(mode),
            read_buffer: Vec::with_capacity(DEFAULT_READ_CHUNK_BYTES),
            initialized: false,
        }
    }

    pub fn send_payload(&mut self, payload: &[u8]) -> Result<(), NetworkError> {
        if !self.initialized {
            self.stream.write_all(self.codec.initial_header())?;
            self.initialized = true;
        }
        let frame = self.codec.encode(payload)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    pub fn receive_payload(&mut self) -> Result<Vec<u8>, NetworkError> {
        loop {
            if let Some(frame) = self.codec.decode(&self.read_buffer)? {
                self.read_buffer.drain(..frame.consumed_bytes);
                if frame.quick_ack_token.is_some() {
                    continue;
                }
                return Ok(frame.payload);
            }

            let mut chunk = [0_u8; DEFAULT_READ_CHUNK_BYTES];
            let count = self.stream.read(&mut chunk)?;
            if count == 0 {
                return Err(NetworkError::ConnectionClosed);
            }
            self.read_buffer.extend_from_slice(&chunk[..count]);
        }
    }

    pub fn exchange_plaintext(
        &mut self,
        request: &PlaintextEnvelope,
    ) -> Result<PlaintextEnvelope, NetworkError> {
        self.send_payload(&request.encode()?)?;
        PlaintextEnvelope::decode(&self.receive_payload()?).map_err(NetworkError::from)
    }

    pub fn shutdown(&self) -> Result<(), NetworkError> {
        self.stream.shutdown(Shutdown::Both)?;
        Ok(())
    }
}

impl EstablishedSession {
    pub fn invoke_raw(&mut self, body: &[u8]) -> Result<RpcResult, NetworkError> {
        if body.is_empty() || !body.len().is_multiple_of(4) {
            return Err(NetworkError::InvalidServiceMessage);
        }
        for attempt in 0..2 {
            let request_message_id = self.send_encrypted_message(body, true)?;
            let mut retry_with_new_salt = false;
            for _ in 0..DEFAULT_MAX_RPC_MESSAGES {
                let payload = self.transport.receive_payload()?;
                let envelope = EncryptedEnvelope::from_bytes(&payload)?;
                let response = decrypt_message(
                    &self.auth_key,
                    CryptoDirection::ServerToClient,
                    &envelope,
                    self.session_id,
                )?;
                let (now, _) = unix_time()?;
                self.message_ids
                    .validate_server_message_id(response.message_id, now)?;
                let mut acknowledgements = Vec::new();
                if response.sequence_number.rem_euclid(2) == 1 {
                    acknowledgements.push(response.message_id);
                }
                let outcome = self.scan_rpc_body(
                    &response.body,
                    request_message_id,
                    response.message_id,
                    &mut acknowledgements,
                )?;
                if !acknowledgements.is_empty() {
                    self.send_acknowledgements(&acknowledgements)?;
                }
                match outcome {
                    RpcOutcome::Result(result) => return Ok(result),
                    RpcOutcome::NewSalt => {
                        retry_with_new_salt = true;
                        break;
                    }
                    RpcOutcome::Continue => {}
                }
            }
            if !(retry_with_new_salt && attempt == 0) {
                return Err(NetworkError::RpcTimeout);
            }
        }
        Err(NetworkError::RpcTimeout)
    }

    fn send_encrypted_message(
        &mut self,
        body: &[u8],
        content_related: bool,
    ) -> Result<i64, NetworkError> {
        let message_id = next_message_id(&mut self.message_ids)?;
        let mut padding = vec![0_u8; encrypted_padding_length(body.len())];
        secure_random(&mut padding)?;
        let message = PlainMessage {
            server_salt: self.server_salt,
            session_id: self.session_id,
            message_id,
            sequence_number: self.sequence.next(content_related)?,
            body: body.to_vec(),
            padding,
        };
        let envelope = encrypt_message(&self.auth_key, CryptoDirection::ClientToServer, &message)?;
        self.transport.send_payload(&envelope.to_bytes())?;
        Ok(message_id)
    }

    fn send_acknowledgements(&mut self, message_ids: &[i64]) -> Result<(), NetworkError> {
        let body = fabushi_telegram_protocol::build_msgs_ack(message_ids)
            .map_err(|_| NetworkError::InvalidServiceMessage)?;
        self.send_encrypted_message(&body, false)?;
        Ok(())
    }

    fn scan_rpc_body(
        &mut self,
        body: &[u8],
        request_message_id: i64,
        response_message_id: i64,
        acknowledgements: &mut Vec<i64>,
    ) -> Result<RpcOutcome, NetworkError> {
        match read_u32_at(body, 0)? {
            RPC_RESULT_CONSTRUCTOR => {
                let request_id = read_i64_at(body, 4)?;
                if request_id != request_message_id {
                    return Ok(RpcOutcome::Continue);
                }
                let result_body = body
                    .get(12..)
                    .filter(|value| !value.is_empty())
                    .ok_or(NetworkError::InvalidServiceMessage)?;
                Ok(RpcOutcome::Result(RpcResult {
                    request_message_id,
                    response_message_id,
                    body: result_body.to_vec(),
                }))
            }
            FUTURE_SALTS_CONSTRUCTOR => {
                let request_id = read_i64_at(body, 4)?;
                if request_id != request_message_id {
                    return Ok(RpcOutcome::Continue);
                }
                Ok(RpcOutcome::Result(RpcResult {
                    request_message_id,
                    response_message_id,
                    body: body.to_vec(),
                }))
            }
            NEW_SESSION_CREATED_CONSTRUCTOR => {
                self.server_salt = read_i64_at(body, 20)?;
                Ok(RpcOutcome::Continue)
            }
            BAD_SERVER_SALT_CONSTRUCTOR => {
                let message_id = read_i64_at(body, 4)?;
                let error_code = read_i32_at(body, 16)?;
                if message_id != request_message_id {
                    return Err(NetworkError::BadServerMessage {
                        message_id,
                        code: error_code,
                    });
                }
                self.server_salt = read_i64_at(body, 20)?;
                Ok(RpcOutcome::NewSalt)
            }
            BAD_MSG_NOTIFICATION_CONSTRUCTOR => Err(NetworkError::BadServerMessage {
                message_id: read_i64_at(body, 4)?,
                code: read_i32_at(body, 16)?,
            }),
            MESSAGE_CONTAINER_CONSTRUCTOR => {
                let count = read_i32_at(body, 4)?;
                if !(0..=1024).contains(&count) {
                    return Err(NetworkError::InvalidServiceMessage);
                }
                let mut offset = 8_usize;
                let mut outcome = RpcOutcome::Continue;
                for _ in 0..count {
                    let message_id = read_i64_at(body, offset)?;
                    let sequence_number = read_i32_at(body, offset + 8)?;
                    let body_length = read_i32_at(body, offset + 12)?;
                    if body_length < 0 {
                        return Err(NetworkError::InvalidServiceMessage);
                    }
                    let body_start = offset
                        .checked_add(16)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let body_end = body_start
                        .checked_add(body_length as usize)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let inner = body
                        .get(body_start..body_end)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let (now, _) = unix_time()?;
                    self.message_ids
                        .validate_server_message_id(message_id, now)?;
                    if sequence_number.rem_euclid(2) == 1 {
                        acknowledgements.push(message_id);
                    }
                    let inner_outcome = self.scan_rpc_body(
                        inner,
                        request_message_id,
                        message_id,
                        acknowledgements,
                    )?;
                    if !matches!(inner_outcome, RpcOutcome::Continue) {
                        outcome = inner_outcome;
                    }
                    offset = body_end;
                }
                if offset != body.len() {
                    return Err(NetworkError::InvalidServiceMessage);
                }
                Ok(outcome)
            }
            _ => Ok(RpcOutcome::Continue),
        }
    }

    pub fn ping(&mut self) -> Result<PingResult, NetworkError> {
        let mut ping_bytes = [0_u8; 8];
        secure_random(&mut ping_bytes)?;
        self.send_ping(i64::from_le_bytes(ping_bytes))
    }

    pub fn send_ping(&mut self, ping_id: i64) -> Result<PingResult, NetworkError> {
        for attempt in 0..2 {
            let request_message_id = next_message_id(&mut self.message_ids)?;
            let mut body = Vec::with_capacity(12);
            body.extend_from_slice(&PING_CONSTRUCTOR.to_le_bytes());
            body.extend_from_slice(&ping_id.to_le_bytes());
            let mut padding = vec![0_u8; encrypted_padding_length(body.len())];
            secure_random(&mut padding)?;
            let message = PlainMessage {
                server_salt: self.server_salt,
                session_id: self.session_id,
                message_id: request_message_id,
                sequence_number: self.sequence.next(false)?,
                body,
                padding,
            };
            let envelope =
                encrypt_message(&self.auth_key, CryptoDirection::ClientToServer, &message)?;
            self.transport.send_payload(&envelope.to_bytes())?;

            let mut should_retry_with_new_salt = false;
            for _ in 0..DEFAULT_MAX_SERVICE_MESSAGES {
                let payload = self.transport.receive_payload()?;
                let envelope = EncryptedEnvelope::from_bytes(&payload)?;
                let response = decrypt_message(
                    &self.auth_key,
                    CryptoDirection::ServerToClient,
                    &envelope,
                    self.session_id,
                )?;
                let (now, _) = unix_time()?;
                self.message_ids
                    .validate_server_message_id(response.message_id, now)?;
                match self.process_service_body(&response.body, ping_id, request_message_id)? {
                    ServiceOutcome::Pong(response_message_id) => {
                        return Ok(PingResult {
                            ping_id,
                            response_message_id,
                            server_salt: self.server_salt,
                        });
                    }
                    ServiceOutcome::NewSalt => should_retry_with_new_salt = true,
                    ServiceOutcome::Continue => {}
                }
            }
            if !(should_retry_with_new_salt && attempt == 0) {
                return Err(NetworkError::PongTimeout);
            }
        }
        Err(NetworkError::PongTimeout)
    }

    fn process_service_body(
        &mut self,
        body: &[u8],
        ping_id: i64,
        request_message_id: i64,
    ) -> Result<ServiceOutcome, NetworkError> {
        match read_u32_at(body, 0)? {
            PONG_CONSTRUCTOR => {
                let response_message_id = read_i64_at(body, 4)?;
                let response_ping_id = read_i64_at(body, 12)?;
                if response_message_id != request_message_id || response_ping_id != ping_id {
                    return Err(NetworkError::PingMismatch);
                }
                Ok(ServiceOutcome::Pong(response_message_id))
            }
            NEW_SESSION_CREATED_CONSTRUCTOR => {
                self.server_salt = read_i64_at(body, 20)?;
                Ok(ServiceOutcome::Continue)
            }
            BAD_SERVER_SALT_CONSTRUCTOR => {
                let message_id = read_i64_at(body, 4)?;
                let error_code = read_i32_at(body, 16)?;
                if message_id != request_message_id {
                    return Err(NetworkError::BadServerMessage {
                        message_id,
                        code: error_code,
                    });
                }
                self.server_salt = read_i64_at(body, 20)?;
                Ok(ServiceOutcome::NewSalt)
            }
            MESSAGE_CONTAINER_CONSTRUCTOR => {
                let count = read_i32_at(body, 4)?;
                if !(0..=1024).contains(&count) {
                    return Err(NetworkError::InvalidServiceMessage);
                }
                let mut offset = 8_usize;
                let mut outcome = ServiceOutcome::Continue;
                for _ in 0..count {
                    let message_id = read_i64_at(body, offset)?;
                    let body_length = read_i32_at(body, offset + 12)?;
                    if body_length < 0 {
                        return Err(NetworkError::InvalidServiceMessage);
                    }
                    let body_start = offset
                        .checked_add(16)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let body_end = body_start
                        .checked_add(body_length as usize)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let inner = body
                        .get(body_start..body_end)
                        .ok_or(NetworkError::InvalidServiceMessage)?;
                    let (now, _) = unix_time()?;
                    self.message_ids
                        .validate_server_message_id(message_id, now)?;
                    let inner_outcome =
                        self.process_service_body(inner, ping_id, request_message_id)?;
                    if !matches!(inner_outcome, ServiceOutcome::Continue) {
                        outcome = inner_outcome;
                    }
                    offset = body_end;
                }
                if offset != body.len() {
                    return Err(NetworkError::InvalidServiceMessage);
                }
                Ok(outcome)
            }
            MSGS_ACK_CONSTRUCTOR => Ok(ServiceOutcome::Continue),
            _ => Ok(ServiceOutcome::Continue),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceOutcome {
    Pong(i64),
    NewSalt,
    Continue,
}

enum RpcOutcome {
    Result(RpcResult),
    NewSalt,
    Continue,
}

pub fn connect_directory(
    directory: &DcDirectory,
    dc_id: i32,
    purpose: DcPurpose,
    config: &NetworkConfig,
) -> Result<(ConnectedEndpoint, TcpTransport), NetworkError> {
    let candidates = directory.candidates(dc_id, purpose, config.prefer_ipv6)?;
    let mut attempts = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let address = SocketAddr::new(candidate.ip_address, candidate.port);
        match TcpTransport::connect(address, config.transport_mode, config) {
            Ok(transport) => {
                return Ok((ConnectedEndpoint { dc_id, address }, transport));
            }
            Err(error) => attempts.push(format!("{address}: {error}")),
        }
    }
    Err(NetworkError::ConnectFailed { attempts })
}

pub fn establish_auth_key(
    directory: &DcDirectory,
    dc_id: i32,
    config: &NetworkConfig,
) -> Result<EstablishedSession, NetworkError> {
    let (endpoint, mut transport) = connect_directory(directory, dc_id, DcPurpose::Main, config)?;
    let mut nonce = Zeroizing::new([0_u8; 16]);
    getrandom::getrandom(&mut nonce[..]).map_err(|_| NetworkError::RandomnessUnavailable)?;
    let mut handshake = AuthKeyHandshake::new(*nonce);
    let mut message_ids = MessageIdGuard::default();

    let request = handshake.begin(next_message_id(&mut message_ids)?)?;
    let response = exchange_and_validate(&mut transport, &request, &mut message_ids)?;
    handshake.receive_res_pq(response)?;

    let request = handshake.prepare_req_dh_params(
        next_message_id(&mut message_ids)?,
        dc_id,
        directory.test_mode(),
    )?;
    let response = exchange_and_validate(&mut transport, &request, &mut message_ids)?;
    let request = handshake.receive_server_dh(response, next_message_id(&mut message_ids)?)?;

    let response = exchange_and_validate(&mut transport, &request, &mut message_ids)?;
    match handshake.receive_dh_gen(response, next_message_id(&mut message_ids)?)? {
        DhGenAction::Established(established) => Ok(EstablishedSession {
            endpoint,
            transport,
            auth_key: established.auth_key,
            server_salt: established.server_salt,
            server_time: established.server_time,
            message_ids,
            sequence: SessionSequence::new(),
            session_id: random_i64()?,
        }),
        DhGenAction::Retry(retry) => {
            continue_dh_retries(endpoint, transport, &mut handshake, &mut message_ids, retry)
        }
    }
}

fn continue_dh_retries(
    endpoint: ConnectedEndpoint,
    mut transport: TcpTransport,
    handshake: &mut AuthKeyHandshake,
    message_ids: &mut MessageIdGuard,
    mut request: PlaintextEnvelope,
) -> Result<EstablishedSession, NetworkError> {
    for _ in 0..DEFAULT_MAX_DH_RETRIES {
        let response = exchange_and_validate(&mut transport, &request, message_ids)?;
        match handshake.receive_dh_gen(response, next_message_id(message_ids)?)? {
            DhGenAction::Established(established) => {
                return Ok(EstablishedSession {
                    endpoint,
                    transport,
                    auth_key: established.auth_key,
                    server_salt: established.server_salt,
                    server_time: established.server_time,
                    message_ids: message_ids.clone(),
                    sequence: SessionSequence::new(),
                    session_id: random_i64()?,
                });
            }
            DhGenAction::Retry(retry) => request = retry,
        }
    }
    Err(NetworkError::DhRetryLimit)
}

fn exchange_and_validate(
    transport: &mut TcpTransport,
    request: &PlaintextEnvelope,
    message_ids: &mut MessageIdGuard,
) -> Result<PlaintextEnvelope, NetworkError> {
    let response = transport.exchange_plaintext(request)?;
    let (seconds, _) = unix_time()?;
    message_ids.validate_server_message_id(response.message_id, seconds)?;
    Ok(response)
}

fn next_message_id(message_ids: &mut MessageIdGuard) -> Result<i64, NetworkError> {
    let (seconds, nanos) = unix_time()?;
    Ok(message_ids.generate_client_message_id(seconds, nanos)?)
}

fn unix_time() -> Result<(i64, u32), NetworkError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NetworkError::InvalidSystemClock)?;
    let seconds =
        i64::try_from(duration.as_secs()).map_err(|_| NetworkError::InvalidSystemClock)?;
    Ok((seconds, duration.subsec_nanos()))
}

fn encrypted_padding_length(body_length: usize) -> usize {
    let unpadded = 32 + body_length;
    let minimum = unpadded + 12;
    minimum.next_multiple_of(16) - unpadded
}

fn random_i64() -> Result<i64, NetworkError> {
    let mut bytes = [0_u8; 8];
    secure_random(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

fn secure_random(output: &mut [u8]) -> Result<(), NetworkError> {
    getrandom::getrandom(output).map_err(|_| NetworkError::RandomnessUnavailable)
}

fn read_u32_at(input: &[u8], offset: usize) -> Result<u32, NetworkError> {
    Ok(u32::from_le_bytes(
        input
            .get(offset..offset + 4)
            .ok_or(NetworkError::InvalidServiceMessage)?
            .try_into()
            .expect("four-byte slice"),
    ))
}

fn read_i32_at(input: &[u8], offset: usize) -> Result<i32, NetworkError> {
    Ok(i32::from_le_bytes(
        input
            .get(offset..offset + 4)
            .ok_or(NetworkError::InvalidServiceMessage)?
            .try_into()
            .expect("four-byte slice"),
    ))
}

fn read_i64_at(input: &[u8], offset: usize) -> Result<i64, NetworkError> {
    Ok(i64::from_le_bytes(
        input
            .get(offset..offset + 8)
            .ok_or(NetworkError::InvalidServiceMessage)?
            .try_into()
            .expect("eight-byte slice"),
    ))
}
