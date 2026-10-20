//! Cookie persistence and the shared HTTP plumbing.
//!
//! The whole point of this crate is that a login can be split across
//! processes: start it in one request, finish it (with the SMS code) in
//! another, and keep reusing the resulting session. All of that state lives in
//! cookies, so the jar must be serializable — including the non-persistent
//! session cookies (`JSESSIONID` & co.) that most jars drop on save.

use std::fmt;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use reqwest::header::{self, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::Config;
use crate::error::{Error, Result};

/// Opaque, serializable snapshot of the cookie jar.
///
/// Treat it like a password: it grants access to the user's tax data until it
/// expires. Encrypt it at rest.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CookieJarState(serde_json::Value);

impl fmt::Debug for CookieJarState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CookieJarState(<redacted>)")
    }
}

/// A logged-in e-fatura session that can be stored and restored later with
/// [`Client::from_session`](crate::Client::from_session).
///
/// AT sessions are short-lived (minutes to hours of inactivity, the exact value
/// is not documented). Expect [`Error::SessionExpired`] and be ready to log in
/// again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    /// NIF the session belongs to.
    pub nif: String,
    /// When the login completed.
    pub authenticated_at: DateTime<Utc>,
    pub(crate) cookies: CookieJarState,
}

/// `reqwest` cookie store backed by `cookie_store`, so it can be snapshotted.
pub(crate) struct Jar(RwLock<cookie_store::CookieStore>);

impl Jar {
    pub(crate) fn new() -> Self {
        Self(RwLock::new(cookie_store::CookieStore::new()))
    }

    pub(crate) fn from_state(state: &CookieJarState) -> Result<Self> {
        let raw = serde_json::to_vec(&state.0)?;
        let store = cookie_store::serde::json::load_all(raw.as_slice())
            .map_err(|e| Error::Cookies(e.to_string()))?;
        Ok(Self(RwLock::new(store)))
    }

    pub(crate) fn to_state(&self) -> Result<CookieJarState> {
        let store = self.0.read().expect("cookie jar lock poisoned");
        let mut buf = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut buf)
            .map_err(|e| Error::Cookies(e.to_string()))?;
        Ok(CookieJarState(serde_json::from_slice(&buf)?))
    }
}

impl reqwest::cookie::CookieStore for Jar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let cookies = cookie_headers.filter_map(|value| {
            let value = value.to_str().ok()?;
            cookie::Cookie::parse(value.to_owned()).ok()
        });
        self.0
            .write()
            .expect("cookie jar lock poisoned")
            .store_response_cookies(cookies, url);
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self.0.read().expect("cookie jar lock poisoned");
        let value = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            None
        } else {
            HeaderValue::from_str(&value).ok()
        }
    }
}

/// A fetched page: where we ended up after redirects, and its body.
#[derive(Debug)]
pub(crate) struct Page {
    pub url: Url,
    pub status: reqwest::StatusCode,
    pub body: String,
}

/// `reqwest::Client` + the jar it writes to + the endpoints.
#[derive(Clone)]
pub(crate) struct Http {
    pub client: reqwest::Client,
    pub jar: Arc<Jar>,
    pub config: Arc<Config>,
}

impl Http {
    pub(crate) fn new(config: Config, jar: Jar) -> Result<Self> {
        let jar = Arc::new(jar);
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("pt-PT,pt;q=0.9,en;q=0.8"),
        );
        let client = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .default_headers(headers)
            .cookie_provider(jar.clone())
            .timeout(config.timeout)
            .build()?;
        Ok(Self {
            client,
            jar,
            config: Arc::new(config),
        })
    }

    pub(crate) async fn get(&self, url: Url) -> Result<Page> {
        tracing::debug!(%url, "GET");
        let response = self
            .client
            .get(url)
            .header(header::ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8")
            .send()
            .await?;
        Self::page(response).await
    }

    pub(crate) async fn post_form(
        &self,
        url: Url,
        fields: &[(String, String)],
        referer: Option<&Url>,
    ) -> Result<Page> {
        tracing::debug!(%url, "POST form");
        let mut request = self
            .client
            .post(url.clone())
            .header(header::ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8")
            .header(header::ORIGIN, url.origin().ascii_serialization())
            .form(fields);
        if let Some(referer) = referer {
            request = request.header(header::REFERER, referer.as_str());
        }
        Self::page(request.send().await?).await
    }

    async fn page(response: reqwest::Response) -> Result<Page> {
        let url = response.url().clone();
        let status = response.status();
        let body = response.text().await?;
        tracing::debug!(%url, %status, len = body.len(), "response");
        Ok(Page { url, status, body })
    }
}
