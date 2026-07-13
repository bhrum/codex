use crate::{
    crypto::{aes_ige_decrypt, aes_ige_encrypt, CryptoError},
    tl::{TlError, TlReader, TlWriter},
};
use num_bigint_dig::BigUint;
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

pub const REQ_PQ_MULTI_CONSTRUCTOR: u32 = 0xbe7e_8ef1;
pub const RES_PQ_CONSTRUCTOR: u32 = 0x0516_2463;
pub const P_Q_INNER_DATA_DC_CONSTRUCTOR: u32 = 0xa9f5_5f95;
pub const REQ_DH_PARAMS_CONSTRUCTOR: u32 = 0xd712_e4be;
pub const SERVER_DH_PARAMS_OK_CONSTRUCTOR: u32 = 0xd0e8_075c;
pub const SERVER_DH_PARAMS_FAIL_CONSTRUCTOR: u32 = 0x79cb_045d;
pub const SERVER_DH_INNER_DATA_CONSTRUCTOR: u32 = 0xb589_0dba;
pub const CLIENT_DH_INNER_DATA_CONSTRUCTOR: u32 = 0x6643_b654;
pub const SET_CLIENT_DH_PARAMS_CONSTRUCTOR: u32 = 0xf504_5f1f;
pub const DH_GEN_OK_CONSTRUCTOR: u32 = 0x3bcb_f734;
pub const DH_GEN_RETRY_CONSTRUCTOR: u32 = 0x46dc_1fb9;
pub const DH_GEN_FAIL_CONSTRUCTOR: u32 = 0xa69d_ae02;
pub const KNOWN_DH_PRIME_HEX: &str = "C71CAEB9C6B1C9048E6C522F70F13F73980D40238E3E21C14934D037563D930F48198A0AA7C14058229493D22530F4DBFA336F6E0AC925139543AED44CCE7C3720FD51F69458705AC68CD4FE6B6B13ABDC9746512969328454F18FAF8C595F642477FE96BB2A941D5BCD1D4AC8CC49880708FA9B378E3C4F3A9060BEE67CF9A4A4A695811051907E162753B56B0F6B410DBA74D8A84B2A14B3144E0EF1284754FD17ED950D5965B4B9DD46582DB1178D169C6BC465B0D6FF9CA3928FEF5B9AE4E418FC15E83EBEA0F87FA9FF5EED70050DED2849F47BF959D956850CE929851F0D8115F635B105EE2E4E15D04B2454BF6F4FADF034B10403119CD8E3B92FCC5B";
pub const TELEGRAM_MAIN_RSA_MODULUS_HEX: &str = "E8BB3305C0B52C6CF2AFDF7637313489E63E05268E5BADB601AF417786472E5F93B85438968E20E6729A301C0AFC121BF7151F834436F7FDA680847A66BF64ACCEC78EE21C0B316F0EDAFE2F41908DA7BD1F4A5107638EEB67040ACE472A14F90D9F7C2B7DEF99688BA3073ADB5750BB02964902A359FE745D8170E36876D4FD8A5D41B2A76CBFF9A13267EB9580B2D06D10357448D20D9DA2191CB5D8C93982961CDFDEDA629E37F1FB09A0722027696032FE61ED663DB7A37F6F263D370F69DB53A0DC0A1748BDAAFF6209D5645485E6E001D1953255757E4B8E42813347B11DA6AB500FD0ACE7E6DFA3736199CCAF9397ED0745A427DCFA6CD67BCB1ACFF3";
pub const TELEGRAM_TEST_RSA_MODULUS_HEX: &str = "C8C11D635691FAC091DD9489AEDCED2932AA8A0BCEFEF05FA800892D9B52ED03200865C9E97211CB2EE6C7AE96D3FB0E15AEFFD66019B44A08A240CFDD2868A85E1F54D6FA5DEAA041F6941DDF302690D61DC476385C2FA655142353CB4E4B59F6E5B6584DB76FE8B1370263246C010C93D011014113EBDF987D093F9D37C2BE48352D69A1683F8F6E6C2167983C761E3AB169FDE5DAAA12123FA1BEAB621E4DA5935E9C198F82F35EAE583A99386D8110EA6BD1ABB0F568759F62694419EA5F69847C43462ABEF858B4CB5EDC84E7B9226CD7BD7E183AA974A712C079DDE85B9DC063B8A5C08E8F859C0EE5DCD824C7807F20153361A7F63CFD2A433A1BE7F5";
pub const DEFAULT_MAX_PLAINTEXT_BODY: usize = 16 * 1024 * 1024;

pub type Nonce = [u8; 16];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaintextEnvelope {
    pub message_id: i64,
    pub body: Vec<u8>,
}

impl PlaintextEnvelope {
    pub fn encode(&self) -> Result<Vec<u8>, HandshakeError> {
        if self.body.len() > DEFAULT_MAX_PLAINTEXT_BODY {
            return Err(HandshakeError::BodyTooLarge {
                length: self.body.len(),
                maximum: DEFAULT_MAX_PLAINTEXT_BODY,
            });
        }
        let body_length: i32 = self
            .body
            .len()
            .try_into()
            .map_err(|_| HandshakeError::InvalidBodyLength(i64::MAX))?;
        let mut encoded = Vec::with_capacity(20 + self.body.len());
        encoded.extend_from_slice(&0_i64.to_le_bytes());
        encoded.extend_from_slice(&self.message_id.to_le_bytes());
        encoded.extend_from_slice(&body_length.to_le_bytes());
        encoded.extend_from_slice(&self.body);
        Ok(encoded)
    }

    pub fn decode(input: &[u8]) -> Result<Self, HandshakeError> {
        Self::decode_with_limit(input, DEFAULT_MAX_PLAINTEXT_BODY)
    }

    pub fn decode_with_limit(input: &[u8], maximum: usize) -> Result<Self, HandshakeError> {
        if input.len() < 20 {
            return Err(HandshakeError::TruncatedEnvelope {
                length: input.len(),
            });
        }
        let auth_key_id = i64::from_le_bytes(input[0..8].try_into().expect("checked length"));
        if auth_key_id != 0 {
            return Err(HandshakeError::UnexpectedAuthKeyId(auth_key_id));
        }
        let message_id = i64::from_le_bytes(input[8..16].try_into().expect("checked length"));
        let raw_length = i32::from_le_bytes(input[16..20].try_into().expect("checked length"));
        if raw_length < 0 {
            return Err(HandshakeError::InvalidBodyLength(i64::from(raw_length)));
        }
        let body_length = raw_length as usize;
        if body_length > maximum {
            return Err(HandshakeError::BodyTooLarge {
                length: body_length,
                maximum,
            });
        }
        let expected = 20_usize
            .checked_add(body_length)
            .ok_or(HandshakeError::InvalidBodyLength(i64::from(raw_length)))?;
        if input.len() != expected {
            return Err(HandshakeError::EnvelopeLengthMismatch {
                declared: body_length,
                actual: input.len().saturating_sub(20),
            });
        }
        Ok(Self {
            message_id,
            body: input[20..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResPq {
    pub nonce: Nonce,
    pub server_nonce: Nonce,
    pub pq: Vec<u8>,
    pub server_public_key_fingerprints: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoredPq {
    pub pq: u64,
    pub p: u64,
    pub q: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    modulus: [u8; 256],
    exponent: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerDhParams {
    Ok { encrypted_answer: Vec<u8> },
    Fail { new_nonce_hash: [u8; 16] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDhInnerData {
    pub nonce: Nonce,
    pub server_nonce: Nonce,
    pub generator: i32,
    pub dh_prime: Vec<u8>,
    pub g_a: Vec<u8>,
    pub server_time: i32,
}

pub struct PreparedClientDh {
    pub request_body: Vec<u8>,
    pub auth_key: crate::crypto::AuthKey,
    pub auth_key_aux_hash: u64,
    pub server_salt: i64,
}

pub struct EstablishedAuthKey {
    pub auth_key: crate::crypto::AuthKey,
    pub server_salt: i64,
    pub server_time: i32,
}

pub enum DhGenAction {
    Established(Box<EstablishedAuthKey>),
    Retry(PlaintextEnvelope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhGenResult {
    Ok,
    Retry,
    Fail,
}

impl RsaPublicKey {
    pub fn from_components(modulus: &[u8], exponent: &[u8]) -> Result<Self, HandshakeError> {
        let modulus: [u8; 256] = modulus
            .try_into()
            .map_err(|_| HandshakeError::InvalidRsaModulusLength(modulus.len()))?;
        if exponent.is_empty() || exponent.iter().all(|byte| *byte == 0) {
            return Err(HandshakeError::InvalidRsaExponent);
        }
        Ok(Self {
            modulus,
            exponent: exponent.to_vec(),
        })
    }

    pub fn modulus(&self) -> &[u8; 256] {
        &self.modulus
    }

    pub fn exponent(&self) -> &[u8] {
        &self.exponent
    }

    pub fn fingerprint(&self) -> Result<u64, HandshakeError> {
        let mut writer = TlWriter::new();
        writer.write_bytes(&self.modulus)?;
        writer.write_bytes(&self.exponent)?;
        let digest = Sha1::digest(writer.as_slice());
        Ok(u64::from_le_bytes(
            digest[12..20].try_into().expect("SHA-1 suffix"),
        ))
    }
}

pub fn telegram_server_rsa_key(test_environment: bool) -> Result<RsaPublicKey, HandshakeError> {
    let modulus = hex::decode(if test_environment {
        TELEGRAM_TEST_RSA_MODULUS_HEX
    } else {
        TELEGRAM_MAIN_RSA_MODULUS_HEX
    })
    .map_err(|_| HandshakeError::InvalidRsaKey)?;
    RsaPublicKey::from_components(&modulus, &[0x01, 0x00, 0x01])
}

impl FactoredPq {
    pub fn p_bytes(&self) -> Vec<u8> {
        unsigned_be_bytes(self.p)
    }

    pub fn q_bytes(&self) -> Vec<u8> {
        unsigned_be_bytes(self.q)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKeyHandshakeState {
    Ready,
    AwaitingResPq,
    ResPqReceived(ResPq),
    AwaitingServerDh,
    AwaitingDhGen,
    Complete,
}

pub struct AuthKeyHandshake {
    nonce: Nonce,
    state: AuthKeyHandshakeState,
    server_nonce: Option<Nonce>,
    new_nonce: Option<Zeroizing<[u8; 32]>>,
    server_dh: Option<ServerDhInnerData>,
    prepared_client_dh: Option<PreparedClientDh>,
}

impl AuthKeyHandshake {
    pub fn new(nonce: Nonce) -> Self {
        Self {
            nonce,
            state: AuthKeyHandshakeState::Ready,
            server_nonce: None,
            new_nonce: None,
            server_dh: None,
            prepared_client_dh: None,
        }
    }

    pub fn state(&self) -> &AuthKeyHandshakeState {
        &self.state
    }

    pub fn begin(&mut self, message_id: i64) -> Result<PlaintextEnvelope, HandshakeError> {
        if !matches!(self.state, AuthKeyHandshakeState::Ready) {
            return Err(HandshakeError::InvalidState("req_pq_multi already sent"));
        }
        let mut writer = TlWriter::with_capacity(20);
        writer.write_u32(REQ_PQ_MULTI_CONSTRUCTOR);
        writer.write_i128_bytes(&self.nonce);
        self.state = AuthKeyHandshakeState::AwaitingResPq;
        Ok(PlaintextEnvelope {
            message_id,
            body: writer.into_bytes(),
        })
    }

    pub fn receive_res_pq(
        &mut self,
        envelope: PlaintextEnvelope,
    ) -> Result<&ResPq, HandshakeError> {
        if !matches!(self.state, AuthKeyHandshakeState::AwaitingResPq) {
            return Err(HandshakeError::InvalidState("not awaiting resPQ"));
        }
        let response = parse_res_pq(&envelope.body)?;
        if response.nonce != self.nonce {
            return Err(HandshakeError::NonceMismatch);
        }
        if response.pq.is_empty() {
            return Err(HandshakeError::EmptyPq);
        }
        if response.server_public_key_fingerprints.is_empty() {
            return Err(HandshakeError::MissingServerKeyFingerprint);
        }
        self.server_nonce = Some(response.server_nonce);
        self.state = AuthKeyHandshakeState::ResPqReceived(response);
        match &self.state {
            AuthKeyHandshakeState::ResPqReceived(response) => Ok(response),
            _ => unreachable!("state was assigned above"),
        }
    }

    pub fn prepare_req_dh_params(
        &mut self,
        message_id: i64,
        dc_id: i32,
        test_environment: bool,
    ) -> Result<PlaintextEnvelope, HandshakeError> {
        self.prepare_req_dh_params_with_random(
            message_id,
            dc_id,
            test_environment,
            secure_random_fill,
        )
    }

    pub fn prepare_req_dh_params_with_random<F>(
        &mut self,
        message_id: i64,
        dc_id: i32,
        test_environment: bool,
        mut fill_random: F,
    ) -> Result<PlaintextEnvelope, HandshakeError>
    where
        F: FnMut(&mut [u8]) -> Result<(), HandshakeError>,
    {
        let response = match &self.state {
            AuthKeyHandshakeState::ResPqReceived(response) => response.clone(),
            _ => return Err(HandshakeError::InvalidState("resPQ is not available")),
        };
        let factors = factor_res_pq(&response)?;
        let key = telegram_server_rsa_key(test_environment)?;
        let fingerprint = key.fingerprint()?;
        if !response
            .server_public_key_fingerprints
            .contains(&fingerprint)
        {
            return Err(HandshakeError::NoTrustedServerKey);
        }
        let mut new_nonce = Zeroizing::new([0_u8; 32]);
        fill_random(&mut new_nonce[..])?;
        let inner = Zeroizing::new(build_p_q_inner_data_dc(
            &response, &factors, &new_nonce, dc_id,
        )?);
        let encrypted = rsa_pad_with_random(&inner, &key, &mut fill_random)?;
        let body = build_req_dh_params(&response, &factors, fingerprint, &encrypted)?;
        self.new_nonce = Some(new_nonce);
        self.state = AuthKeyHandshakeState::AwaitingServerDh;
        Ok(PlaintextEnvelope { message_id, body })
    }

    pub fn receive_server_dh(
        &mut self,
        envelope: PlaintextEnvelope,
        client_message_id: i64,
    ) -> Result<PlaintextEnvelope, HandshakeError> {
        self.receive_server_dh_with_random(envelope, client_message_id, secure_random_fill)
    }

    pub fn receive_server_dh_with_random<F>(
        &mut self,
        envelope: PlaintextEnvelope,
        client_message_id: i64,
        fill_random: F,
    ) -> Result<PlaintextEnvelope, HandshakeError>
    where
        F: FnMut(&mut [u8]) -> Result<(), HandshakeError>,
    {
        if !matches!(self.state, AuthKeyHandshakeState::AwaitingServerDh) {
            return Err(HandshakeError::InvalidState(
                "not awaiting server_DH_params",
            ));
        }
        let new_nonce = self
            .new_nonce
            .as_ref()
            .ok_or(HandshakeError::MissingHandshakeData)?;
        let server_nonce = self
            .server_nonce
            .ok_or(HandshakeError::MissingHandshakeData)?;
        let response = parse_server_dh_params(&envelope.body, &self.nonce, &server_nonce)?;
        let encrypted_answer = match response {
            ServerDhParams::Ok { encrypted_answer } => encrypted_answer,
            ServerDhParams::Fail { new_nonce_hash } => {
                let digest = Sha1::digest(&new_nonce[..]);
                let expected: [u8; 16] =
                    digest[4..20].try_into().expect("SHA-1 suffix is 16 bytes");
                if !bool::from(new_nonce_hash.ct_eq(&expected)) {
                    return Err(HandshakeError::ServerDhHashMismatch);
                }
                return Err(HandshakeError::ServerDhRejected);
            }
        };
        let server_dh =
            decrypt_server_dh_inner_data(&encrypted_answer, new_nonce, &self.nonce, &server_nonce)?;
        let prepared = prepare_client_dh(&server_dh, new_nonce, 0, fill_random)?;
        let body = prepared.request_body.clone();
        self.server_dh = Some(server_dh);
        self.prepared_client_dh = Some(prepared);
        self.state = AuthKeyHandshakeState::AwaitingDhGen;
        Ok(PlaintextEnvelope {
            message_id: client_message_id,
            body,
        })
    }

    pub fn receive_dh_gen(
        &mut self,
        envelope: PlaintextEnvelope,
        retry_message_id: i64,
    ) -> Result<DhGenAction, HandshakeError> {
        if !matches!(self.state, AuthKeyHandshakeState::AwaitingDhGen) {
            return Err(HandshakeError::InvalidState("not awaiting dh_gen result"));
        }
        let new_nonce = self
            .new_nonce
            .as_ref()
            .ok_or(HandshakeError::MissingHandshakeData)?;
        let auth_key_aux_hash = self
            .prepared_client_dh
            .as_ref()
            .ok_or(HandshakeError::MissingHandshakeData)?
            .auth_key_aux_hash;
        let server_nonce = self
            .server_nonce
            .ok_or(HandshakeError::MissingHandshakeData)?;
        let result = parse_dh_gen_result(
            &envelope.body,
            &self.nonce,
            &server_nonce,
            new_nonce,
            auth_key_aux_hash,
        )?;
        match result {
            DhGenResult::Ok => {
                let prepared = self
                    .prepared_client_dh
                    .take()
                    .ok_or(HandshakeError::MissingHandshakeData)?;
                let server_time = self
                    .server_dh
                    .as_ref()
                    .ok_or(HandshakeError::MissingHandshakeData)?
                    .server_time;
                self.state = AuthKeyHandshakeState::Complete;
                Ok(DhGenAction::Established(Box::new(EstablishedAuthKey {
                    auth_key: prepared.auth_key,
                    server_salt: prepared.server_salt,
                    server_time,
                })))
            }
            DhGenResult::Retry => {
                let server_dh = self
                    .server_dh
                    .as_ref()
                    .ok_or(HandshakeError::MissingHandshakeData)?;
                let next =
                    prepare_client_dh(server_dh, new_nonce, auth_key_aux_hash, secure_random_fill)?;
                let body = next.request_body.clone();
                self.prepared_client_dh = Some(next);
                Ok(DhGenAction::Retry(PlaintextEnvelope {
                    message_id: retry_message_id,
                    body,
                }))
            }
            DhGenResult::Fail => Err(HandshakeError::DhGenRejected),
        }
    }

    pub fn pending_auth_key_aux_hash(&self) -> Option<u64> {
        self.prepared_client_dh
            .as_ref()
            .map(|prepared| prepared.auth_key_aux_hash)
    }
}

fn secure_random_fill(output: &mut [u8]) -> Result<(), HandshakeError> {
    getrandom::getrandom(output).map_err(|_| HandshakeError::RandomnessUnavailable)
}

pub fn parse_res_pq(input: &[u8]) -> Result<ResPq, HandshakeError> {
    let mut reader = TlReader::with_limits(input, 1024, 64);
    let constructor = reader.read_u32()?;
    if constructor != RES_PQ_CONSTRUCTOR {
        return Err(HandshakeError::UnexpectedConstructor(constructor));
    }
    let nonce = reader.read_i128_bytes()?;
    let server_nonce = reader.read_i128_bytes()?;
    let pq = reader.read_bytes()?.to_vec();
    let fingerprint_count = reader.read_vector_length()?;
    let mut server_public_key_fingerprints = Vec::with_capacity(fingerprint_count);
    for _ in 0..fingerprint_count {
        server_public_key_fingerprints.push(reader.read_u64()?);
    }
    if !reader.is_finished() {
        return Err(HandshakeError::TrailingBodyBytes(reader.remaining()));
    }
    Ok(ResPq {
        nonce,
        server_nonce,
        pq,
        server_public_key_fingerprints,
    })
}

pub fn factor_res_pq(response: &ResPq) -> Result<FactoredPq, HandshakeError> {
    let pq = decode_unsigned_be(&response.pq)?;
    if pq < 4 || is_prime(pq) {
        return Err(HandshakeError::PqNotSemiprime(pq));
    }
    let factor = pollard_rho_factor(pq).ok_or(HandshakeError::PqFactorizationFailed(pq))?;
    let other = pq / factor;
    if factor == 1 || other == 1 || factor.saturating_mul(other) != pq {
        return Err(HandshakeError::PqFactorizationFailed(pq));
    }
    if !is_prime(factor) || !is_prime(other) {
        return Err(HandshakeError::PqNotSemiprime(pq));
    }
    let (p, q) = if factor < other {
        (factor, other)
    } else {
        (other, factor)
    };
    Ok(FactoredPq { pq, p, q })
}

pub fn select_server_key_fingerprint(
    response: &ResPq,
    trusted_fingerprints: &[u64],
) -> Result<u64, HandshakeError> {
    response
        .server_public_key_fingerprints
        .iter()
        .copied()
        .find(|fingerprint| trusted_fingerprints.contains(fingerprint))
        .ok_or(HandshakeError::NoTrustedServerKey)
}

pub fn build_p_q_inner_data_dc(
    response: &ResPq,
    factors: &FactoredPq,
    new_nonce: &[u8; 32],
    dc_id: i32,
) -> Result<Vec<u8>, HandshakeError> {
    if decode_unsigned_be(&response.pq)? != factors.pq
        || factors.p.saturating_mul(factors.q) != factors.pq
    {
        return Err(HandshakeError::PqFactorsDoNotMatch);
    }
    let mut writer = TlWriter::new();
    writer.write_u32(P_Q_INNER_DATA_DC_CONSTRUCTOR);
    writer.write_bytes(&response.pq)?;
    writer.write_bytes(&factors.p_bytes())?;
    writer.write_bytes(&factors.q_bytes())?;
    writer.write_i128_bytes(&response.nonce);
    writer.write_i128_bytes(&response.server_nonce);
    writer.write_i256_bytes(new_nonce);
    writer.write_i32(dc_id);
    Ok(writer.into_bytes())
}

pub fn build_req_dh_params(
    response: &ResPq,
    factors: &FactoredPq,
    public_key_fingerprint: u64,
    encrypted_inner_data: &[u8],
) -> Result<Vec<u8>, HandshakeError> {
    if !response
        .server_public_key_fingerprints
        .contains(&public_key_fingerprint)
    {
        return Err(HandshakeError::NoTrustedServerKey);
    }
    if encrypted_inner_data.is_empty() {
        return Err(HandshakeError::EmptyEncryptedInnerData);
    }
    let mut writer = TlWriter::new();
    writer.write_u32(REQ_DH_PARAMS_CONSTRUCTOR);
    writer.write_i128_bytes(&response.nonce);
    writer.write_i128_bytes(&response.server_nonce);
    writer.write_bytes(&factors.p_bytes())?;
    writer.write_bytes(&factors.q_bytes())?;
    writer.write_u64(public_key_fingerprint);
    writer.write_bytes(encrypted_inner_data)?;
    Ok(writer.into_bytes())
}

pub fn rsa_pad(data: &[u8], key: &RsaPublicKey) -> Result<Vec<u8>, HandshakeError> {
    rsa_pad_with_random(data, key, |output| {
        getrandom::getrandom(output).map_err(|_| HandshakeError::RandomnessUnavailable)
    })
}

pub fn rsa_pad_with_random<F>(
    data: &[u8],
    key: &RsaPublicKey,
    mut fill_random: F,
) -> Result<Vec<u8>, HandshakeError>
where
    F: FnMut(&mut [u8]) -> Result<(), HandshakeError>,
{
    if data.len() > 144 {
        return Err(HandshakeError::RsaPadDataTooLong(data.len()));
    }
    let modulus = BigUint::from_bytes_be(&key.modulus);
    let exponent = BigUint::from_bytes_be(&key.exponent);
    let exponent_is_odd = exponent
        .to_bytes_le()
        .first()
        .is_some_and(|byte| byte & 1 == 1);
    if exponent < BigUint::from(3_u8) || !exponent_is_odd || modulus.bits() != 2048 {
        return Err(HandshakeError::InvalidRsaKey);
    }

    for _ in 0..128 {
        let mut data_with_padding = Zeroizing::new(vec![0_u8; 192]);
        data_with_padding[..data.len()].copy_from_slice(data);
        fill_random(&mut data_with_padding[data.len()..])?;
        let mut data_pad_reversed = Zeroizing::new(data_with_padding.to_vec());
        data_pad_reversed.reverse();

        let mut temp_key = Zeroizing::new([0_u8; 32]);
        fill_random(temp_key.as_mut())?;
        let mut hash = Sha256::new();
        hash.update(*temp_key);
        hash.update(&data_with_padding);
        let digest = hash.finalize();
        data_pad_reversed.extend_from_slice(&digest);

        let aes_encrypted = aes_ige_encrypt(&data_pad_reversed, &temp_key, &[0_u8; 32])?;
        let encrypted_hash = Sha256::digest(&aes_encrypted);
        let mut key_aes_encrypted = Zeroizing::new(Vec::with_capacity(256));
        key_aes_encrypted.extend(
            temp_key
                .iter()
                .zip(encrypted_hash.iter())
                .map(|(left, right)| *left ^ *right),
        );
        key_aes_encrypted.extend_from_slice(&aes_encrypted);

        let padded = BigUint::from_bytes_be(&key_aes_encrypted);
        if padded >= modulus {
            continue;
        }
        let encrypted = padded.modpow(&exponent, &modulus);
        let encoded = encrypted.to_bytes_be();
        if encoded.len() > 256 {
            return Err(HandshakeError::InvalidRsaResult);
        }
        let mut result = vec![0_u8; 256 - encoded.len()];
        result.extend_from_slice(&encoded);
        return Ok(result);
    }
    Err(HandshakeError::RsaPadRetryLimit)
}

pub fn parse_server_dh_params(
    input: &[u8],
    expected_nonce: &Nonce,
    expected_server_nonce: &Nonce,
) -> Result<ServerDhParams, HandshakeError> {
    let mut reader = TlReader::with_limits(input, DEFAULT_MAX_PLAINTEXT_BODY, 64);
    let constructor = reader.read_u32()?;
    let nonce = reader.read_i128_bytes()?;
    let server_nonce = reader.read_i128_bytes()?;
    validate_nonce_pair(&nonce, &server_nonce, expected_nonce, expected_server_nonce)?;
    let response = match constructor {
        SERVER_DH_PARAMS_OK_CONSTRUCTOR => ServerDhParams::Ok {
            encrypted_answer: reader.read_bytes()?.to_vec(),
        },
        SERVER_DH_PARAMS_FAIL_CONSTRUCTOR => ServerDhParams::Fail {
            new_nonce_hash: reader.read_i128_bytes()?,
        },
        other => return Err(HandshakeError::UnexpectedServerDhConstructor(other)),
    };
    if !reader.is_finished() {
        return Err(HandshakeError::TrailingBodyBytes(reader.remaining()));
    }
    Ok(response)
}

pub fn derive_tmp_aes_key_iv(new_nonce: &[u8; 32], server_nonce: &Nonce) -> ([u8; 32], [u8; 32]) {
    let mut new_server = Sha1::new();
    new_server.update(new_nonce);
    new_server.update(server_nonce);
    let new_server = new_server.finalize();

    let mut server_new = Sha1::new();
    server_new.update(server_nonce);
    server_new.update(new_nonce);
    let server_new = server_new.finalize();

    let mut new_new = Sha1::new();
    new_new.update(new_nonce);
    new_new.update(new_nonce);
    let new_new = new_new.finalize();

    let mut key = [0_u8; 32];
    key[..20].copy_from_slice(&new_server);
    key[20..].copy_from_slice(&server_new[..12]);

    let mut iv = [0_u8; 32];
    iv[..8].copy_from_slice(&server_new[12..20]);
    iv[8..28].copy_from_slice(&new_new);
    iv[28..].copy_from_slice(&new_nonce[..4]);
    (key, iv)
}

pub fn decrypt_server_dh_inner_data(
    encrypted_answer: &[u8],
    new_nonce: &[u8; 32],
    expected_nonce: &Nonce,
    expected_server_nonce: &Nonce,
) -> Result<ServerDhInnerData, HandshakeError> {
    if encrypted_answer.is_empty() || !encrypted_answer.len().is_multiple_of(16) {
        return Err(HandshakeError::InvalidServerDhCiphertext);
    }
    let (mut key, mut iv) = derive_tmp_aes_key_iv(new_nonce, expected_server_nonce);
    let plaintext = Zeroizing::new(aes_ige_decrypt(encrypted_answer, &key, &iv)?);
    key.zeroize();
    iv.zeroize();
    if plaintext.len() < 20 + 4 + 16 + 16 + 4 + 4 + 4 + 4 {
        return Err(HandshakeError::InvalidServerDhAnswer);
    }
    let expected_hash: [u8; 20] = plaintext[..20]
        .try_into()
        .map_err(|_| HandshakeError::InvalidServerDhAnswer)?;
    let answer = &plaintext[20..];
    let mut reader = TlReader::with_limits(answer, 1024 * 1024, 64);
    let constructor = reader.read_u32()?;
    if constructor != SERVER_DH_INNER_DATA_CONSTRUCTOR {
        return Err(HandshakeError::UnexpectedServerDhInnerConstructor(
            constructor,
        ));
    }
    let nonce = reader.read_i128_bytes()?;
    let server_nonce = reader.read_i128_bytes()?;
    validate_nonce_pair(&nonce, &server_nonce, expected_nonce, expected_server_nonce)?;
    let generator = reader.read_i32()?;
    let dh_prime = reader.read_bytes()?.to_vec();
    let g_a = reader.read_bytes()?.to_vec();
    let server_time = reader.read_i32()?;
    let answer_length = reader.position();
    let padding_length = answer.len().saturating_sub(answer_length);
    if padding_length > 15 {
        return Err(HandshakeError::InvalidServerDhPadding(padding_length));
    }
    let actual_hash: [u8; 20] = Sha1::digest(&answer[..answer_length]).into();
    if !bool::from(expected_hash.ct_eq(&actual_hash)) {
        return Err(HandshakeError::ServerDhHashMismatch);
    }
    if !(2..=7).contains(&generator) || dh_prime.len() != 256 || g_a.is_empty() || g_a.len() > 256 {
        return Err(HandshakeError::InvalidServerDhParameters);
    }
    let result = ServerDhInnerData {
        nonce,
        server_nonce,
        generator,
        dh_prime,
        g_a,
        server_time,
    };
    validate_server_dh_parameters(&result)?;
    Ok(result)
}

pub fn validate_server_dh_parameters(data: &ServerDhInnerData) -> Result<(), HandshakeError> {
    if data.dh_prime.len() != 256 {
        return Err(HandshakeError::InvalidServerDhParameters);
    }
    let prime = BigUint::from_bytes_be(&data.dh_prime);
    if prime.bits() != 2048 {
        return Err(HandshakeError::InvalidServerDhParameters);
    }
    let known_prime = hex::decode(KNOWN_DH_PRIME_HEX)
        .expect("the pinned Telegram safe-prime constant is valid hex");
    if data.dh_prime != known_prime {
        // Unknown primes must undergo expensive safe-prime verification before
        // use. Rejecting here is safer than accepting an attacker-selected p.
        return Err(HandshakeError::UntrustedDhPrime);
    }
    let remainder = |modulus: u32| {
        (&prime % BigUint::from(modulus))
            .to_bytes_be()
            .iter()
            .fold(0_u32, |value, byte| (value << 8) | u32::from(*byte))
    };
    let generator_is_valid = match data.generator {
        2 => remainder(8) == 7,
        3 => remainder(3) == 2,
        4 => true,
        5 => matches!(remainder(5), 1 | 4),
        6 => matches!(remainder(24), 19 | 23),
        7 => matches!(remainder(7), 3 | 5 | 6),
        _ => false,
    };
    if !generator_is_valid {
        return Err(HandshakeError::InvalidDhGenerator);
    }
    let g_a = BigUint::from_bytes_be(&data.g_a);
    validate_dh_public_value(&prime, &g_a)?;
    Ok(())
}

pub fn prepare_client_dh<F>(
    data: &ServerDhInnerData,
    new_nonce: &[u8; 32],
    retry_id: u64,
    mut fill_random: F,
) -> Result<PreparedClientDh, HandshakeError>
where
    F: FnMut(&mut [u8]) -> Result<(), HandshakeError>,
{
    validate_server_dh_parameters(data)?;
    let prime = BigUint::from_bytes_be(&data.dh_prime);
    let g_a = BigUint::from_bytes_be(&data.g_a);
    let mut exponent_bytes = Zeroizing::new([0_u8; 256]);
    fill_random(&mut exponent_bytes[..])?;
    let mut exponent = Zeroizing::new(BigUint::from_bytes_be(&exponent_bytes[..]));
    if exponent.bits() == 0 {
        *exponent = BigUint::from(2_u8);
    }
    let generator = BigUint::from(data.generator as u32);
    let g_b = Zeroizing::new(generator.modpow(&exponent, &prime));
    validate_dh_public_value(&prime, &g_b)?;
    let auth_key_number = Zeroizing::new(g_a.modpow(&exponent, &prime));
    exponent.zeroize();
    let auth_key_bytes = biguint_to_fixed_256(&auth_key_number)?;
    let auth_key = crate::crypto::AuthKey::from_slice(&auth_key_bytes)?;
    let auth_key_digest = Sha1::digest(&auth_key_bytes[..]);
    let auth_key_aux_hash = u64::from_le_bytes(
        auth_key_digest[..8]
            .try_into()
            .expect("SHA-1 prefix is 8 bytes"),
    );
    let server_salt = i64::from_le_bytes(std::array::from_fn(|index| {
        new_nonce[index] ^ data.server_nonce[index]
    }));

    let mut inner = TlWriter::new();
    inner.write_u32(CLIENT_DH_INNER_DATA_CONSTRUCTOR);
    inner.write_i128_bytes(&data.nonce);
    inner.write_i128_bytes(&data.server_nonce);
    inner.write_u64(retry_id);
    inner.write_bytes(&biguint_to_minimal_be(&g_b))?;
    let inner = Zeroizing::new(inner.into_bytes());

    let mut answer_with_hash = Zeroizing::new(Sha1::digest(&inner[..]).to_vec());
    answer_with_hash.extend_from_slice(&inner);
    let padding_length = (16 - answer_with_hash.len() % 16) % 16;
    let original_length = answer_with_hash.len();
    answer_with_hash.resize(original_length + padding_length, 0);
    if padding_length > 0 {
        fill_random(&mut answer_with_hash[original_length..])?;
    }
    let (mut key, mut iv) = derive_tmp_aes_key_iv(new_nonce, &data.server_nonce);
    let encrypted_data = aes_ige_encrypt(&answer_with_hash, &key, &iv)?;
    key.zeroize();
    iv.zeroize();

    let mut request = TlWriter::new();
    request.write_u32(SET_CLIENT_DH_PARAMS_CONSTRUCTOR);
    request.write_i128_bytes(&data.nonce);
    request.write_i128_bytes(&data.server_nonce);
    request.write_bytes(&encrypted_data)?;
    Ok(PreparedClientDh {
        request_body: request.into_bytes(),
        auth_key,
        auth_key_aux_hash,
        server_salt,
    })
}

pub fn parse_dh_gen_result(
    input: &[u8],
    expected_nonce: &Nonce,
    expected_server_nonce: &Nonce,
    new_nonce: &[u8; 32],
    auth_key_aux_hash: u64,
) -> Result<DhGenResult, HandshakeError> {
    let mut reader = TlReader::new(input);
    let constructor = reader.read_u32()?;
    let nonce = reader.read_i128_bytes()?;
    let server_nonce = reader.read_i128_bytes()?;
    validate_nonce_pair(&nonce, &server_nonce, expected_nonce, expected_server_nonce)?;
    let actual_hash = reader.read_i128_bytes()?;
    if !reader.is_finished() {
        return Err(HandshakeError::TrailingBodyBytes(reader.remaining()));
    }
    let (result, tag) = match constructor {
        DH_GEN_OK_CONSTRUCTOR => (DhGenResult::Ok, 1_u8),
        DH_GEN_RETRY_CONSTRUCTOR => (DhGenResult::Retry, 2_u8),
        DH_GEN_FAIL_CONSTRUCTOR => (DhGenResult::Fail, 3_u8),
        other => return Err(HandshakeError::UnexpectedDhGenConstructor(other)),
    };
    let mut material = Zeroizing::new(Vec::with_capacity(41));
    material.extend_from_slice(new_nonce);
    material.push(tag);
    material.extend_from_slice(&auth_key_aux_hash.to_le_bytes());
    let digest = Sha1::digest(&material);
    let expected_hash: [u8; 16] = digest[4..20].try_into().expect("SHA-1 suffix is 16 bytes");
    if !bool::from(actual_hash.ct_eq(&expected_hash)) {
        return Err(HandshakeError::DhGenHashMismatch);
    }
    Ok(result)
}

fn validate_dh_public_value(prime: &BigUint, value: &BigUint) -> Result<(), HandshakeError> {
    let lower_bound = BigUint::from(1_u8) << (2048 - 64);
    let upper_bound = prime - &lower_bound;
    if value < &lower_bound || value > &upper_bound {
        return Err(HandshakeError::InvalidDhPublicValue);
    }
    Ok(())
}

fn biguint_to_minimal_be(value: &BigUint) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    if bytes.is_empty() {
        vec![0]
    } else {
        bytes
    }
}

fn biguint_to_fixed_256(value: &BigUint) -> Result<Zeroizing<Vec<u8>>, HandshakeError> {
    let bytes = value.to_bytes_be();
    if bytes.len() > 256 {
        return Err(HandshakeError::InvalidDhPublicValue);
    }
    let mut output = Zeroizing::new(vec![0_u8; 256 - bytes.len()]);
    output.extend_from_slice(&bytes);
    Ok(output)
}

fn validate_nonce_pair(
    nonce: &Nonce,
    server_nonce: &Nonce,
    expected_nonce: &Nonce,
    expected_server_nonce: &Nonce,
) -> Result<(), HandshakeError> {
    if nonce != expected_nonce {
        return Err(HandshakeError::NonceMismatch);
    }
    if server_nonce != expected_server_nonce {
        return Err(HandshakeError::ServerNonceMismatch);
    }
    Ok(())
}

fn decode_unsigned_be(bytes: &[u8]) -> Result<u64, HandshakeError> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(HandshakeError::InvalidPqEncoding(bytes.len()));
    }
    Ok(bytes
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte)))
}

fn unsigned_be_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len() - 1);
    bytes[first..].to_vec()
}

fn greatest_common_divisor(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn multiply_mod(a: u64, b: u64, modulus: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) % u128::from(modulus)) as u64
}

fn rho_step(value: u64, constant: u64, modulus: u64) -> u64 {
    ((u128::from(multiply_mod(value, value, modulus)) + u128::from(constant)) % u128::from(modulus))
        as u64
}

fn power_mod(mut base: u64, mut exponent: u64, modulus: u64) -> u64 {
    let mut result = 1_u64;
    base %= modulus;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = multiply_mod(result, base, modulus);
        }
        base = multiply_mod(base, base, modulus);
        exponent >>= 1;
    }
    result
}

fn is_prime(value: u64) -> bool {
    if value < 2 {
        return false;
    }
    for prime in [2_u64, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37] {
        if value == prime {
            return true;
        }
        if value.is_multiple_of(prime) {
            return false;
        }
    }
    let mut odd = value - 1;
    let powers_of_two = odd.trailing_zeros();
    odd >>= powers_of_two;
    for base in [2_u64, 325, 9_375, 28_178, 450_775, 9_780_504, 1_795_265_022] {
        if base % value == 0 {
            continue;
        }
        let mut witness = power_mod(base, odd, value);
        if witness == 1 || witness == value - 1 {
            continue;
        }
        let mut composite = true;
        for _ in 1..powers_of_two {
            witness = multiply_mod(witness, witness, value);
            if witness == value - 1 {
                composite = false;
                break;
            }
        }
        if composite {
            return false;
        }
    }
    true
}

fn pollard_rho_factor(value: u64) -> Option<u64> {
    if value.is_multiple_of(2) {
        return Some(2);
    }
    if value.is_multiple_of(3) {
        return Some(3);
    }
    for constant in 1_u64..=128 {
        let mut x = 2_u64;
        let mut y = 2_u64;
        for _ in 0..1_000_000 {
            x = rho_step(x, constant, value);
            y = rho_step(y, constant, value);
            y = rho_step(y, constant, value);
            let divisor = greatest_common_divisor(x.abs_diff(y), value);
            if divisor == 1 {
                continue;
            }
            if divisor != value {
                return Some(divisor);
            }
            break;
        }
    }
    None
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    #[error("plaintext MTProto envelope is truncated: {length} bytes")]
    TruncatedEnvelope { length: usize },
    #[error("unencrypted MTProto envelope has non-zero auth_key_id {0}")]
    UnexpectedAuthKeyId(i64),
    #[error("plaintext MTProto body length {0} is invalid")]
    InvalidBodyLength(i64),
    #[error("plaintext MTProto body length {length} exceeds maximum {maximum}")]
    BodyTooLarge { length: usize, maximum: usize },
    #[error("plaintext MTProto body declared {declared} bytes but carried {actual}")]
    EnvelopeLengthMismatch { declared: usize, actual: usize },
    #[error("auth-key handshake state does not permit this operation: {0}")]
    InvalidState(&'static str),
    #[error("auth-key handshake is missing data from an earlier phase")]
    MissingHandshakeData,
    #[error("expected resPQ but received constructor 0x{0:08x}")]
    UnexpectedConstructor(u32),
    #[error("resPQ nonce does not match req_pq_multi nonce")]
    NonceMismatch,
    #[error("server nonce does not match the active auth-key handshake")]
    ServerNonceMismatch,
    #[error("resPQ returned an empty pq value")]
    EmptyPq,
    #[error("resPQ did not contain a server RSA key fingerprint")]
    MissingServerKeyFingerprint,
    #[error("resPQ pq value has invalid unsigned big-endian length {0}")]
    InvalidPqEncoding(usize),
    #[error("resPQ pq value {0} could not be factored")]
    PqFactorizationFailed(u64),
    #[error("resPQ pq value {0} is not a product of two primes")]
    PqNotSemiprime(u64),
    #[error("provided p and q do not multiply to the resPQ value")]
    PqFactorsDoNotMatch,
    #[error("none of the server RSA fingerprints are trusted")]
    NoTrustedServerKey,
    #[error("RSA-encrypted p_q_inner_data must not be empty")]
    EmptyEncryptedInnerData,
    #[error("Telegram RSA modulus must contain 256 bytes, found {0}")]
    InvalidRsaModulusLength(usize),
    #[error("Telegram RSA exponent is empty or zero")]
    InvalidRsaExponent,
    #[error("Telegram auth-key RSA key is not a valid 2048-bit key")]
    InvalidRsaKey,
    #[error("p_q_inner_data is {0} bytes; RSA_PAD permits at most 144")]
    RsaPadDataTooLong(usize),
    #[error("secure random bytes are unavailable")]
    RandomnessUnavailable,
    #[error("RSA_PAD could not produce a value smaller than the modulus")]
    RsaPadRetryLimit,
    #[error("RSA modular exponentiation produced an invalid result")]
    InvalidRsaResult,
    #[error("expected server_DH_params_ok/fail but received constructor 0x{0:08x}")]
    UnexpectedServerDhConstructor(u32),
    #[error("expected server_DH_inner_data but received constructor 0x{0:08x}")]
    UnexpectedServerDhInnerConstructor(u32),
    #[error("server_DH_params_ok encrypted_answer has an invalid AES-IGE length")]
    InvalidServerDhCiphertext,
    #[error("decrypted server_DH_inner_data is structurally invalid")]
    InvalidServerDhAnswer,
    #[error("decrypted server_DH_inner_data has {0} padding bytes; expected at most 15")]
    InvalidServerDhPadding(usize),
    #[error("server_DH_inner_data SHA-1 authentication failed")]
    ServerDhHashMismatch,
    #[error("Telegram server rejected req_DH_params")]
    ServerDhRejected,
    #[error("server_DH_inner_data contains invalid generator, prime, or g_a values")]
    InvalidServerDhParameters,
    #[error("server_DH_inner_data uses a safe prime that is not in the audited trust set")]
    UntrustedDhPrime,
    #[error("server_DH_inner_data generator fails the p mod 4g safety condition")]
    InvalidDhGenerator,
    #[error("server_DH_inner_data g_a is outside the recommended 2048-bit safety interval")]
    InvalidDhPublicValue,
    #[error("expected dh_gen_ok/retry/fail but received constructor 0x{0:08x}")]
    UnexpectedDhGenConstructor(u32),
    #[error("dh_gen response new_nonce_hash authentication failed")]
    DhGenHashMismatch,
    #[error("Telegram server rejected the client DH value")]
    DhGenRejected,
    #[error("resPQ body contains {0} trailing bytes")]
    TrailingBodyBytes(usize),
    #[error("invalid TL body: {0}")]
    Tl(#[from] TlError),
    #[error("MTProto cryptography failed: {0}")]
    Crypto(#[from] CryptoError),
}
