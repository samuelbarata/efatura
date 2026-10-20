//! End-to-end flow against two mock servers standing in for acesso.gov.pt and
//! e-fatura. Checks the state machine and that a pending login survives being
//! serialized and restored, which is the whole point of the crate.

use chrono::NaiveDate;
use efatura::{
    Authenticator, Client, Config, Credentials, DocumentQuery, Error, LoginOutcome, PendingLogin,
    PendingLoginState, Sector, SessionState,
};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const LOGIN_INITIAL: &str = include_str!("fixtures/login_initial.html");
const LOGIN_SMS: &str = include_str!("fixtures/login_sms.html");
const LOGIN_SUCCESS: &str = include_str!("fixtures/login_success.html");
const LOGIN_WRONG_PASSWORD: &str = include_str!("fixtures/login_wrong_password.html");
const DETAIL: &str = include_str!("fixtures/detail.html");

fn html(body: impl Into<String>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into(), "text/html;charset=utf-8")
}

struct Servers {
    acesso: MockServer,
    faturas: MockServer,
}

impl Servers {
    async fn start() -> Self {
        let servers = Self {
            acesso: MockServer::start().await,
            faturas: MockServer::start().await,
        };
        Mock::given(method("GET"))
            .and(path("/jsp/loginRedirectForm.jsp"))
            .and(query_param("partID", "EFPF"))
            .respond_with(
                html(LOGIN_INITIAL)
                    .insert_header("Set-Cookie", "JSESSIONID=acesso1; Path=/; HttpOnly"),
            )
            .mount(&servers.acesso)
            .await;
        servers
    }

    fn config(&self) -> Config {
        Config {
            acesso_url: self.acesso.uri().parse().unwrap(),
            faturas_url: self.faturas.uri().parse().unwrap(),
            ..Config::default()
        }
    }

    /// Success page whose forward form points at the mock e-fatura.
    fn success_page(&self) -> String {
        LOGIN_SUCCESS.replace(
            "https://faturas.portaldasfinancas.gov.pt/painelAdquirente.action",
            &format!("{}/painelAdquirente.action", self.faturas.uri()),
        )
    }

    /// e-fatura only answers when the session cookie it handed out comes back.
    async fn mount_faturas(&self, expected_resolves: u64) {
        let login_redirect = format!(
            "{}/jsp/loginRedirectForm.jsp?path=painelAdquirente.action&partID=EFPF",
            self.acesso.uri()
        );
        Mock::given(method("POST"))
            .and(path("/painelAdquirente.action"))
            .and(body_string_contains("sign=SIGNATURE"))
            .respond_with(
                html("<html>painel</html>")
                    .insert_header("Set-Cookie", "SESSION_EFATURA=ok; Path=/; HttpOnly"),
            )
            .mount(&self.faturas)
            .await;
        Mock::given(method("GET"))
            .and(path("/painelAdquirente.action"))
            .respond_with(SessionGate {
                ok: html("<html>painel</html>"),
                redirect: login_redirect.clone(),
            })
            .mount(&self.faturas)
            .await;
        Mock::given(method("GET"))
            .and(path("/json/obterDocumentosAdquirente.action"))
            .and(query_param("estadoDocumentoFilter", "P"))
            .and(header("X-Requested-With", "XMLHttpRequest"))
            .respond_with(SessionGate {
                ok: ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success": true,
                    "totalElementos": 1,
                    "linhas": [{
                        "idDocumento": 100000000001u64,
                        "dataEmissaoDocumento": "2023-10-21",
                        "nifEmitente": 500000000,
                        "nomeEmitente": "LOJA EXEMPLO LDA",
                        "valorTotal": 2043,
                        "actividadeEmitente": "C03"
                    }]
                })),
                redirect: login_redirect.clone(),
            })
            .mount(&self.faturas)
            .await;
        Mock::given(method("GET"))
            .and(path("/detalheDocumentoAdquirente.action"))
            .and(query_param("idDocumento", "100000000001"))
            .respond_with(SessionGate {
                ok: html(DETAIL),
                redirect: login_redirect.clone(),
            })
            .mount(&self.faturas)
            .await;
        Mock::given(method("POST"))
            .and(path("/resolverPendenciaAdquirente.action"))
            .and(body_string_contains("ambitoAquisicaoPend=C03"))
            .and(body_string_contains("hashDocumento=CDUA"))
            .and(body_string_contains("idLinha"))
            .respond_with(SessionGate {
                ok: html(DETAIL),
                redirect: login_redirect,
            })
            .expect(expected_resolves)
            .mount(&self.faturas)
            .await;
    }
}

/// Serves `ok` when the e-fatura session cookie is present, otherwise bounces
/// to the login page like the real portal.
struct SessionGate {
    ok: ResponseTemplate,
    redirect: String,
}

impl Respond for SessionGate {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let has_session = request
            .headers
            .get("cookie")
            .and_then(|c| c.to_str().ok())
            .is_some_and(|c| c.contains("SESSION_EFATURA=ok"));
        if has_session {
            self.ok.clone()
        } else {
            ResponseTemplate::new(302).insert_header("Location", self.redirect.as_str())
        }
    }
}

fn credentials() -> Credentials {
    Credentials::new("123456789", "correct-horse").unwrap()
}

#[tokio::test]
async fn sms_login_survives_persistence_then_categorizes() {
    let servers = Servers::start().await;
    servers.mount_faturas(1).await;

    // Password step: needs the CSRF token and cookie from the login page.
    Mock::given(method("POST"))
        .and(path("/jsp/submissaoFormularioLogin"))
        .and(header("cookie", "JSESSIONID=acesso1"))
        .and(body_string_contains(
            "_csrf=00000000-0000-0000-0000-000000000001",
        ))
        .and(body_string_contains("username=123456789"))
        .and(body_string_contains("password=correct-horse"))
        .and(body_string_contains("selectedAuthMethod=N"))
        .and(body_string_contains("partID=EFPF"))
        .respond_with(html(LOGIN_SMS))
        .expect(1)
        .mount(&servers.acesso)
        .await;
    // SMS step: CSRF token from the 2FA page, nif and code.
    Mock::given(method("POST"))
        .and(path("/jsp/submissaoFormularioLogin"))
        .and(body_string_contains(
            "_csrf=00000000-0000-0000-0000-000000000002",
        ))
        .and(body_string_contains("codigoSms2Fa=654321"))
        .and(body_string_contains("nif=123456789"))
        .respond_with(html(servers.success_page()))
        .expect(1)
        .mount(&servers.acesso)
        .await;

    let outcome = Authenticator::with_config(servers.config())
        .login(&credentials())
        .await
        .unwrap();
    let LoginOutcome::SmsRequired(pending) = outcome else {
        panic!("expected SMS step, got {outcome:?}");
    };
    assert_eq!(pending.phone(), Some("*******321"));
    assert_eq!(pending.sends_remaining(), Some(3));

    // Round-trip through JSON as if stored in a database between requests.
    let stored = serde_json::to_string(&pending.state().unwrap()).unwrap();
    drop(pending);
    let state: PendingLoginState = serde_json::from_str(&stored).unwrap();
    let mut pending = PendingLogin::restore(state, servers.config()).unwrap();

    let client = pending.submit_code("654 321").await.unwrap();
    assert!(client.is_authenticated().await.unwrap());

    // Same for the authenticated session.
    let stored = serde_json::to_string(&client.session().unwrap()).unwrap();
    let state: SessionState = serde_json::from_str(&stored).unwrap();
    let client = Client::from_session(state, servers.config()).unwrap();

    let from = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
    let pending_docs = client
        .documents(&DocumentQuery::pending(from, to))
        .await
        .unwrap();
    assert_eq!(pending_docs.len(), 1);
    assert_eq!(pending_docs[0].total_cents, Some(2043));

    let detail = client
        .categorize(pending_docs[0].key(), &Sector::Restaurants)
        .await
        .unwrap();
    assert_eq!(detail.sector, Some(Sector::Restaurants));
}

#[tokio::test]
async fn wrong_password_is_an_auth_failure() {
    let servers = Servers::start().await;
    Mock::given(method("POST"))
        .and(path("/jsp/submissaoFormularioLogin"))
        .respond_with(html(LOGIN_WRONG_PASSWORD))
        .mount(&servers.acesso)
        .await;

    let err = Authenticator::with_config(servers.config())
        .login(&credentials())
        .await
        .unwrap_err();
    assert!(err.is_auth_failure(), "{err:?}");
    assert!(
        matches!(err, Error::InvalidCredentials(ref m) if m.contains("Tentativas restantes: 2"))
    );
}

#[tokio::test]
async fn wrong_sms_code_keeps_login_pending() {
    let servers = Servers::start().await;
    Mock::given(method("POST"))
        .and(path("/jsp/submissaoFormularioLogin"))
        .and(body_string_contains("password="))
        .respond_with(html(LOGIN_SMS))
        .mount(&servers.acesso)
        .await;
    let wrong = LOGIN_SMS
        .replace(
            "codigoErrado: parseBoolean('')",
            "codigoErrado: parseBoolean('true')",
        )
        .replace(
            "sendsRemaining: parseInt('3')",
            "sendsRemaining: parseInt('2')",
        )
        .replace("000000000002", "000000000003");
    Mock::given(method("POST"))
        .and(path("/jsp/submissaoFormularioLogin"))
        .and(body_string_contains("codigoSms2Fa="))
        .respond_with(html(wrong))
        .mount(&servers.acesso)
        .await;

    let LoginOutcome::SmsRequired(mut pending) = Authenticator::with_config(servers.config())
        .login(&credentials())
        .await
        .unwrap()
    else {
        panic!("expected SMS step");
    };
    let err = pending.submit_code("000000").await.unwrap_err();
    assert!(
        matches!(err, Error::InvalidSmsCode { remaining: Some(2) }),
        "{err:?}"
    );
    assert_eq!(pending.sends_remaining(), Some(2));
    // The refreshed CSRF token is what gets persisted for the next attempt.
    let state = serde_json::to_string(&pending.state().unwrap()).unwrap();
    assert!(state.contains("000000000003"));
}

#[tokio::test]
async fn expired_session_is_reported() {
    let servers = Servers::start().await;
    servers.mount_faturas(0).await;
    // A client with an empty jar has no e-fatura session.
    let state: SessionState = serde_json::from_value(serde_json::json!({
        "nif": "123456789",
        "authenticated_at": "2026-01-01T00:00:00Z",
        "cookies": []
    }))
    .unwrap();
    let client = Client::from_session(state, servers.config()).unwrap();
    assert!(!client.is_authenticated().await.unwrap());
    let from = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();
    let err = client.pending_documents(from, from).await.unwrap_err();
    assert!(matches!(err, Error::SessionExpired), "{err:?}");
}
