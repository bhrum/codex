use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodeDeliveryType {
    TelegramMessage,
    Sms,
    Call,
    FlashCall,
    MissedCall,
    Email,
    Fragment,
    FirebaseAndroid,
    FirebaseIos,
    Unknown,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum AuthorizationState {
    #[default]
    WaitParameters,
    WaitPhoneNumber,
    WaitCode {
        phone_number: String,
        delivery_type: CodeDeliveryType,
        code_length: u8,
        timeout_seconds: Option<u32>,
    },
    WaitOtherDeviceConfirmation {
        link: String,
    },
    WaitRegistration {
        terms_of_service_id: Option<String>,
    },
    WaitPassword {
        password_hint: String,
        has_recovery_email: bool,
        recovery_email_pattern: Option<String>,
    },
    Ready,
    LoggingOut,
    Closing,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum AuthCommand {
    ParametersAccepted,
    SubmitPhoneNumber {
        phone_number: String,
    },
    RequestQrCode,
    SubmitCode {
        code: String,
    },
    SubmitRegistration {
        first_name: String,
        last_name: String,
    },
    SubmitPassword {
        password: String,
    },
    RequestPasswordRecovery,
    Logout,
    Close,
    ApplyRemoteState {
        state: AuthorizationState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum AuthEvent {
    ParametersConfigured,
    PhoneNumberSubmitted { phone_number: String },
    QrCodeRequested,
    CodeSubmitted,
    RegistrationSubmitted,
    PasswordSubmitted,
    PasswordRecoveryRequested,
    LogoutRequested,
    CloseRequested,
    StateChanged { state: AuthorizationState },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuthError {
    #[error("command {command} is invalid while authorization state is {state:?}")]
    InvalidTransition {
        state: AuthorizationState,
        command: &'static str,
    },
    #[error("phone number must use international format and contain 6-20 digits")]
    InvalidPhoneNumber,
    #[error("authentication code must contain 1-16 digits or letters")]
    InvalidCode,
    #[error("first name is required and names are limited to 64 characters")]
    InvalidRegistrationName,
    #[error("password must not be empty")]
    InvalidPassword,
}

#[derive(Debug, Default, Clone)]
pub struct AuthorizationMachine {
    state: AuthorizationState,
}

impl AuthorizationMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(state: AuthorizationState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &AuthorizationState {
        &self.state
    }

    pub fn execute(&mut self, command: AuthCommand) -> Result<Vec<AuthEvent>, AuthError> {
        let events = self.decide(command)?;
        for event in &events {
            self.apply(event.clone());
        }
        Ok(events)
    }

    pub fn decide(&self, command: AuthCommand) -> Result<Vec<AuthEvent>, AuthError> {
        let event = match command {
            AuthCommand::ParametersAccepted
                if matches!(self.state, AuthorizationState::WaitParameters) =>
            {
                AuthEvent::ParametersConfigured
            }
            AuthCommand::SubmitPhoneNumber { phone_number }
                if matches!(self.state, AuthorizationState::WaitPhoneNumber) =>
            {
                let normalized = normalize_phone_number(&phone_number)?;
                AuthEvent::PhoneNumberSubmitted {
                    phone_number: normalized,
                }
            }
            AuthCommand::RequestQrCode
                if matches!(self.state, AuthorizationState::WaitPhoneNumber) =>
            {
                AuthEvent::QrCodeRequested
            }
            AuthCommand::SubmitCode { code }
                if matches!(self.state, AuthorizationState::WaitCode { .. }) =>
            {
                let code = code.trim();
                if code.is_empty()
                    || code.len() > 16
                    || !code
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
                {
                    return Err(AuthError::InvalidCode);
                }
                AuthEvent::CodeSubmitted
            }
            AuthCommand::SubmitRegistration {
                first_name,
                last_name,
            } if matches!(self.state, AuthorizationState::WaitRegistration { .. }) => {
                let first_name = first_name.trim();
                let last_name = last_name.trim();
                if first_name.is_empty() || first_name.len() > 64 || last_name.len() > 64 {
                    return Err(AuthError::InvalidRegistrationName);
                }
                AuthEvent::RegistrationSubmitted
            }
            AuthCommand::SubmitPassword { password }
                if matches!(self.state, AuthorizationState::WaitPassword { .. }) =>
            {
                if password.is_empty() {
                    return Err(AuthError::InvalidPassword);
                }
                AuthEvent::PasswordSubmitted
            }
            AuthCommand::RequestPasswordRecovery
                if matches!(self.state, AuthorizationState::WaitPassword { .. }) =>
            {
                AuthEvent::PasswordRecoveryRequested
            }
            AuthCommand::Logout if matches!(self.state, AuthorizationState::Ready) => {
                AuthEvent::LogoutRequested
            }
            AuthCommand::Close
                if !matches!(
                    self.state,
                    AuthorizationState::Closing | AuthorizationState::Closed
                ) =>
            {
                AuthEvent::CloseRequested
            }
            AuthCommand::ApplyRemoteState { state } => AuthEvent::StateChanged { state },
            other => {
                return Err(AuthError::InvalidTransition {
                    state: self.state.clone(),
                    command: command_name(&other),
                })
            }
        };
        Ok(vec![event])
    }

    pub fn apply(&mut self, event: AuthEvent) {
        match event {
            AuthEvent::ParametersConfigured => {
                self.state = AuthorizationState::WaitPhoneNumber;
            }
            AuthEvent::LogoutRequested => {
                self.state = AuthorizationState::LoggingOut;
            }
            AuthEvent::CloseRequested => {
                self.state = AuthorizationState::Closing;
            }
            AuthEvent::StateChanged { state } => {
                self.state = state;
            }
            AuthEvent::PhoneNumberSubmitted { .. }
            | AuthEvent::QrCodeRequested
            | AuthEvent::CodeSubmitted
            | AuthEvent::RegistrationSubmitted
            | AuthEvent::PasswordSubmitted
            | AuthEvent::PasswordRecoveryRequested => {
                // The remote authorization update is authoritative. Keeping the
                // current state prevents UI from assuming acceptance too early.
            }
        }
    }
}

fn normalize_phone_number(value: &str) -> Result<String, AuthError> {
    let trimmed = value.trim();
    let digits: String = trimmed
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if !trimmed.starts_with('+') || !(6..=20).contains(&digits.len()) {
        return Err(AuthError::InvalidPhoneNumber);
    }
    Ok(format!("+{digits}"))
}

fn command_name(command: &AuthCommand) -> &'static str {
    match command {
        AuthCommand::ParametersAccepted => "parametersAccepted",
        AuthCommand::SubmitPhoneNumber { .. } => "submitPhoneNumber",
        AuthCommand::RequestQrCode => "requestQrCode",
        AuthCommand::SubmitCode { .. } => "submitCode",
        AuthCommand::SubmitRegistration { .. } => "submitRegistration",
        AuthCommand::SubmitPassword { .. } => "submitPassword",
        AuthCommand::RequestPasswordRecovery => "requestPasswordRecovery",
        AuthCommand::Logout => "logout",
        AuthCommand::Close => "close",
        AuthCommand::ApplyRemoteState { .. } => "applyRemoteState",
    }
}
