use std::fmt;

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while talking to the AT portals.
///
/// Authentication failures are deliberately split into distinct variants: the
/// AT suspends credentials after a handful of bad passwords or SMS codes, so
/// callers must **never** blindly retry on [`Error::InvalidCredentials`],
/// [`Error::InvalidSmsCode`] or [`Error::CredentialsSuspended`]. Use
/// [`Error::is_auth_failure`] to tell them apart from transient errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// NIF/password rejected by acesso.gov.pt. The message is the one shown by the AT.
    #[error("invalid credentials: {0}")]
    InvalidCredentials(String),

    /// The SMS code was wrong. The AT counts these attempts.
    #[error("invalid SMS code ({remaining:?} attempts remaining)")]
    InvalidSmsCode { remaining: Option<u32> },

    /// The SMS code expired; request a new one with
    /// [`PendingLogin::resend_code`](crate::PendingLogin::resend_code).
    #[error("the SMS code has expired, request a new one")]
    SmsCodeExpired,

    /// The AT failed to deliver the SMS (their side).
    #[error("the AT failed to send the SMS code")]
    SmsDeliveryFailed,

    /// Too many failed attempts: the AT suspended the credentials (usually for one hour).
    #[error("credentials suspended by the AT: {0}")]
    CredentialsSuspended(String),

    /// The AT wants the user to do something interactively in a browser before
    /// the login can proceed (e.g. confirm contact details).
    #[error("the AT requires interactive action in a browser: {0}")]
    InteractionRequired(String),

    /// The e-fatura session is gone (expired or never established). Log in again.
    #[error("the e-fatura session has expired")]
    SessionExpired,

    /// e-fatura reported a business error (e.g. "invalid category for this invoice").
    #[error("e-fatura error: {0}")]
    Portal(String),

    /// The portal answered with something this crate does not understand. Most
    /// likely the AT changed their website.
    #[error("unexpected response while {context}: {detail}")]
    UnexpectedResponse {
        context: &'static str,
        detail: String,
    },

    /// Bad input provided by the caller.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Transport error.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// JSON (de)serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Failed to (de)serialize the cookie jar.
    #[error("cookie jar error: {0}")]
    Cookies(String),
}

impl Error {
    /// `true` for errors that mean "the user's credentials or SMS code were
    /// rejected". Retrying these automatically will get the account suspended.
    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            Error::InvalidCredentials(_)
                | Error::InvalidSmsCode { .. }
                | Error::CredentialsSuspended(_)
        )
    }

    /// `true` when a fresh login (and therefore probably a new SMS code) is needed.
    pub fn needs_login(&self) -> bool {
        matches!(self, Error::SessionExpired | Error::SmsCodeExpired)
    }

    pub(crate) fn unexpected(context: &'static str, detail: impl fmt::Display) -> Self {
        Error::UnexpectedResponse {
            context,
            detail: detail.to_string(),
        }
    }
}
