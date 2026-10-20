//! Logging in as the user through acesso.gov.pt (NIF + password + SMS code).
//!
//! ```text
//!  Authenticator::login ──► LoginOutcome::Authenticated(Client)       (no 2FA on the account)
//!          │
//!          └──────────────► LoginOutcome::SmsRequired(PendingLogin)   (SMS sent to the user)
//!                                   │  .state()  ──► persist (DB, Redis, file...)
//!                                   │  PendingLogin::restore(state)  ◄── later, maybe in another process
//!                                   └─ .submit_code("123456") ──► Client
//! ```

mod login_page;

use std::fmt;

use chrono::{DateTime, Utc};
use reqwest::header;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::client::Client;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::html;
use crate::session::{CookieJarState, Http, Jar, Page};

pub(crate) use login_page::LoginPage;

/// e-fatura's identifier in the AT single sign-on.
const PART_ID: &str = "EFPF";
/// Where e-fatura sends you after logging in.
const LANDING_PATH: &str = "painelAdquirente.action";

/// NIF + Portal das Finanças password. `Debug` never prints the password.
#[derive(Clone)]
pub struct Credentials {
    nif: String,
    password: String,
}

impl Credentials {
    /// Validates the shape the AT login form enforces (9-digit NIF, 8–16 char
    /// password) so we never burn one of the few allowed attempts on garbage.
    pub fn new(nif: impl Into<String>, password: impl Into<String>) -> Result<Self> {
        let nif = nif.into().trim().to_owned();
        let password = password.into();
        if nif.len() != 9 || !nif.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::InvalidInput("NIF must have 9 digits".into()));
        }
        if !(8..=16).contains(&password.chars().count()) {
            return Err(Error::InvalidInput(
                "AT passwords have between 8 and 16 characters".into(),
            ));
        }
        Ok(Self { nif, password })
    }

    pub fn nif(&self) -> &str {
        &self.nif
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("nif", &self.nif)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Result of submitting NIF + password.
#[derive(Debug)]
pub enum LoginOutcome {
    /// Logged in straight away (the account has no second factor).
    Authenticated(Client),
    /// The AT sent an SMS code; finish with [`PendingLogin::submit_code`].
    SmsRequired(PendingLogin),
}

/// Starts logins. Cheap to create; holds no state besides the [`Config`].
#[derive(Clone, Debug, Default)]
pub struct Authenticator {
    config: Config,
}

impl Authenticator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// Submit NIF + password. On accounts with 2FA this triggers the SMS.
    pub async fn login(&self, credentials: &Credentials) -> Result<LoginOutcome> {
        let http = Http::new(self.config.clone(), Jar::new())?;

        let mut entry = self.config.acesso("jsp/loginRedirectForm.jsp");
        entry
            .query_pairs_mut()
            .append_pair("path", LANDING_PATH)
            .append_pair("partID", PART_ID);
        let page = http.get(entry).await?;
        let login = LoginPage::parse(&page.body).ok_or_else(|| {
            Error::unexpected(
                "loading the login page",
                format!("no login form at {}", page.url),
            )
        })?;

        let mut fields = login.base_fields();
        fields.push(("username".into(), credentials.nif.clone()));
        fields.push(("password".into(), credentials.password.clone()));
        let submit_url = submit_url(&page.url, &login)?;
        let response = http.post_form(submit_url, &fields, Some(&page.url)).await?;

        if let Some(login) = still_on_login(&http, &response) {
            if login.is_two_factor() {
                check_two_factor_page(&login)?;
                let pending = PendingLogin::new(http, credentials.nif.clone(), &login, &response)?;
                return Ok(LoginOutcome::SmsRequired(pending));
            }
            fail_from_login_page(&login, "submitting credentials")?;
        }

        let client = finish_sso(http, credentials.nif.clone(), response).await?;
        Ok(LoginOutcome::Authenticated(client))
    }
}

/// A login waiting for the SMS code.
///
/// The AT keeps the half-done login in a server-side session tied to cookies,
/// so this can be persisted with [`PendingLogin::state`] and picked up later
/// (e.g. when the code arrives through a webhook) with [`PendingLogin::restore`].
/// SMS codes expire after a few minutes.
pub struct PendingLogin {
    http: Http,
    context: Box<TwoFactorContext>,
}

impl fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingLogin")
            .field("nif", &self.context.nif)
            .field("phone", &self.context.phone)
            .field("sends_remaining", &self.context.sends_remaining)
            .finish_non_exhaustive()
    }
}

/// Serializable snapshot of a [`PendingLogin`]. Contains session cookies:
/// store it encrypted and throw it away once the login completes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingLoginState {
    #[serde(flatten)]
    context: TwoFactorContext,
    cookies: CookieJarState,
}

impl PendingLoginState {
    pub fn nif(&self) -> &str {
        &self.context.nif
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.context.started_at
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TwoFactorContext {
    nif: String,
    /// Masked phone number as shown by the AT, e.g. `*******321`.
    phone: Option<String>,
    sends_remaining: Option<u32>,
    started_at: DateTime<Utc>,
    submit_url: Url,
    referer: Url,
    csrf_token: String,
    /// Hidden fields of `form2FA`, minus the code.
    fields: Vec<(String, String)>,
    /// Query used by the resend/cancel endpoints.
    params: Vec<(String, String)>,
}

impl PendingLogin {
    fn new(http: Http, nif: String, login: &LoginPage, page: &Page) -> Result<Self> {
        let context = TwoFactorContext {
            phone: login.string("phone").map(str::to_owned),
            sends_remaining: login.int("sendsRemaining"),
            started_at: Utc::now(),
            submit_url: submit_url(&page.url, login)?,
            referer: page.url.clone(),
            csrf_token: String::new(),
            fields: Vec::new(),
            params: Vec::new(),
            nif,
        };
        let mut pending = Self {
            http,
            context: Box::new(context),
        };
        pending.update_from(login);
        Ok(pending)
    }

    /// Recreate a pending login saved with [`PendingLogin::state`].
    pub fn restore(state: PendingLoginState, config: Config) -> Result<Self> {
        let http = Http::new(config, Jar::from_state(&state.cookies)?)?;
        Ok(Self {
            http,
            context: Box::new(state.context),
        })
    }

    /// Snapshot to persist. Take a fresh one after every call on `self`: the
    /// CSRF token and cookies change as the flow advances.
    pub fn state(&self) -> Result<PendingLoginState> {
        Ok(PendingLoginState {
            context: (*self.context).clone(),
            cookies: self.http.jar.to_state()?,
        })
    }

    pub fn nif(&self) -> &str {
        &self.context.nif
    }

    /// Masked phone number the code was sent to, e.g. `*******321`.
    pub fn phone(&self) -> Option<&str> {
        self.context.phone.as_deref()
    }

    /// How many more codes the AT is willing to send / accept.
    pub fn sends_remaining(&self) -> Option<u32> {
        self.context.sends_remaining
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.context.started_at
    }

    /// Finish the login with the 6-digit code from the SMS.
    ///
    /// On [`Error::InvalidSmsCode`] the pending login stays usable (persist
    /// [`state`](Self::state) again and retry with another code). Do not retry
    /// automatically: the AT suspends the credentials after a few failures.
    pub async fn submit_code(&mut self, code: &str) -> Result<Client> {
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::InvalidInput("the SMS code has 6 digits".into()));
        }

        let mut fields = self.context.fields.clone();
        fields.push(("codigoSms2Fa".into(), code));
        let response = self
            .http
            .post_form(
                self.context.submit_url.clone(),
                &fields,
                Some(&self.context.referer),
            )
            .await?;

        if let Some(login) = still_on_login(&self.http, &response) {
            if login.is_two_factor() {
                self.update_from(&login);
                check_two_factor_page(&login)?;
                return Err(Error::unexpected(
                    "submitting the SMS code",
                    "the second factor page came back without an error",
                ));
            }
            if !login.flag("loginSuccess") {
                // Back to the plain login form: the half-done login is gone.
                if let Some(message) = login.error_message() {
                    return Err(classify_error(message));
                }
                return Err(Error::SmsCodeExpired);
            }
        }

        finish_sso(self.http.clone(), self.context.nif.clone(), response).await
    }

    /// Ask the AT for a new SMS code. Returns how many more sends are allowed.
    pub async fn resend_code(&mut self) -> Result<Option<u32>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Resend {
            #[serde(default)]
            sms_failed: bool,
            #[serde(default)]
            new_code: bool,
            sends_remaining: Option<u32>,
        }

        let mut url = self.endpoint("reenviarCodigo");
        url.query_pairs_mut().append_pair("isLogin", "true");
        let response = self
            .http
            .client
            .post(url)
            .header(header::ACCEPT, "application/json")
            .header("X-CSRF-TOKEN", &self.context.csrf_token)
            .header(header::REFERER, self.context.referer.as_str())
            .send()
            .await?;
        if !response.status().is_success() {
            // The AT answers errors with `{redirectUrl}` back to the login form.
            return Err(Error::SessionExpired);
        }
        let resend: Resend = response.json().await?;
        if resend.sms_failed {
            return Err(Error::SmsDeliveryFailed);
        }
        if !resend.new_code {
            return Err(Error::CredentialsSuspended(
                "too many SMS codes requested".into(),
            ));
        }
        self.context.sends_remaining = resend.sends_remaining;
        Ok(resend.sends_remaining)
    }

    /// Abort the login on the AT side. Best effort.
    pub async fn cancel(self) -> Result<()> {
        self.http
            .client
            .post(self.endpoint("cancelarLogin2Fa"))
            .header(header::ACCEPT, "application/json")
            .header("X-CSRF-TOKEN", &self.context.csrf_token)
            .send()
            .await?;
        Ok(())
    }

    /// acesso.gov.pt JSON endpoint with the `partID`/`path` query the login app sends.
    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.http.config.acesso(path);
        url.query_pairs_mut().extend_pairs(&self.context.params);
        url
    }

    fn update_from(&mut self, login: &LoginPage) {
        let ctx = &mut self.context;
        if let Some(phone) = login.string("phone") {
            ctx.phone = Some(phone.to_owned());
        }
        if let Some(remaining) = login.int("sendsRemaining") {
            ctx.sends_remaining = Some(remaining);
        }
        ctx.csrf_token = login.csrf_token.clone();
        ctx.fields = login.base_fields();
        ctx.fields.push((
            "nif".into(),
            login.string("nifIn2FA").unwrap_or(&ctx.nif).to_owned(),
        ));
        ctx.params = vec![(
            "partID".into(),
            login
                .attributes
                .get("partID")
                .map_or(PART_ID, String::as_str)
                .to_owned(),
        )];
        for key in ["issuer", "path"] {
            if let Some(value) = login.attributes.get(key) {
                ctx.params.push((key.into(), value.clone()));
            }
        }
    }
}

fn submit_url(page_url: &Url, login: &LoginPage) -> Result<Url> {
    let target = login
        .url_login
        .as_deref()
        .unwrap_or("submissaoFormularioLogin");
    page_url
        .join(target)
        .map_err(|e| Error::unexpected("resolving the login form action", e))
}

/// The login page, if `page` is still the acesso.gov.pt login app without a
/// pending redirect to the service.
fn still_on_login(http: &Http, page: &Page) -> Option<LoginPage> {
    if !http.config.is_acesso(&page.url) {
        return None;
    }
    LoginPage::parse(&page.body)
}

fn check_two_factor_page(login: &LoginPage) -> Result<()> {
    if login.flag("userSuspenso") {
        return Err(Error::CredentialsSuspended(
            "too many wrong SMS codes; the AT suspends the credentials for one hour".into(),
        ));
    }
    if login.flag("codeExpired2Fa") {
        return Err(Error::SmsCodeExpired);
    }
    if login.flag("codigoErrado") {
        return Err(Error::InvalidSmsCode {
            remaining: login.int("sendsRemaining"),
        });
    }
    if login.flag("smsFailed") {
        return Err(Error::SmsDeliveryFailed);
    }
    Ok(())
}

/// Turn a login page that should have moved on into the right error.
fn fail_from_login_page(login: &LoginPage, context: &'static str) -> Result<()> {
    if login.flag("loginSuccess") {
        return Ok(());
    }
    if let Some(message) = login.error_message() {
        return Err(classify_error(message));
    }
    Err(Error::unexpected(
        context,
        "the login page came back without an error message",
    ))
}

fn classify_error(message: String) -> Error {
    let lower = message.to_lowercase();
    if lower.contains("suspens") || lower.contains("bloque") {
        Error::CredentialsSuspended(message)
    } else {
        Error::InvalidCredentials(message)
    }
}

/// Follow the "authenticated, redirecting..." hops from acesso.gov.pt to
/// e-fatura and check we really ended up with a session.
async fn finish_sso(http: Http, nif: String, mut page: Page) -> Result<Client> {
    for _ in 0..6 {
        if !http.config.is_acesso(&page.url) {
            break;
        }
        let login = LoginPage::parse(&page.body);
        if login
            .as_ref()
            .is_some_and(|l| l.flag("showFiabilizacaoModal"))
        {
            tracing::info!("AT asks to confirm contact details; skipping");
        }
        // The success page carries `forwardParticipantForm`, which the React
        // app auto-submits. Other intermediate pages are plain auto-submit forms.
        let form =
            html::find_form(&page.body, &page.url, Some("forwardParticipantForm")).or_else(|| {
                login
                    .is_none()
                    .then(|| html::find_form(&page.body, &page.url, None))
                    .flatten()
            });
        if let Some(form) = form {
            page = if form.post {
                http.post_form(form.action, &form.fields, Some(&page.url))
                    .await?
            } else {
                let mut url = form.action;
                url.query_pairs_mut().extend_pairs(&form.fields);
                http.get(url).await?
            };
            continue;
        }
        if let Some(url) = html::find_js_redirect(&page.body, &page.url) {
            page = http.get(url).await?;
            continue;
        }
        break;
    }

    let client = Client::new(http, nif, Utc::now());
    if client.is_authenticated().await? {
        return Ok(client);
    }
    if let Some(login) = LoginPage::parse(&page.body)
        && let Some(message) = login.error_message()
    {
        return Err(classify_error(message));
    }
    Err(Error::unexpected(
        "completing the single sign-on",
        format!(
            "no e-fatura session after the redirects (stopped at {})",
            page.url
        ),
    ))
}
