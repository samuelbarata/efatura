//! Small HTML helpers shared by the auth flow and the e-fatura scrapers.

use std::sync::LazyLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use url::Url;

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

/// An HTML form, ready to be replayed.
#[derive(Debug, Clone)]
pub(crate) struct HtmlForm {
    pub action: Url,
    pub post: bool,
    pub fields: Vec<(String, String)>,
}

/// Extract a form from `html`. If `id` is given, only that form is considered;
/// otherwise the first one in the document.
pub(crate) fn find_form(html: &str, base: &Url, id: Option<&str>) -> Option<HtmlForm> {
    let doc = Html::parse_document(html);
    let form = match id {
        Some(id) => doc.select(&selector(&format!("form#{id}"))).next()?,
        None => doc.select(&selector("form")).next()?,
    };
    Some(form_from_element(form, base))
}

fn form_from_element(form: ElementRef<'_>, base: &Url) -> HtmlForm {
    let el = form.value();
    let action = el
        .attr("action")
        .filter(|a| !a.trim().is_empty())
        .and_then(|a| base.join(a.trim()).ok())
        .unwrap_or_else(|| base.clone());
    let post = el
        .attr("method")
        .is_some_and(|m| m.eq_ignore_ascii_case("post"));
    let fields = form
        .select(&selector("input[name]"))
        .filter(|input| {
            !matches!(
                input
                    .value()
                    .attr("type")
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("submit" | "button" | "image" | "reset")
            )
        })
        .filter_map(|input| {
            let name = input.value().attr("name")?.to_owned();
            let value = input.value().attr("value").unwrap_or_default().to_owned();
            Some((name, value))
        })
        .collect();
    HtmlForm {
        action,
        post,
        fields,
    }
}

/// Find a `window.location = "..."`-style redirect.
pub(crate) fn find_js_redirect(html: &str, base: &Url) -> Option<Url> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:window\.)?location(?:\.href)?\s*=\s*['"]([^'"]+)['"]"#).unwrap()
    });
    let target = RE.captures(html)?.get(1)?.as_str();
    base.join(target).ok()
}

/// Text of the visible `div.alert` boxes of the given kind (`error`,
/// `success`, `info`, `block`) on an e-fatura page. Hidden ones (the empty
/// templates every page carries) are skipped.
pub(crate) fn alerts(html: &str, kind: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    doc.select(&selector(&format!("div.alert.alert-{kind}")))
        .filter(|div| !div.value().classes().any(|c| c == "hide"))
        .map(|div| {
            div.text()
                .filter(|t| t.trim() != "\u{d7}")
                .collect::<Vec<_>>()
                .join(" ")
        })
        .map(|t| collapse_whitespace(&t))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Value attribute of the element with the given id.
pub(crate) fn value_by_id(doc: &Html, id: &str) -> Option<String> {
    doc.select(&selector(&format!("#{id}")))
        .next()
        .and_then(|el| el.value().attr("value"))
        .map(|v| v.trim().to_owned())
}

pub(crate) fn parse(html: &str) -> Html {
    Html::parse_document(html)
}

pub(crate) fn select_all<'a>(doc: &'a Html, css: &str) -> Vec<ElementRef<'a>> {
    doc.select(&selector(css)).collect()
}

pub(crate) fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip tags and decode the handful of entities the AT uses in messages.
pub(crate) fn to_text(fragment: &str) -> String {
    let doc = Html::parse_fragment(fragment);
    collapse_whitespace(&doc.root_element().text().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_auto_submit_form() {
        let html = r#"<html><body>
            <form id="other" action="/x"></form>
            <form id="forwardParticipantForm" action="https://faturas.example/home.action" method="post">
              <input type="hidden" name="sign" value="abc"/>
              <input type="hidden" name="userID" value="123"/>
              <input type="submit" value="clique aqui"/>
            </form></body></html>"#;
        let base = Url::parse("https://acesso.example/login").unwrap();
        let form = find_form(html, &base, Some("forwardParticipantForm")).unwrap();
        assert!(form.post);
        assert_eq!(form.action.as_str(), "https://faturas.example/home.action");
        assert_eq!(
            form.fields,
            vec![
                ("sign".to_owned(), "abc".to_owned()),
                ("userID".to_owned(), "123".to_owned())
            ]
        );
    }

    #[test]
    fn visible_alerts_only() {
        let html = r#"<div class='alert alert-error  hide '><button>&#215;</button></div>
            <div class='alert alert-error'><button type="button" class="close">&#215;</button>
              <ul><li>Setor   inválido</li></ul></div>"#;
        assert_eq!(alerts(html, "error"), vec!["Setor inválido".to_owned()]);
    }
}
