//! Parser for the acesso.gov.pt login page.
//!
//! The page is a React app bootstrapped from an inline `model` object whose
//! values are templated server-side, e.g.
//!
//! ```js
//! _csrf: { parameterName: `_csrf`, token: `ae67...` },
//! authVersion: stringOrNull('1'),
//! is2FA: parseBoolean('true'),
//! phone: stringOrNull('*******123'),
//! sendsRemaining: parseInt('3') || null,
//! ```
//!
//! The same template renders the initial form, the "wrong password" page, the
//! SMS second-factor modal and the "authenticated, redirecting" page, so one
//! parser covers every step of the flow.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::html;

macro_rules! re {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

re!(CSRF_PARAM, r"parameterName:\s*`([^`]+)`");
re!(CSRF_TOKEN, r"token:\s*`([^`]+)`");
re!(BOOL, r"(\w+):\s*parseBoolean\('([^']*)'\)");
re!(STRING, r"(\w+):\s*stringOrNull\('([^']*)'\)");
re!(INT, r"(\w+):\s*parseInt\('([^']*)'\)");
re!(GLOBAL_ERROR, r"(?s)globalError:\s*`(.*?)`");

fn json_script(html: &str, id: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?s)<script id="{}" type="application/json">(.*?)</script>"#,
        regex::escape(id)
    ))
    .ok()?;
    let body = re.captures(html)?.get(1)?.as_str().trim();
    (!body.is_empty()).then(|| body.to_owned())
}

#[derive(Debug, Deserialize)]
struct FieldError {
    field: Option<String>,
    #[serde(rename = "errorMsg")]
    error_msg: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SisPart {
    #[serde(rename = "fPermissaoPartilhaDados")]
    share_personal_data: Option<String>,
}

/// Everything we need from one rendering of the login page.
#[derive(Debug, Clone)]
pub(crate) struct LoginPage {
    pub csrf_param: String,
    pub csrf_token: String,
    /// Hidden attributes echoed back on every submit (`path`, `partID`, ...).
    pub attributes: BTreeMap<String, String>,
    pub auth_version: Option<String>,
    /// Relative or absolute URL the forms post to.
    pub url_login: Option<String>,
    /// The service provider asks for consent to share NIF + name.
    pub share_personal_data: bool,
    flags: HashMap<String, bool>,
    strings: HashMap<String, String>,
    ints: HashMap<String, u32>,
    /// `(field, message)` of a validation error, e.g. wrong password.
    pub field_error: Option<(Option<String>, String)>,
    pub global_error: Option<String>,
}

impl LoginPage {
    /// Parse the page; `None` if `html` is not the acesso.gov.pt login app.
    pub(crate) fn parse(html: &str) -> Option<Self> {
        if !html.contains("startLogin") && !html.contains(r#"id="data-attributes""#) {
            return None;
        }
        let csrf_token = CSRF_TOKEN.captures(html)?.get(1)?.as_str().to_owned();
        let csrf_param = CSRF_PARAM
            .captures(html)
            .and_then(|c| c.get(1))
            .map_or_else(|| "_csrf".to_owned(), |m| m.as_str().to_owned());

        let attributes = json_script(html, "data-attributes")
            .and_then(|json| {
                serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&json).ok()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| match v {
                serde_json::Value::String(s) => Some((k, s)),
                serde_json::Value::Number(n) => Some((k, n.to_string())),
                serde_json::Value::Bool(b) => Some((k, b.to_string())),
                _ => None,
            })
            .collect();

        let sis_part: SisPart = json_script(html, "data-sispart")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        let flags = BOOL
            .captures_iter(html)
            .map(|c| (c[1].to_owned(), c[2].eq_ignore_ascii_case("true")))
            .collect();
        let strings: HashMap<String, String> = STRING
            .captures_iter(html)
            .filter(|c| !c[2].is_empty())
            .map(|c| (c[1].to_owned(), c[2].to_owned()))
            .collect();
        let ints = INT
            .captures_iter(html)
            .filter_map(|c| Some((c[1].to_owned(), c[2].trim().parse().ok()?)))
            .collect();

        let field_error = json_script(html, "data-field-error")
            .and_then(|json| serde_json::from_str::<FieldError>(&json).ok())
            .and_then(|e| Some((e.field, html::to_text(&e.error_msg?))))
            .filter(|(_, msg)| !msg.is_empty());
        let global_error = GLOBAL_ERROR
            .captures(html)
            .map(|c| html::to_text(&c[1]))
            .filter(|s| !s.is_empty());

        Some(Self {
            csrf_param,
            csrf_token,
            attributes,
            auth_version: strings.get("authVersion").cloned(),
            url_login: strings.get("urlLogin").cloned(),
            share_personal_data: sis_part.share_personal_data.as_deref() == Some("V"),
            flags,
            strings,
            ints,
            field_error,
            global_error,
        })
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    pub(crate) fn string(&self, name: &str) -> Option<&str> {
        self.strings.get(name).map(String::as_str)
    }

    pub(crate) fn int(&self, name: &str) -> Option<u32> {
        self.ints.get(name).copied()
    }

    pub(crate) fn is_two_factor(&self) -> bool {
        self.flag("is2FA")
    }

    pub(crate) fn error_message(&self) -> Option<String> {
        self.global_error
            .clone()
            .or_else(|| self.field_error.as_ref().map(|(_, msg)| msg.clone()))
    }

    /// Hidden fields every submission of the login app carries.
    pub(crate) fn base_fields(&self) -> Vec<(String, String)> {
        let mut fields = vec![(self.csrf_param.clone(), self.csrf_token.clone())];
        if !self.attributes.contains_key("authVersion")
            && let Some(version) = &self.auth_version
        {
            fields.push(("authVersion".into(), version.clone()));
        }
        if self.share_personal_data {
            fields.push(("envioDadosPessoais".into(), "on".into()));
        }
        fields.extend(self.attributes.iter().map(|(k, v)| (k.clone(), v.clone())));
        fields.push(("selectedAuthMethod".into(), "N".into()));
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const INITIAL: &str = include_str!("../../tests/fixtures/login_initial.html");
    pub(crate) const SMS: &str = include_str!("../../tests/fixtures/login_sms.html");
    pub(crate) const WRONG_PASSWORD: &str =
        include_str!("../../tests/fixtures/login_wrong_password.html");

    #[test]
    fn parses_initial_page() {
        let page = LoginPage::parse(INITIAL).unwrap();
        assert_eq!(page.csrf_param, "_csrf");
        assert_eq!(page.csrf_token, "00000000-0000-0000-0000-000000000001");
        assert_eq!(page.attributes.get("partID").unwrap(), "EFPF");
        assert_eq!(
            page.attributes.get("path").unwrap(),
            "painelAdquirente.action"
        );
        assert_eq!(page.auth_version.as_deref(), Some("1"));
        assert_eq!(page.url_login.as_deref(), Some("submissaoFormularioLogin"));
        assert!(!page.share_personal_data);
        assert!(!page.is_two_factor());
        assert!(page.error_message().is_none());
        assert_eq!(
            page.base_fields(),
            vec![
                (
                    "_csrf".to_owned(),
                    "00000000-0000-0000-0000-000000000001".to_owned()
                ),
                ("authVersion".to_owned(), "1".to_owned()),
                ("partID".to_owned(), "EFPF".to_owned()),
                ("path".to_owned(), "painelAdquirente.action".to_owned()),
                ("selectedAuthMethod".to_owned(), "N".to_owned()),
            ]
        );
    }

    #[test]
    fn parses_sms_page() {
        let page = LoginPage::parse(SMS).unwrap();
        assert!(page.is_two_factor());
        assert_eq!(page.csrf_token, "00000000-0000-0000-0000-000000000002");
        assert_eq!(page.string("nifIn2FA"), Some("123456789"));
        assert_eq!(page.string("phone"), Some("*******321"));
        assert_eq!(page.int("sendsRemaining"), Some(3));
        assert!(!page.flag("codigoErrado"));
    }

    #[test]
    fn parses_wrong_password() {
        let page = LoginPage::parse(WRONG_PASSWORD).unwrap();
        assert!(!page.is_two_factor());
        assert_eq!(
            page.error_message().as_deref(),
            Some("NIF ou Senha inválidos. Tentativas restantes: 2")
        );
    }

    #[test]
    fn rejects_other_pages() {
        assert!(LoginPage::parse("<html><body>e-fatura</body></html>").is_none());
    }
}
