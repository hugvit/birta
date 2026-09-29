//! Request guard for the local server, and escaping for values the page embeds.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

const LOOPBACK_NAMES: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// Accepts only loopback Host names (on any port, so `ssh -L` forwards work).
#[derive(Clone)]
pub(crate) struct HostGuard {
    names: &'static [&'static str],
}

impl HostGuard {
    pub(crate) fn new() -> Self {
        Self {
            names: &LOOPBACK_NAMES,
        }
    }

    fn check(&self, headers: &HeaderMap, uri: &Uri) -> Result<(), StatusCode> {
        let host = self
            .host(headers, uri)
            .ok_or(StatusCode::MISDIRECTED_REQUEST)?;
        if origin_matches(headers, host) && fetch_site_allowed(headers) {
            Ok(())
        } else {
            Err(StatusCode::FORBIDDEN)
        }
    }

    /// The single Host header, if it names loopback and agrees with an absolute-form URI.
    fn host<'a>(&self, headers: &'a HeaderMap, uri: &Uri) -> Option<&'a str> {
        let mut hosts = headers.get_all(header::HOST).iter();
        let (Some(host), None) = (hosts.next(), hosts.next()) else {
            return None;
        };
        let host = host.to_str().ok()?;
        if let Some(authority) = uri.authority()
            && !authority.as_str().eq_ignore_ascii_case(host)
        {
            return None;
        }
        self.is_loopback(host).then_some(host)
    }

    /// Exact name match rather than IP parsing, so only the spellings browsers send pass.
    fn is_loopback(&self, host: &str) -> bool {
        let name_end = if host.starts_with('[') {
            host.find(']').map(|i| i + 1)
        } else {
            host.find(':')
        };
        let (name, port) = host.split_at(name_end.unwrap_or(host.len()));
        let port_ok = match port.strip_prefix(':') {
            Some(digits) => {
                // `parse` alone would accept a leading `+`.
                digits.bytes().all(|b| b.is_ascii_digit()) && digits.parse::<u16>().is_ok()
            }
            None => port.is_empty(),
        };
        port_ok && self.names.iter().any(|n| name.eq_ignore_ascii_case(n))
    }
}

/// An absent Origin passes (curl, editors); a present one must be this server.
fn origin_matches(headers: &HeaderMap, host: &str) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    match (origins.next(), origins.next()) {
        (None, _) => true,
        (Some(origin), None) => origin.to_str().is_ok_and(|origin| {
            origin
                .strip_prefix("http://")
                .is_some_and(|rest| rest.eq_ignore_ascii_case(host))
        }),
        _ => false,
    }
}

/// Other sites, and scripts on other localhost ports, may only navigate here.
fn fetch_site_allowed(headers: &HeaderMap) -> bool {
    match headers.get("sec-fetch-site").map(HeaderValue::as_bytes) {
        Some(b"cross-site") => false,
        Some(b"same-site") => headers
            .get("sec-fetch-mode")
            .is_some_and(|mode| mode == "navigate"),
        _ => true,
    }
}

/// Rejects DNS-rebound and cross-site requests: 421 for a bad Host, 403 for a bad Origin or Sec-Fetch.
pub(crate) async fn guard(
    State(host_guard): State<HostGuard>,
    req: Request,
    next: Next,
) -> Response {
    match host_guard.check(req.headers(), req.uri()) {
        Ok(()) => next.run(req).await,
        Err(status) => status.into_response(),
    }
}

pub(crate) async fn base_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// Escapes text for an HTML attribute value or element content.
pub(crate) fn html_escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// JSON that is inert inside `<script>`: no `</script>`, `<!--` or raw line separators.
pub(crate) fn json_for_script(value: &serde_json::Value) -> String {
    let json = value.to_string();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            // These only occur inside JSON strings, where `\uXXXX` is equivalent
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}
