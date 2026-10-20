//! Unofficial client for Portugal's **e-fatura** portal
//! (<https://faturas.portaldasfinancas.gov.pt>).
//!
//! The AT offers no public API for consumers, so this crate drives the same
//! endpoints the website uses, logging in *as the user* through acesso.gov.pt,
//! including the SMS second factor.
//!
//! The SMS step can be split across processes: start a login, persist the
//! [`PendingLoginState`], and finish it whenever the code shows up (e.g. from a
//! webhook fed by a phone automation).
//!
//! ```no_run
//! use efatura::{Authenticator, Credentials, DocumentQuery, LoginOutcome};
//! use chrono::NaiveDate;
//!
//! # async fn run() -> efatura::Result<()> {
//! let credentials = Credentials::new("123456789", "password")?;
//! let client = match Authenticator::new().login(&credentials).await? {
//!     LoginOutcome::Authenticated(client) => client,
//!     LoginOutcome::SmsRequired(mut pending) => {
//!         // ... wait for the SMS ...
//!         pending.submit_code("123456").await?
//!     }
//! };
//!
//! // Read-only: list the invoices still waiting for an IRS sector.
//! let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
//! let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
//! for invoice in client.documents(&DocumentQuery::pending(from, to)).await? {
//!     println!("{} {:?} {:?}", invoice.id, invoice.issuer_name, invoice.total_cents);
//!     // To assign a sector (this changes your tax data on e-fatura):
//!     // client.categorize(invoice.key(), &efatura::Sector::Health).await?;
//! }
//! # Ok(()) }
//! ```
//!
//! # Splitting the SMS step across processes
//!
//! ```no_run
//! use efatura::{Authenticator, Config, Credentials, LoginOutcome, PendingLogin, PendingLoginState};
//!
//! # async fn run() -> efatura::Result<()> {
//! // Request A: log in; the AT sends the SMS. Store the state (it holds live
//! // session cookies: encrypt it at rest) and forget the `PendingLogin`.
//! let credentials = Credentials::new("123456789", "password")?;
//! let LoginOutcome::SmsRequired(pending) = Authenticator::new().login(&credentials).await? else {
//!     return Ok(()); // this account has no second factor
//! };
//! let stored: String = serde_json::to_string(&pending.state()?)?;
//!
//! // Request B, minutes later, maybe another process: the code arrives.
//! let state: PendingLoginState = serde_json::from_str(&stored)?;
//! let mut pending = PendingLogin::restore(state, Config::default())?;
//! let client = pending.submit_code("123456").await?;
//! let session = client.session()?; // store it too; `Client::from_session` resumes it
//! # let _ = session;
//! # Ok(()) }
//! ```
//!
//! # Never retry authentication failures
//!
//! The AT suspends credentials after a few wrong passwords or SMS codes.
//! [`Error::is_auth_failure`] is `true` for those errors; retrying them
//! automatically gets the user's account locked. [`Error::needs_login`] marks the
//! cases where starting a new login is the right answer.

mod auth;
mod client;
mod config;
mod error;
mod html;
mod model;
mod session;

pub use auth::{Authenticator, Credentials, LoginOutcome, PendingLogin, PendingLoginState};
pub use client::Client;
pub use config::Config;
pub use error::{Error, Result};
pub use model::{
    Document, DocumentDetail, DocumentId, DocumentKey, DocumentLine, DocumentQuery, DocumentState,
    Sector,
};
pub use session::{CookieJarState, SessionState};
