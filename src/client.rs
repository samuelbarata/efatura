//! Authenticated access to e-fatura (faturas.portaldasfinancas.gov.pt).

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use reqwest::header;
use serde::Deserialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::html;
use crate::model::{Document, DocumentDetail, DocumentKey, DocumentLine, DocumentQuery, Sector};
use crate::session::{Http, Jar, Page, SessionState};

/// A logged-in e-fatura session.
///
/// Get one from [`Authenticator::login`](crate::Authenticator::login) /
/// [`PendingLogin::submit_code`](crate::PendingLogin::submit_code), or restore
/// a saved one with [`Client::from_session`]. Cloning is cheap and clones share
/// the session.
#[derive(Clone)]
pub struct Client {
    http: Http,
    nif: String,
    authenticated_at: DateTime<Utc>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("nif", &self.nif)
            .field("authenticated_at", &self.authenticated_at)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    expired_session: bool,
    #[serde(default)]
    linhas: Vec<serde_json::Value>,
    #[serde(default)]
    total_elementos: Option<u64>,
    #[serde(default)]
    messages: Option<Messages>,
}

#[derive(Deserialize, Default)]
struct Messages {
    #[serde(default)]
    error: Vec<serde_json::Value>,
}

impl Client {
    pub(crate) fn new(http: Http, nif: String, authenticated_at: DateTime<Utc>) -> Self {
        Self {
            http,
            nif,
            authenticated_at,
        }
    }

    /// Restore a session saved with [`Client::session`]. Does not hit the
    /// network; call [`Client::is_authenticated`] to check it's still alive.
    pub fn from_session(state: SessionState, config: Config) -> Result<Self> {
        let http = Http::new(config, Jar::from_state(&state.cookies)?)?;
        Ok(Self::new(http, state.nif, state.authenticated_at))
    }

    /// Snapshot of the session (cookies). Contains credentials-equivalent
    /// secrets: encrypt it at rest.
    pub fn session(&self) -> Result<SessionState> {
        Ok(SessionState {
            nif: self.nif.clone(),
            authenticated_at: self.authenticated_at,
            cookies: self.http.jar.to_state()?,
        })
    }

    pub fn nif(&self) -> &str {
        &self.nif
    }

    pub fn authenticated_at(&self) -> DateTime<Utc> {
        self.authenticated_at
    }

    /// `true` if e-fatura still accepts this session (the buyer dashboard
    /// loads without bouncing to the login page).
    pub async fn is_authenticated(&self) -> Result<bool> {
        let page = self
            .http
            .get(self.http.config.faturas("painelAdquirente.action"))
            .await?;
        Ok(self.http.config.is_faturas(&page.url) && page.status.is_success())
    }

    /// Invoices where the user is the buyer, matching `query`.
    ///
    /// The portal silently truncates big result sets (a few hundred rows), so
    /// ranges are split per calendar year and bisected further whenever the
    /// portal reports more rows than it returned.
    pub async fn documents(&self, query: &DocumentQuery) -> Result<Vec<Document>> {
        if query.from > query.to {
            return Err(Error::InvalidInput("`from` is after `to`".into()));
        }

        let mut ranges = Vec::new();
        let mut start = query.from;
        loop {
            let year_end = NaiveDate::from_ymd_opt(start.year(), 12, 31).expect("valid date");
            let end = year_end.min(query.to);
            ranges.push((start, end));
            if end >= query.to {
                break;
            }
            start = end + Days::new(1);
        }
        ranges.reverse();

        let mut seen = HashSet::new();
        let mut documents = Vec::new();
        while let Some((from, to)) = ranges.pop() {
            let (batch, total) = self.documents_page(query, from, to).await?;
            if total.is_some_and(|total| total > batch.len() as u64) {
                if from == to {
                    return Err(Error::Portal(format!(
                        "more invoices on {from} than e-fatura returns in one response \
                         ({} of {})",
                        batch.len(),
                        total.unwrap_or_default()
                    )));
                }
                let half = (to - from).num_days() / 2;
                let mid = from + Days::new(half as u64);
                // Stack: push the later half first so results stay chronological.
                ranges.push((mid + Days::new(1), to));
                ranges.push((from, mid));
                continue;
            }
            documents.extend(batch.into_iter().filter(|doc| seen.insert(doc.id)));
        }
        Ok(documents)
    }

    /// Shortcut for invoices waiting for a sector.
    pub async fn pending_documents(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Document>> {
        self.documents(&DocumentQuery::pending(from, to)).await
    }

    async fn documents_page(
        &self,
        query: &DocumentQuery,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<(Vec<Document>, Option<u64>)> {
        let mut url = self
            .http
            .config
            .faturas("json/obterDocumentosAdquirente.action");
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("dataInicioFilter", &from.to_string())
                .append_pair("dataFimFilter", &to.to_string())
                .append_pair(
                    "ambitoAquisicaoFilter",
                    query.sector.as_ref().map_or("", Sector::code),
                );
            if let Some(state) = query.state {
                pairs.append_pair("estadoDocumentoFilter", state.code());
            }
        }

        let response = self
            .http
            .client
            .get(url)
            .header(
                header::ACCEPT,
                "application/json, text/javascript, */*; q=0.01",
            )
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await?;
        let page_url = response.url().clone();
        let body = response.text().await?;
        if self.http.config.is_acesso(&page_url) {
            return Err(Error::SessionExpired);
        }
        let list: ListResponse = serde_json::from_str(&body).map_err(|e| {
            if body.trim_start().starts_with('<') {
                // An HTML page instead of JSON is how expired sessions look
                // when the redirect is done client-side.
                Error::SessionExpired
            } else {
                Error::unexpected("listing documents", e)
            }
        })?;
        if list.expired_session {
            return Err(Error::SessionExpired);
        }
        if list.success == Some(false) {
            let message = list
                .messages
                .unwrap_or_default()
                .error
                .into_iter()
                .filter_map(|m| m.as_str().map(html::to_text))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::Portal(if message.is_empty() {
                "e-fatura refused to list documents".into()
            } else {
                message
            }));
        }

        let documents = list
            .linhas
            .into_iter()
            .map(Document::from_json)
            .collect::<Result<Vec<_>>>()?;
        Ok((documents, list.total_elementos))
    }

    /// Full detail of one invoice: lines, sector and the sectors it accepts.
    pub async fn document(&self, key: DocumentKey) -> Result<DocumentDetail> {
        let mut url = self
            .http
            .config
            .faturas("detalheDocumentoAdquirente.action");
        url.query_pairs_mut()
            .append_pair("idDocumento", &key.id.to_string())
            .append_pair("dataEmissaoDocumento", &key.issue_date.to_string());
        let page = self.http.get(url).await?;
        self.check_page(&page)?;
        parse_detail(&page.body, key)
    }

    /// Assign `sector` to an invoice (resolving it if it was pending).
    ///
    /// Mirrors the portal's "Alterar → Guardar" flow: the invoice lines are
    /// posted back untouched together with the new sector.
    pub async fn set_sector(&self, detail: &DocumentDetail, sector: &Sector) -> Result<()> {
        if !detail.available_sectors.is_empty() && !detail.available_sectors.contains(sector) {
            return Err(Error::InvalidInput(format!(
                "sector {sector} is not available for invoice {} (available: {})",
                detail.key.id,
                detail
                    .available_sectors
                    .iter()
                    .map(Sector::code)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let key = detail.key;
        let date = key.issue_date.to_string();
        let cancel_action = format!(
            "detalheDocumentoAdquirente.action?idDocumento={}&dataEmissaoDocumento={date}",
            key.id
        );
        let fields = vec![
            ("idDocumento".to_owned(), key.id.to_string()),
            ("dataEmissaoDocumento".to_owned(), date.clone()),
            ("dataEmissaoDocumentoOriginal".to_owned(), date),
            (
                "hashDocumento".to_owned(),
                detail.control_code.clone().unwrap_or_default(),
            ),
            (
                "linhasDocumento".to_owned(),
                serde_json::to_string(&detail.raw_lines)?,
            ),
            ("ambitoAquisicaoPend".to_owned(), sector.code().to_owned()),
            ("cancelarAction".to_owned(), cancel_action.clone()),
            ("mostraRemoverBtn".to_owned(), "false".to_owned()),
            ("showAdquirenteEdit".to_owned(), "true".to_owned()),
        ];
        let referer = self.http.config.faturas(&cancel_action);
        let page = self
            .http
            .post_form(
                self.http
                    .config
                    .faturas("resolverPendenciaAdquirente.action"),
                &fields,
                Some(&referer),
            )
            .await?;
        self.check_page(&page)
    }

    /// Fetch the invoice, assign `sector` and return the refreshed detail.
    ///
    /// Fails with [`Error::Portal`] if the portal accepted the request but the
    /// invoice ended up with a different sector.
    pub async fn categorize(&self, key: DocumentKey, sector: &Sector) -> Result<DocumentDetail> {
        let detail = self.document(key).await?;
        self.set_sector(&detail, sector).await?;
        let refreshed = self.document(key).await?;
        match &refreshed.sector {
            Some(current) if current != sector => Err(Error::Portal(format!(
                "invoice {} still has sector {current} after setting {sector}",
                key.id
            ))),
            _ => Ok(refreshed),
        }
    }

    /// Map an HTML response to an error if it's the login page or shows an
    /// error banner.
    fn check_page(&self, page: &Page) -> Result<()> {
        if self.http.config.is_acesso(&page.url) {
            return Err(Error::SessionExpired);
        }
        if !page.status.is_success() {
            return Err(Error::unexpected(
                "loading an e-fatura page",
                format!("HTTP {} at {}", page.status, page.url),
            ));
        }
        let errors = html::alerts(&page.body, "error");
        if !errors.is_empty() {
            return Err(Error::Portal(errors.join("; ")));
        }
        Ok(())
    }
}

fn parse_detail(body: &str, key: DocumentKey) -> Result<DocumentDetail> {
    const MARKER: &str = "dadosLinhasDocumento";
    let raw_lines = body
        .match_indices(MARKER)
        .find_map(|(idx, _)| {
            let rest = body[idx + MARKER.len()..].trim_start().strip_prefix('=')?;
            serde_json::Deserializer::from_str(rest.trim_start())
                .into_iter::<serde_json::Value>()
                .next()?
                .ok()
        })
        .filter(serde_json::Value::is_array)
        .ok_or_else(|| {
            Error::unexpected(
                "reading invoice detail",
                "invoice lines not found in the page",
            )
        })?;
    let lines = raw_lines
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .map(DocumentLine::from_json)
        .collect();

    let doc = html::parse(body);
    let options = html::select_all(&doc, "select#ambitoAquisicao option");
    let available_sectors = options
        .iter()
        .filter_map(|o| o.value().attr("value"))
        .filter_map(|v| v.parse::<Sector>().ok())
        .collect::<Vec<_>>();

    let selected = options
        .iter()
        .find(|o| o.value().attr("selected").is_some())
        .and_then(|o| o.value().attr("value"))
        .and_then(|v| v.parse::<Sector>().ok());
    // Read-only pages show the sector as a label only.
    let described = || {
        let label = html::select_all(&doc, "#ambitoAquisicaoDesc")
            .first()
            .map(|el| html::collapse_whitespace(&el.text().collect::<String>()))
            .filter(|label| !label.is_empty())?;
        options
            .iter()
            .find(|o| html::collapse_whitespace(&o.text().collect::<String>()) == label)
            .and_then(|o| o.value().attr("value"))
            .and_then(|v| v.parse::<Sector>().ok())
    };

    Ok(DocumentDetail {
        key,
        sector: selected.or_else(described),
        available_sectors,
        control_code: html::value_by_id(&doc, "hashAdquirenteDesc").filter(|c| !c.is_empty()),
        lines,
        raw_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DocumentId;

    const DETAIL: &str = include_str!("../tests/fixtures/detail.html");

    #[test]
    fn parses_detail_page() {
        let key = DocumentKey {
            id: DocumentId(100000000001),
            issue_date: NaiveDate::from_ymd_opt(2023, 10, 21).unwrap(),
        };
        let detail = parse_detail(DETAIL, key).unwrap();
        assert_eq!(detail.control_code.as_deref(), Some("CDUA"));
        assert_eq!(detail.lines.len(), 1);
        assert_eq!(detail.lines[0].total_cents, Some(2043));
        assert_eq!(detail.lines[0].vat_rate, Some(2300));
        assert_eq!(detail.sector, Some(Sector::Restaurants));
        assert_eq!(
            detail.available_sectors,
            vec![
                Sector::CarRepair,
                Sector::Restaurants,
                Sector::Health,
                Sector::Other
            ]
        );
        // Lines are posted back verbatim.
        assert_eq!(detail.raw_lines[0]["idLinha"], 100000000002u64);
    }
}
