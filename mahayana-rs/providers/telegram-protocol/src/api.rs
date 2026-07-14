use crate::dc::DcDirectory;
use crate::dc::DcEndpoint;
use crate::dc::DcError;
use crate::srp::PasswordSrpParameters;
use crate::srp::PasswordSrpProof;
use crate::srp::SrpError;
use crate::tl::TlError;
use crate::tl::TlReader;
use crate::tl::TlWriter;
use serde::Deserialize;
use serde::Serialize;
use std::net::IpAddr;
use thiserror::Error;

pub const MTPROTO_LAYER: i32 = 227;
pub const INVOKE_WITH_LAYER_CONSTRUCTOR: u32 = 0xda9b_0d0d;
pub const INIT_CONNECTION_CONSTRUCTOR: u32 = 0xc1cd_5ea9;
pub const HELP_GET_CONFIG_CONSTRUCTOR: u32 = 0xc4f9_186b;
pub const AUTH_SEND_CODE_CONSTRUCTOR: u32 = 0xa677_244f;
pub const CODE_SETTINGS_CONSTRUCTOR: u32 = 0xad25_3d78;
pub const AUTH_SIGN_IN_CONSTRUCTOR: u32 = 0x8d52_a951;
pub const AUTH_SIGN_UP_CONSTRUCTOR: u32 = 0xaac7_b717;
pub const MSGS_ACK_CONSTRUCTOR: u32 = 0x62d6_b459;
pub const CONFIG_CONSTRUCTOR: u32 = 0xcc1a_241e;
pub const DC_OPTION_CONSTRUCTOR: u32 = 0x18b7_a10d;
pub const AUTH_SENT_CODE_CONSTRUCTOR: u32 = 0x5e00_2502;
pub const AUTH_SENT_CODE_SUCCESS_CONSTRUCTOR: u32 = 0x2390_fe44;
pub const AUTH_SENT_CODE_PAYMENT_REQUIRED_CONSTRUCTOR: u32 = 0xf882_7ebf;
pub const RPC_ERROR_CONSTRUCTOR: u32 = 0x2144_ca19;
pub const ACCOUNT_GET_PASSWORD_CONSTRUCTOR: u32 = 0x548a_30f5;
pub const ACCOUNT_PASSWORD_CONSTRUCTOR: u32 = 0x957b_50fb;
pub const PASSWORD_KDF_ALGO_CONSTRUCTOR: u32 = 0x3a91_2d4a;
pub const AUTH_CHECK_PASSWORD_CONSTRUCTOR: u32 = 0xd18b_4d16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitConnection<'a> {
    pub api_id: i32,
    pub device_model: &'a str,
    pub system_version: &'a str,
    pub app_version: &'a str,
    pub system_lang_code: &'a str,
    pub lang_pack: &'a str,
    pub lang_code: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDcDirectory {
    pub flags: i32,
    pub date: i32,
    pub expires: i32,
    pub directory: DcDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentCode {
    pub phone_code_hash: String,
    pub delivery: SentCodeDelivery,
    pub next_type: Option<NextCodeType>,
    pub timeout_seconds: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SentCodeResult {
    Code {
        code: SentCode,
    },
    Success,
    PaymentRequired {
        store_product: String,
        phone_code_hash: String,
        support_email_address: String,
        support_email_subject: String,
        premium_days: i32,
        currency: String,
        amount: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SentCodeDelivery {
    App { length: i32 },
    Sms { length: i32 },
    Call { length: i32 },
    FlashCall { pattern: String },
    MissedCall { prefix: String, length: i32 },
    Email { pattern: String, length: i32 },
    SetUpEmailRequired,
    FragmentSms { url: String, length: i32 },
    FirebaseSms { length: i32 },
    SmsWord { beginning: Option<String> },
    SmsPhrase { beginning: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NextCodeType {
    Sms,
    Call,
    FlashCall,
    MissedCall,
    FragmentSms,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcErrorResponse {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPasswordState {
    pub has_recovery: bool,
    pub hint: Option<String>,
    pub email_unconfirmed_pattern: Option<String>,
    pub srp: Option<PasswordSrpParameters>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ApiRequestError {
    #[error("Telegram api_id must be positive")]
    InvalidApiId,
    #[error("Telegram API field {0} must not be empty")]
    EmptyField(&'static str),
    #[error("Telegram phone number must use international +number format")]
    InvalidPhoneNumber,
    #[error("Telegram phone code must contain only digits")]
    InvalidPhoneCode,
    #[error("TL encoding failed: {0}")]
    Tl(#[from] TlError),
    #[error("Telegram Config contains an invalid IP address: {0}")]
    InvalidIpAddress(String),
    #[error("Telegram Config contains an invalid port: {0}")]
    InvalidPort(i32),
    #[error("Telegram data center configuration failed: {0}")]
    Dc(#[from] DcError),
    #[error("Telegram API response contains {0} trailing bytes")]
    TrailingBytes(usize),
    #[error("Telegram password SRP failed: {0}")]
    Srp(#[from] SrpError),
}

pub fn build_init_connection_get_config(
    connection: &InitConnection<'_>,
) -> Result<Vec<u8>, ApiRequestError> {
    validate_connection(connection)?;
    let mut writer = TlWriter::new();
    writer.write_u32(INVOKE_WITH_LAYER_CONSTRUCTOR);
    writer.write_i32(MTPROTO_LAYER);
    writer.write_u32(INIT_CONNECTION_CONSTRUCTOR);
    writer.write_i32(0);
    writer.write_i32(connection.api_id);
    writer.write_string(connection.device_model)?;
    writer.write_string(connection.system_version)?;
    writer.write_string(connection.app_version)?;
    writer.write_string(connection.system_lang_code)?;
    writer.write_string(connection.lang_pack)?;
    writer.write_string(connection.lang_code)?;
    writer.write_u32(HELP_GET_CONFIG_CONSTRUCTOR);
    Ok(writer.into_bytes())
}

pub fn build_auth_send_code(
    phone_number: &str,
    api_id: i32,
    api_hash: &str,
) -> Result<Vec<u8>, ApiRequestError> {
    validate_credentials(api_id, api_hash)?;
    validate_phone_number(phone_number)?;
    let mut writer = TlWriter::new();
    writer.write_u32(AUTH_SEND_CODE_CONSTRUCTOR);
    writer.write_string(phone_number)?;
    writer.write_i32(api_id);
    writer.write_string(api_hash)?;
    writer.write_u32(CODE_SETTINGS_CONSTRUCTOR);
    writer.write_i32(0);
    Ok(writer.into_bytes())
}

pub fn build_auth_sign_in(
    phone_number: &str,
    phone_code_hash: &str,
    phone_code: &str,
) -> Result<Vec<u8>, ApiRequestError> {
    validate_phone_number(phone_number)?;
    require_nonempty(phone_code_hash, "phone_code_hash")?;
    if phone_code.is_empty() || !phone_code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiRequestError::InvalidPhoneCode);
    }
    let mut writer = TlWriter::new();
    writer.write_u32(AUTH_SIGN_IN_CONSTRUCTOR);
    writer.write_i32(1);
    writer.write_string(phone_number)?;
    writer.write_string(phone_code_hash)?;
    writer.write_string(phone_code)?;
    Ok(writer.into_bytes())
}

pub fn build_auth_sign_up(
    phone_number: &str,
    phone_code_hash: &str,
    first_name: &str,
    last_name: &str,
) -> Result<Vec<u8>, ApiRequestError> {
    validate_phone_number(phone_number)?;
    require_nonempty(phone_code_hash, "phone_code_hash")?;
    require_nonempty(first_name, "first_name")?;
    let mut writer = TlWriter::new();
    writer.write_u32(AUTH_SIGN_UP_CONSTRUCTOR);
    writer.write_i32(0);
    writer.write_string(phone_number)?;
    writer.write_string(phone_code_hash)?;
    writer.write_string(first_name.trim())?;
    writer.write_string(last_name.trim())?;
    Ok(writer.into_bytes())
}

pub fn build_account_get_password() -> Vec<u8> {
    ACCOUNT_GET_PASSWORD_CONSTRUCTOR.to_le_bytes().to_vec()
}

pub fn build_auth_check_password(proof: &PasswordSrpProof) -> Result<Vec<u8>, ApiRequestError> {
    let encoded_proof = proof.encode_input_check_password()?;
    let mut output = Vec::with_capacity(4 + encoded_proof.len());
    output.extend_from_slice(&AUTH_CHECK_PASSWORD_CONSTRUCTOR.to_le_bytes());
    output.extend_from_slice(&encoded_proof);
    Ok(output)
}

pub fn build_msgs_ack(message_ids: &[i64]) -> Result<Vec<u8>, ApiRequestError> {
    let mut writer = TlWriter::new();
    writer.write_u32(MSGS_ACK_CONSTRUCTOR);
    writer.write_vector_length(message_ids.len())?;
    for message_id in message_ids {
        writer.write_i64(*message_id);
    }
    Ok(writer.into_bytes())
}

/// Parses the stable prefix of Telegram `Config` through `dc_options`.
/// Remaining layer-specific Config fields are intentionally left unread.
pub fn parse_config_dc_directory_prefix(
    input: &[u8],
) -> Result<ConfigDcDirectory, ApiRequestError> {
    let mut reader = TlReader::new(input);
    let constructor = reader.read_u32()?;
    if constructor != CONFIG_CONSTRUCTOR {
        return Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
            expected: CONFIG_CONSTRUCTOR,
            actual: constructor,
        }));
    }
    let flags = reader.read_i32()?;
    let date = reader.read_i32()?;
    let expires = reader.read_i32()?;
    let test_mode = reader.read_bool()?;
    let this_dc = reader.read_i32()?;
    let count = reader.read_vector_length()?;
    let mut endpoints = Vec::with_capacity(count);
    for _ in 0..count {
        let constructor = reader.read_u32()?;
        if constructor != DC_OPTION_CONSTRUCTOR {
            return Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
                expected: DC_OPTION_CONSTRUCTOR,
                actual: constructor,
            }));
        }
        let option_flags = reader.read_i32()?;
        let dc_id = reader.read_i32()?;
        let address = reader.read_string()?.to_string();
        let ip_address: IpAddr = address
            .parse()
            .map_err(|_| ApiRequestError::InvalidIpAddress(address))?;
        let raw_port = reader.read_i32()?;
        let port = u16::try_from(raw_port)
            .ok()
            .filter(|value| *value != 0)
            .ok_or(ApiRequestError::InvalidPort(raw_port))?;
        let secret = if option_flags & (1 << 10) != 0 {
            Some(reader.read_bytes()?.to_vec())
        } else {
            None
        };
        endpoints.push(DcEndpoint::new(
            dc_id,
            ip_address,
            port,
            option_flags & 2 != 0,
            option_flags & 4 != 0,
            option_flags & 8 != 0,
            option_flags & 16 != 0,
            option_flags & 32 != 0,
            secret,
        )?);
    }
    Ok(ConfigDcDirectory {
        flags,
        date,
        expires,
        directory: DcDirectory::new(this_dc, test_mode, endpoints)?,
    })
}

pub fn parse_auth_sent_code(input: &[u8]) -> Result<SentCodeResult, ApiRequestError> {
    let mut reader = TlReader::new(input);
    let constructor = reader.read_u32()?;
    match constructor {
        AUTH_SENT_CODE_CONSTRUCTOR => {
            let flags = reader.read_i32()?;
            let delivery = parse_sent_code_delivery(&mut reader)?;
            let phone_code_hash = reader.read_string()?.to_string();
            let next_type = if flags & 2 != 0 {
                Some(parse_next_code_type(&mut reader)?)
            } else {
                None
            };
            let timeout_seconds = if flags & 4 != 0 {
                Some(reader.read_i32()?)
            } else {
                None
            };
            if !reader.is_finished() {
                return Err(ApiRequestError::TrailingBytes(reader.remaining()));
            }
            Ok(SentCodeResult::Code {
                code: SentCode {
                    phone_code_hash,
                    delivery,
                    next_type,
                    timeout_seconds,
                },
            })
        }
        AUTH_SENT_CODE_SUCCESS_CONSTRUCTOR => Ok(SentCodeResult::Success),
        AUTH_SENT_CODE_PAYMENT_REQUIRED_CONSTRUCTOR => {
            let result = SentCodeResult::PaymentRequired {
                store_product: reader.read_string()?.to_string(),
                phone_code_hash: reader.read_string()?.to_string(),
                support_email_address: reader.read_string()?.to_string(),
                support_email_subject: reader.read_string()?.to_string(),
                premium_days: reader.read_i32()?,
                currency: reader.read_string()?.to_string(),
                amount: reader.read_i64()?,
            };
            if !reader.is_finished() {
                return Err(ApiRequestError::TrailingBytes(reader.remaining()));
            }
            Ok(result)
        }
        actual => Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
            expected: AUTH_SENT_CODE_CONSTRUCTOR,
            actual,
        })),
    }
}

pub fn try_parse_rpc_error(input: &[u8]) -> Result<Option<RpcErrorResponse>, ApiRequestError> {
    if input.len() < 4 {
        return Err(ApiRequestError::Tl(TlError::UnexpectedEof {
            offset: input.len(),
            needed: 4 - input.len(),
        }));
    }
    if u32::from_le_bytes(input[..4].try_into().expect("four-byte prefix")) != RPC_ERROR_CONSTRUCTOR
    {
        return Ok(None);
    }
    let mut reader = TlReader::new(input);
    reader.read_u32()?;
    let error = RpcErrorResponse {
        code: reader.read_i32()?,
        message: reader.read_string()?.to_string(),
    };
    if !reader.is_finished() {
        return Err(ApiRequestError::TrailingBytes(reader.remaining()));
    }
    Ok(Some(error))
}

/// Parses the current-password prefix of `account.Password`. New-password and
/// secure-secret algorithms follow this prefix and belong to settings flows.
pub fn parse_account_password_prefix(
    input: &[u8],
) -> Result<AccountPasswordState, ApiRequestError> {
    let mut reader = TlReader::new(input);
    let constructor = reader.read_u32()?;
    if constructor != ACCOUNT_PASSWORD_CONSTRUCTOR {
        return Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
            expected: ACCOUNT_PASSWORD_CONSTRUCTOR,
            actual: constructor,
        }));
    }
    let flags = reader.read_i32()?;
    let srp = if flags & 4 != 0 {
        let algorithm = reader.read_u32()?;
        if algorithm != PASSWORD_KDF_ALGO_CONSTRUCTOR {
            return Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
                expected: PASSWORD_KDF_ALGO_CONSTRUCTOR,
                actual: algorithm,
            }));
        }
        let salt1 = reader.read_bytes()?.to_vec();
        let salt2 = reader.read_bytes()?.to_vec();
        let generator = reader.read_i32()?;
        let prime = reader.read_bytes()?.to_vec();
        let server_b = reader.read_bytes()?.to_vec();
        let srp_id = reader.read_i64()?;
        Some(PasswordSrpParameters {
            srp_id,
            salt1,
            salt2,
            generator,
            prime,
            server_b,
        })
    } else {
        None
    };
    let hint = if flags & 8 != 0 {
        Some(reader.read_string()?.to_string())
    } else {
        None
    };
    let email_unconfirmed_pattern = if flags & 16 != 0 {
        Some(reader.read_string()?.to_string())
    } else {
        None
    };
    Ok(AccountPasswordState {
        has_recovery: flags & 1 != 0,
        hint,
        email_unconfirmed_pattern,
        srp,
    })
}

fn parse_sent_code_delivery(
    reader: &mut TlReader<'_>,
) -> Result<SentCodeDelivery, ApiRequestError> {
    Ok(match reader.read_u32()? {
        0x3dbb_5986 => SentCodeDelivery::App {
            length: reader.read_i32()?,
        },
        0xc000_bba2 => SentCodeDelivery::Sms {
            length: reader.read_i32()?,
        },
        0x5353_e5a7 => SentCodeDelivery::Call {
            length: reader.read_i32()?,
        },
        0xab03_c6d9 => SentCodeDelivery::FlashCall {
            pattern: reader.read_string()?.to_string(),
        },
        0x8200_6484 => SentCodeDelivery::MissedCall {
            prefix: reader.read_string()?.to_string(),
            length: reader.read_i32()?,
        },
        0xf450_f59b => {
            let flags = reader.read_i32()?;
            let pattern = reader.read_string()?.to_string();
            let length = reader.read_i32()?;
            if flags & 8 != 0 {
                reader.read_i32()?;
            }
            if flags & 16 != 0 {
                reader.read_i32()?;
            }
            SentCodeDelivery::Email { pattern, length }
        }
        0xa549_1dea => {
            reader.read_i32()?;
            SentCodeDelivery::SetUpEmailRequired
        }
        0xd956_5c39 => SentCodeDelivery::FragmentSms {
            url: reader.read_string()?.to_string(),
            length: reader.read_i32()?,
        },
        0x009f_d736 => {
            let flags = reader.read_i32()?;
            if flags & 1 != 0 {
                reader.read_bytes()?;
            }
            if flags & 4 != 0 {
                reader.read_i64()?;
                reader.read_bytes()?;
            }
            if flags & 2 != 0 {
                reader.read_string()?;
                reader.read_i32()?;
            }
            SentCodeDelivery::FirebaseSms {
                length: reader.read_i32()?,
            }
        }
        0xa416_ac81 => SentCodeDelivery::SmsWord {
            beginning: read_optional_beginning(reader)?,
        },
        0xb377_94af => SentCodeDelivery::SmsPhrase {
            beginning: read_optional_beginning(reader)?,
        },
        actual => {
            return Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
                expected: 0x3dbb_5986,
                actual,
            }))
        }
    })
}

fn read_optional_beginning(reader: &mut TlReader<'_>) -> Result<Option<String>, ApiRequestError> {
    let flags = reader.read_i32()?;
    Ok(if flags & 1 != 0 {
        Some(reader.read_string()?.to_string())
    } else {
        None
    })
}

fn parse_next_code_type(reader: &mut TlReader<'_>) -> Result<NextCodeType, ApiRequestError> {
    match reader.read_u32()? {
        0x72a3_158c => Ok(NextCodeType::Sms),
        0x741c_d3e3 => Ok(NextCodeType::Call),
        0x226c_cefb => Ok(NextCodeType::FlashCall),
        0xd61a_d6ee => Ok(NextCodeType::MissedCall),
        0x06ed_998c => Ok(NextCodeType::FragmentSms),
        actual => Err(ApiRequestError::Tl(TlError::UnexpectedConstructor {
            expected: 0x72a3_158c,
            actual,
        })),
    }
}

fn validate_connection(connection: &InitConnection<'_>) -> Result<(), ApiRequestError> {
    if connection.api_id <= 0 {
        return Err(ApiRequestError::InvalidApiId);
    }
    require_nonempty(connection.device_model, "device_model")?;
    require_nonempty(connection.system_version, "system_version")?;
    require_nonempty(connection.app_version, "app_version")?;
    require_nonempty(connection.system_lang_code, "system_lang_code")?;
    require_nonempty(connection.lang_code, "lang_code")?;
    Ok(())
}

fn validate_credentials(api_id: i32, api_hash: &str) -> Result<(), ApiRequestError> {
    if api_id <= 0 {
        return Err(ApiRequestError::InvalidApiId);
    }
    require_nonempty(api_hash, "api_hash")
}

fn validate_phone_number(phone_number: &str) -> Result<(), ApiRequestError> {
    if phone_number.len() < 8
        || !phone_number.starts_with('+')
        || !phone_number[1..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ApiRequestError::InvalidPhoneNumber);
    }
    Ok(())
}

fn require_nonempty(value: &str, field: &'static str) -> Result<(), ApiRequestError> {
    if value.trim().is_empty() {
        Err(ApiRequestError::EmptyField(field))
    } else {
        Ok(())
    }
}
