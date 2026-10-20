//! Data types for e-fatura documents.
//!
//! Amounts are kept the way the portal sends them: integer **cents** (and VAT
//! rates in hundredths of a percent, so `2300` = 23%). No floats.

use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::Error;
use crate::html;

/// e-fatura's identifier for an invoice (`idDocumento`).
///
/// Unique across the portal and stable, so it's the key to map an invoice to
/// a bank transaction. Most operations also need the issue date, see
/// [`DocumentKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(#[serde(deserialize_with = "u64_or_string")] pub u64);

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for DocumentId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse()
            .map(Self)
            .map_err(|_| Error::InvalidInput(format!("invalid document id {s:?}")))
    }
}

/// What the portal needs to address one invoice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocumentKey {
    pub id: DocumentId,
    pub issue_date: NaiveDate,
}

/// Sector ("atividade de realização da aquisição") an expense is assigned to.
///
/// This is what decides which IRS deduction an invoice counts towards. Codes
/// come from the portal's `factemipf.catalogs.js`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Sector {
    /// C01 — Manutenção e reparação de veículos automóveis.
    CarRepair,
    /// C02 — Manutenção e reparação de motociclos.
    MotorcycleRepair,
    /// C03 — Alojamento, restauração e similares.
    Restaurants,
    /// C04 — Cabeleireiros e institutos de beleza.
    Hairdressers,
    /// C05 — Saúde.
    Health,
    /// C06 — Educação.
    Education,
    /// C07 — Imóveis (rendas, juros de habitação).
    Housing,
    /// C08 — Lares.
    CareHomes,
    /// C09 — Atividades veterinárias.
    Veterinary,
    /// C10 — Passes mensais de transportes públicos.
    PublicTransportPasses,
    /// C11 — Ginásios.
    Gyms,
    /// C12 — Jornais e revistas.
    Newspapers,
    /// C13 — Livros.
    Books,
    /// C14 — Artes.
    Arts,
    /// C15 — Museus.
    Museums,
    /// C26 — Educação (second code used by the portal).
    Education2,
    /// C99 — Outros (despesas gerais familiares).
    Other,
    /// A code this crate does not know yet.
    Unknown(String),
}

impl Sector {
    pub const ALL: [Sector; 17] = [
        Sector::CarRepair,
        Sector::MotorcycleRepair,
        Sector::Restaurants,
        Sector::Hairdressers,
        Sector::Health,
        Sector::Education,
        Sector::Housing,
        Sector::CareHomes,
        Sector::Veterinary,
        Sector::PublicTransportPasses,
        Sector::Gyms,
        Sector::Newspapers,
        Sector::Books,
        Sector::Arts,
        Sector::Museums,
        Sector::Education2,
        Sector::Other,
    ];

    /// Portal code, e.g. `"C05"`.
    pub fn code(&self) -> &str {
        match self {
            Sector::CarRepair => "C01",
            Sector::MotorcycleRepair => "C02",
            Sector::Restaurants => "C03",
            Sector::Hairdressers => "C04",
            Sector::Health => "C05",
            Sector::Education => "C06",
            Sector::Housing => "C07",
            Sector::CareHomes => "C08",
            Sector::Veterinary => "C09",
            Sector::PublicTransportPasses => "C10",
            Sector::Gyms => "C11",
            Sector::Newspapers => "C12",
            Sector::Books => "C13",
            Sector::Arts => "C14",
            Sector::Museums => "C15",
            Sector::Education2 => "C26",
            Sector::Other => "C99",
            Sector::Unknown(code) => code,
        }
    }

    /// Portuguese label as shown on the portal.
    pub fn label(&self) -> &str {
        match self {
            Sector::CarRepair => "Manutenção e reparação de veículos automóveis",
            Sector::MotorcycleRepair => "Manutenção e reparação de motociclos",
            Sector::Restaurants => "Alojamento, restauração e similares",
            Sector::Hairdressers => "Cabeleireiros e institutos de beleza",
            Sector::Health => "Saúde",
            Sector::Education | Sector::Education2 => "Educação",
            Sector::Housing => "Imóveis",
            Sector::CareHomes => "Lares",
            Sector::Veterinary => "Atividades veterinárias",
            Sector::PublicTransportPasses => "Passes mensais",
            Sector::Gyms => "Ginásios",
            Sector::Newspapers => "Jornais e revistas",
            Sector::Books => "Livros",
            Sector::Arts => "Artes",
            Sector::Museums => "Museus",
            Sector::Other => "Outros",
            Sector::Unknown(code) => code,
        }
    }
}

impl FromStr for Sector {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let code = s.trim().to_ascii_uppercase();
        if code.is_empty() {
            return Err(Error::InvalidInput("empty sector code".into()));
        }
        Ok(Sector::ALL
            .into_iter()
            .find(|sector| sector.code() == code)
            .unwrap_or(Sector::Unknown(code)))
    }
}

impl fmt::Display for Sector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl Serialize for Sector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Sector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        code.parse().map_err(serde::de::Error::custom)
    }
}

/// Document state filter (`estadoDocumentoFilter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentState {
    /// `P` — needs the user's input (usually: pick a sector).
    Pending,
    /// `R` — registered.
    Registered,
    /// `D` — divergent between issuer and buyer.
    Divergent,
    /// `B` — counted for a tax benefit.
    BenefitAttributed,
    /// `N` — not counted for a tax benefit.
    BenefitNotAttributed,
}

impl DocumentState {
    pub fn code(self) -> &'static str {
        match self {
            DocumentState::Pending => "P",
            DocumentState::Registered => "R",
            DocumentState::Divergent => "D",
            DocumentState::BenefitAttributed => "B",
            DocumentState::BenefitNotAttributed => "N",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim() {
            "P" => Some(DocumentState::Pending),
            "R" => Some(DocumentState::Registered),
            "D" => Some(DocumentState::Divergent),
            "B" => Some(DocumentState::BenefitAttributed),
            "N" => Some(DocumentState::BenefitNotAttributed),
            _ => None,
        }
    }
}

/// An invoice as returned by the document list (`obterDocumentosAdquirente`).
///
/// Field names on the portal side are not documented, so everything except
/// the id and date is optional and the untouched JSON is kept in [`raw`](Self::raw).
#[derive(Clone, Debug, Serialize)]
pub struct Document {
    pub id: DocumentId,
    pub issue_date: NaiveDate,
    pub issuer_nif: Option<String>,
    pub issuer_name: Option<String>,
    /// Invoice number as printed by the issuer, e.g. `FS 0123/45678`.
    pub number: Option<String>,
    /// ATCUD, the unique code printed on the receipt (e.g. `JFM87YYX-931804`).
    /// Handy for matching scanned receipts.
    pub atcud: Option<String>,
    pub document_type: Option<String>,
    pub document_type_description: Option<String>,
    pub total_cents: Option<i64>,
    pub vat_cents: Option<i64>,
    pub taxable_base_cents: Option<i64>,
    /// Sector currently assigned (or the issuer's default).
    pub sector: Option<Sector>,
    pub sector_description: Option<String>,
    /// State on the buyer's side (`estadoBeneficio`).
    pub state: Option<DocumentState>,
    /// Professional-activity sector, for self-employed buyers (`actividadeProf`).
    pub professional_sector: Option<String>,
    pub professional_sector_description: Option<String>,
    /// Invoice from a foreign issuer, registered manually.
    pub foreign: bool,
    pub raw: serde_json::Value,
}

impl Document {
    pub fn key(&self) -> DocumentKey {
        DocumentKey {
            id: self.id,
            issue_date: self.issue_date,
        }
    }

    pub(crate) fn from_json(raw: serde_json::Value) -> Result<Self, Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            id_documento: DocumentId,
            data_emissao_documento: NaiveDate,
            #[serde(default, deserialize_with = "opt_string")]
            nif_emitente: Option<String>,
            nome_emitente: Option<String>,
            #[serde(default, alias = "numeroDocumento", deserialize_with = "opt_string")]
            numerodocumento: Option<String>,
            #[serde(default, deserialize_with = "opt_string")]
            atcud: Option<String>,
            #[serde(default, deserialize_with = "opt_string")]
            tipo_documento: Option<String>,
            tipo_documento_desc: Option<String>,
            #[serde(default, deserialize_with = "opt_cents")]
            valor_total: Option<i64>,
            #[serde(default, deserialize_with = "opt_cents")]
            valor_total_iva: Option<i64>,
            #[serde(default, deserialize_with = "opt_cents")]
            valor_total_base_tributavel: Option<i64>,
            #[serde(default, deserialize_with = "opt_string")]
            actividade_emitente: Option<String>,
            actividade_emitente_desc: Option<String>,
            #[serde(default, deserialize_with = "opt_string")]
            actividade_prof: Option<String>,
            actividade_prof_desc: Option<String>,
            #[serde(default, deserialize_with = "opt_string")]
            estado_beneficio: Option<String>,
            #[serde(default)]
            is_documento_estrangeiro: Option<bool>,
        }

        let wire: Wire = serde_json::from_value(raw.clone())?;
        // Names arrive HTML-escaped (`Jo&atilde;o`).
        let text = |s: Option<String>| s.map(|s| html::to_text(&s)).filter(|s| !s.is_empty());
        Ok(Self {
            id: wire.id_documento,
            issue_date: wire.data_emissao_documento,
            issuer_nif: wire.nif_emitente,
            issuer_name: text(wire.nome_emitente),
            number: wire.numerodocumento,
            atcud: wire.atcud,
            document_type: wire.tipo_documento,
            document_type_description: text(wire.tipo_documento_desc),
            total_cents: wire.valor_total,
            vat_cents: wire.valor_total_iva,
            taxable_base_cents: wire.valor_total_base_tributavel,
            sector: wire
                .actividade_emitente
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok()),
            sector_description: text(wire.actividade_emitente_desc),
            state: wire
                .estado_beneficio
                .as_deref()
                .and_then(DocumentState::from_code),
            professional_sector: wire.actividade_prof,
            professional_sector_description: text(wire.actividade_prof_desc),
            foreign: wire.is_documento_estrangeiro.unwrap_or(false),
            raw,
        })
    }
}

/// One invoice line (detail page).
#[derive(Clone, Debug, Serialize)]
pub struct DocumentLine {
    pub id: Option<u64>,
    pub taxable_base_cents: Option<i64>,
    pub vat_cents: Option<i64>,
    pub total_cents: Option<i64>,
    /// Hundredths of a percent (`2300` = 23%).
    pub vat_rate: Option<i64>,
    pub vat_exemption_reason: Option<String>,
    pub raw: serde_json::Value,
}

impl DocumentLine {
    pub(crate) fn from_json(raw: serde_json::Value) -> Self {
        let int = |key: &str| raw.get(key).and_then(serde_json::Value::as_i64);
        Self {
            id: raw.get("idLinha").and_then(serde_json::Value::as_u64),
            taxable_base_cents: int("valorBaseTributavel"),
            vat_cents: int("valorIva"),
            total_cents: int("valorTotal"),
            vat_rate: int("taxaIva"),
            vat_exemption_reason: raw
                .get("motivoIsencao")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            raw,
        }
    }
}

/// An invoice's detail page: lines, current sector and the sectors the portal
/// offers for it.
#[derive(Clone, Debug, Serialize)]
pub struct DocumentDetail {
    pub key: DocumentKey,
    pub sector: Option<Sector>,
    /// Sectors offered in the dropdown. Only these are valid for this invoice
    /// (they depend on the issuer's registered activities).
    pub available_sectors: Vec<Sector>,
    /// Issuer control code shown on the page (`hashAdquirente`).
    pub control_code: Option<String>,
    pub lines: Vec<DocumentLine>,
    /// The lines exactly as the portal sent them; posted back unchanged when
    /// changing the sector.
    pub(crate) raw_lines: serde_json::Value,
}

/// Filters for [`Client::documents`](crate::Client::documents).
#[derive(Clone, Debug)]
pub struct DocumentQuery {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub sector: Option<Sector>,
    pub state: Option<DocumentState>,
}

impl DocumentQuery {
    pub fn new(from: NaiveDate, to: NaiveDate) -> Self {
        Self {
            from,
            to,
            sector: None,
            state: None,
        }
    }

    /// Invoices waiting for a sector in the given period.
    pub fn pending(from: NaiveDate, to: NaiveDate) -> Self {
        Self::new(from, to).state(DocumentState::Pending)
    }

    pub fn sector(mut self, sector: Sector) -> Self {
        self.sector = Some(sector);
        self
    }

    pub fn state(mut self, state: DocumentState) -> Self {
        self.state = Some(state);
        self
    }
}

fn u64_or_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Number(u64),
        Text(String),
    }
    match Either::deserialize(deserializer)? {
        Either::Number(n) => Ok(n),
        Either::Text(s) => s.trim().parse().map_err(serde::de::Error::custom),
    }
}

fn opt_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        },
    )
}

fn opt_cents<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sector_round_trip() {
        for sector in Sector::ALL {
            assert_eq!(sector.code().parse::<Sector>().unwrap(), sector);
        }
        assert_eq!("c05".parse::<Sector>().unwrap(), Sector::Health);
        assert_eq!(
            "C42".parse::<Sector>().unwrap(),
            Sector::Unknown("C42".into())
        );
    }

    #[test]
    fn document_from_list_json() {
        let raw = serde_json::json!({
            "idDocumento": 100000000001u64,
            "dataEmissaoDocumento": "2023-10-21",
            "nifEmitente": 500000000,
            "nomeEmitente": "Jo&atilde;o Exemplo Lda",
            "numerodocumento": "FS 001/123",
            "atcud": "JFM87YYX-931804",
            "estadoBeneficio": "R",
            "actividadeProf": null,
            "tipoDocumento": "FS",
            "tipoDocumentoDesc": "Fatura Simplificada",
            "valorTotal": 2043,
            "valorTotalIva": 382,
            "valorTotalBaseTributavel": 1661,
            "actividadeEmitente": "C03",
            "actividadeEmitenteDesc": "Alojamento, restauração e similares",
            "isDocumentoEstrangeiro": false,
            "somethingNew": 1
        });
        let doc = Document::from_json(raw).unwrap();
        assert_eq!(doc.id, DocumentId(100000000001));
        assert_eq!(
            doc.issue_date,
            NaiveDate::from_ymd_opt(2023, 10, 21).unwrap()
        );
        assert_eq!(doc.issuer_nif.as_deref(), Some("500000000"));
        assert_eq!(doc.number.as_deref(), Some("FS 001/123"));
        assert_eq!(doc.issuer_name.as_deref(), Some("João Exemplo Lda"));
        assert_eq!(doc.atcud.as_deref(), Some("JFM87YYX-931804"));
        assert_eq!(doc.state, Some(DocumentState::Registered));
        assert!(doc.professional_sector.is_none());
        assert_eq!(doc.total_cents, Some(2043));
        assert_eq!(doc.sector, Some(Sector::Restaurants));
        assert_eq!(doc.raw["somethingNew"], 1);
    }

    #[test]
    fn document_tolerates_missing_fields() {
        let raw = serde_json::json!({
            "idDocumento": "42",
            "dataEmissaoDocumento": "2024-01-02",
            "actividadeEmitente": ""
        });
        let doc = Document::from_json(raw).unwrap();
        assert_eq!(doc.id, DocumentId(42));
        assert!(doc.sector.is_none());
        assert!(doc.total_cents.is_none());
    }
}
