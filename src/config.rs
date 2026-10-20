use std::time::Duration;

use url::Url;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Where the portals live and how to talk to them.
///
/// The defaults point at production. Overriding the URLs is mostly useful for
/// tests (see `tests/flow.rs`) or a recording proxy.
#[derive(Clone, Debug)]
pub struct Config {
    /// Base URL of the AT single sign-on (`https://www.acesso.gov.pt/`).
    pub acesso_url: Url,
    /// Base URL of e-fatura (`https://faturas.portaldasfinancas.gov.pt/`).
    pub faturas_url: Url,
    /// `User-Agent` sent with every request.
    pub user_agent: String,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            acesso_url: Url::parse("https://www.acesso.gov.pt/").expect("valid url"),
            faturas_url: Url::parse("https://faturas.portaldasfinancas.gov.pt/")
                .expect("valid url"),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(60),
        }
    }
}

impl Config {
    pub(crate) fn acesso(&self, path: &str) -> Url {
        self.acesso_url.join(path).expect("static path joins")
    }

    pub(crate) fn faturas(&self, path: &str) -> Url {
        self.faturas_url.join(path).expect("static path joins")
    }

    pub(crate) fn is_acesso(&self, url: &Url) -> bool {
        url.origin() == self.acesso_url.origin()
    }

    pub(crate) fn is_faturas(&self, url: &Url) -> bool {
        url.origin() == self.faturas_url.origin()
    }
}
