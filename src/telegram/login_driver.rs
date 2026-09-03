//! Staged in-browser Telegram onboarding driver.
//!
//! [`TelegramLoginDriver`] owns the single human-facing login flow the `/_admin`
//! wizard drives: phone -> code -> (only if required) cloud-password/2FA. Only ONE
//! flow may be in flight per process; a second operator attempting `begin` while a
//! flow is mid-stage is told the current stage instead of being allowed to fight
//! over the shared Telegram client.
//!
//! The driver holds only the small retained state that must survive between HTTP
//! turns (the phone, the sign-in token, and the cloud-password token if 2FA is
//! pending). It borrows the live [`TelegramTransport`] per RPC call and routes
//! failures through the same classification as the CLI login so behaviour matches.
//!
//! Under the mock runtime (`TELEGRAM_TRANSPORT_RUNTIME=mock`) the driver runs a
//! staged simulation so the HTTP endpoints and their tests can drive all stages
//! deterministically. `TELEGRAM_MOCK_FORCE_2FA=1` makes the simulated account
//! require a cloud password so the 2FA branch is exercised too.

use crate::telegram::transport::TelegramTransport;
use grammers_client::client::{LoginToken, PasswordToken, SignInError};

/// The observable stage of the active onboarding flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStage {
    Idle,
    /// Waiting for the SMS/app confirmation code.
    Code,
    /// Telegram has demanded the cloud password (2FA).
    TwoFa,
    /// Login completed successfully.
    Authorized,
}

/// A step result that says where the flow now sits and whether it wants text.
#[derive(Debug, Clone)]
pub struct LoginStep {
    pub stage: LoginStage,
    pub message: String,
    pub needs_2fa: bool,
}

#[derive(Debug, Clone)]
pub struct LoginSnapshot {
    pub stage: LoginStage,
    pub owner: Option<String>,
    pub phone_provided: bool,
}

/// Structured, user-answerable error for the wizard endpoints.
#[derive(Debug, Clone)]
pub enum LoginDriverError {
    /// Another operator holds an in-flight flow at the given stage.
    Occupied {
        stage: LoginStage,
        owner: Option<String>,
    },
    MissingPhone,
    MissingCode,
    MissingPassword,
    InvalidCode,
    ExpiredCode,
    WrongPassword,
    SignUpRequired,
    /// An account/transport failure that should be surfaced verbatim.
    Unauthorized(String),
}

impl Default for TelegramLoginDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Retained state for a single, process-wide onboarding flow.
pub struct TelegramLoginDriver {
    stage: LoginStage,
    owner: Option<String>,
    phone: Option<String>,
    code_token: Option<LoginToken>,
    password_token: Option<PasswordToken>,
    // Mock-only: ask for a cloud password during the staged simulation.
    mock_force_2fa: bool,
}

impl TelegramLoginDriver {
    pub fn new() -> Self {
        let mock_force_2fa = std::env::var("TELEGRAM_MOCK_FORCE_2FA")
            .ok()
            .is_some_and(|value| value == "1");
        Self {
            stage: LoginStage::Idle,
            owner: None,
            phone: None,
            code_token: None,
            password_token: None,
            mock_force_2fa,
        }
    }

    pub fn snapshot(&self) -> LoginSnapshot {
        LoginSnapshot {
            stage: self.stage.clone(),
            owner: self.owner.clone(),
            phone_provided: self.phone.is_some(),
        }
    }

    /// True while a flow is mid-stage (Code or waiting for 2FA) and so locked
    /// to the operator who started it.
    pub fn is_busy(&self) -> bool {
        matches!(self.stage, LoginStage::Code | LoginStage::TwoFa)
    }

    pub fn is_authorized(&self) -> bool {
        self.stage == LoginStage::Authorized
    }

    /// Begin: record the phone and request a login code. Returns the stage we
    /// moved to, or an error if the flow is already owned by another operator.
    pub async fn begin(
        &mut self,
        transport: &TelegramTransport,
        phone: Option<String>,
        owner: &str,
    ) -> Result<LoginStep, LoginDriverError> {
        if self.is_busy() {
            return Err(LoginDriverError::Occupied {
                stage: self.stage.clone(),
                owner: self.owner.clone(),
            });
        }
        let phone = phone
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if transport.is_mock() {
            if phone.is_none() {
                return Err(LoginDriverError::MissingPhone);
            }
            self.reset_to(LoginStage::Code, phone, owner);
            return Ok(LoginStep {
                stage: LoginStage::Code,
                message: "confirmation code required".to_string(),
                needs_2fa: false,
            });
        }
        let client = transport
            .login_client()
            .map_err(|error| self.internal(error))?;
        if client
            .is_authorized()
            .await
            .map_err(|error| self.internal(error))?
        {
            self.authorize();
            return Ok(authorized_step());
        }
        let phone = phone.ok_or(LoginDriverError::MissingPhone)?;
        let token = transport
            .retry_invocation(|| client.request_login_code(&phone, transport.app_api_hash()))
            .await
            .map_err(|error| self.internal(error))?;
        self.phone = Some(phone.clone());
        self.code_token = Some(token);
        self.owner = Some(owner.to_string());
        self.stage = LoginStage::Code;
        Ok(LoginStep {
            stage: LoginStage::Code,
            message: "confirmation code sent; awaiting it".to_string(),
            needs_2fa: false,
        })
    }

    /// Submit the confirmation code. If the account requires 2FA the driver
    /// transitions to [`LoginStage::TwoFa`] and reports `needs_2fa`.
    pub async fn submit_code(
        &mut self,
        transport: &TelegramTransport,
        code: &str,
    ) -> Result<LoginStep, LoginDriverError> {
        let code = code.trim();
        if code.is_empty() {
            return Err(LoginDriverError::MissingCode);
        }
        if transport.is_mock() {
            if self.stage != LoginStage::Code {
                return Err(self.not_in_stage());
            }
            if self.mock_force_2fa {
                // Simulate Telegram demanding the cloud password.
                self.stage = LoginStage::TwoFa;
                return Ok(LoginStep {
                    stage: LoginStage::TwoFa,
                    message: "cloud password required".to_string(),
                    needs_2fa: true,
                });
            }
            self.authorize();
            return Ok(authorized_step());
        }
        let client = transport
            .login_client()
            .map_err(|error| self.internal(error))?;
        let token = self.code_token.take().ok_or_else(|| self.not_in_stage())?;
        match client.sign_in(&token, code).await {
            Ok(_user) => {
                self.authorize();
                Ok(authorized_step())
            }
            Err(SignInError::PasswordRequired(password_token)) => {
                self.password_token = Some(password_token);
                self.stage = LoginStage::TwoFa;
                Ok(LoginStep {
                    stage: LoginStage::TwoFa,
                    message: "cloud password required".to_string(),
                    needs_2fa: true,
                })
            }
            Err(SignInError::SignUpRequired) => Err(LoginDriverError::SignUpRequired),
            Err(SignInError::InvalidCode) => Err(LoginDriverError::InvalidCode),
            Err(SignInError::InvalidPassword(password_token)) => {
                // Fresh token on retry so the operator can try again.
                self.password_token = Some(password_token);
                Err(LoginDriverError::WrongPassword)
            }
            Err(SignInError::Other(error)) => {
                Err(LoginDriverError::Unauthorized(error.to_string()))
            }
        }
    }

    /// Submit the cloud password only when the flow is waiting for 2FA.
    pub async fn submit_password(
        &mut self,
        transport: &TelegramTransport,
        password: &str,
    ) -> Result<LoginStep, LoginDriverError> {
        if password.trim().is_empty() {
            return Err(LoginDriverError::MissingPassword);
        }
        if transport.is_mock() {
            if self.stage != LoginStage::TwoFa {
                return Err(self.not_in_stage());
            }
            self.authorize();
            return Ok(authorized_step());
        }
        let client = transport
            .login_client()
            .map_err(|error| self.internal(error))?;
        let password_token = self
            .password_token
            .take()
            .ok_or_else(|| self.not_in_stage())?;
        match client.check_password(password_token, password).await {
            Ok(_user) => {
                self.authorize();
                Ok(authorized_step())
            }
            Err(SignInError::InvalidPassword(password_token)) => {
                self.password_token = Some(password_token);
                Err(LoginDriverError::WrongPassword)
            }
            Err(SignInError::Other(error)) => {
                Err(LoginDriverError::Unauthorized(error.to_string()))
            }
            Err(SignInError::PasswordRequired(_))
            | Err(SignInError::InvalidCode)
            | Err(SignInError::SignUpRequired) => Err(LoginDriverError::WrongPassword),
        }
    }

    /// Drop any in-flight flow and its retained tokens.
    pub fn cancel(&mut self) {
        self.stage = LoginStage::Idle;
        self.owner = None;
        self.phone = None;
        self.code_token = None;
        self.password_token = None;
    }

    fn authorize(&mut self) {
        self.stage = LoginStage::Authorized;
        self.owner = None;
        self.code_token = None;
        self.password_token = None;
    }

    fn reset_to(&mut self, stage: LoginStage, phone: Option<String>, owner: &str) {
        self.stage = stage;
        self.phone = phone;
        self.owner = Some(owner.to_string());
        self.code_token = None;
        self.password_token = None;
    }

    fn internal(&self, error: impl std::fmt::Display) -> LoginDriverError {
        LoginDriverError::Unauthorized(error.to_string())
    }

    fn not_in_stage(&self) -> LoginDriverError {
        LoginDriverError::Unauthorized("flow is in the wrong stage for that action".to_string())
    }
}

fn authorized_step() -> LoginStep {
    LoginStep {
        stage: LoginStage::Authorized,
        message: "authorized".to_string(),
        needs_2fa: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_starts_idle_and_not_busy() {
        let driver = TelegramLoginDriver::new();
        assert_eq!(driver.snapshot().stage, LoginStage::Idle);
        assert!(!driver.is_busy());
    }

    #[test]
    fn cancel_returns_to_idle() {
        let mut driver = TelegramLoginDriver {
            stage: LoginStage::TwoFa,
            phone: Some("+15551234567".to_string()),
            owner: Some("alice".to_string()),
            ..Default::default()
        };
        assert!(driver.is_busy());
        driver.cancel();
        assert_eq!(driver.snapshot().stage, LoginStage::Idle);
        assert_eq!(driver.snapshot().owner, None);
        assert!(!driver.is_busy());
    }

    #[test]
    fn authorize_clears_retained_tokens_and_owner() {
        let mut driver = TelegramLoginDriver {
            stage: LoginStage::TwoFa,
            owner: Some("alice".to_string()),
            ..Default::default()
        };
        driver.authorize();
        assert_eq!(driver.snapshot().stage, LoginStage::Authorized);
        assert_eq!(driver.snapshot().owner, None);
        assert!(!driver.is_busy());
        assert!(driver.is_authorized());
    }

    #[test]
    fn a_mid_stage_flow_counts_as_busy_for_occupancy() {
        let driver = TelegramLoginDriver {
            stage: LoginStage::Code,
            owner: Some("bob".to_string()),
            ..Default::default()
        };
        assert!(driver.is_busy());
    }
}
