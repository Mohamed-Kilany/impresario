//! DevTools page discovery (spec 10.3 "Page resolution") and the tiny HTTP/1.1
//! GET used against `/json/list` (the connector has no HTTP client dependency;
//! the DevTools endpoint speaks plain HTTP with `Connection: close`).

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::errors::ConnectorError;

/// One entry of `/json/list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub ws_url: String,
}

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// A `host:port` endpoint (IPv6 hosts are written `[::1]:9222`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

impl HostPort {
    /// Parse `host:port`; requires an explicit port and a non-empty host.
    pub fn parse(address: &str) -> Result<Self, ConnectorError> {
        let (host, port) = address.rsplit_once(':').ok_or_else(|| {
            ConnectorError::protocol(format!("'{address}' is not a host:port endpoint"))
        })?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return Err(ConnectorError::protocol(format!("'{address}' has no host")));
        }
        let port: u16 = port
            .parse()
            .map_err(|_| ConnectorError::protocol(format!("'{address}' has no valid port")))?;
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    #[must_use]
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// True when the endpoint already names a DevTools page target.
#[must_use]
pub fn has_page_path(ws_url: &str) -> bool {
    ws_url.contains("/devtools/page/")
}

/// List pages via `GET /json/list` with the `/json` fallback of spec 10.3.
pub async fn list_pages(endpoint: &HostPort) -> Result<Vec<PageInfo>, ConnectorError> {
    let body = match http_get(endpoint, "/json/list").await {
        Ok(body) => body,
        Err(first) => match http_get(endpoint, "/json").await {
            Ok(body) => body,
            Err(_) => return Err(first),
        },
    };
    parse_page_listing(&body).map_err(|error| {
        ConnectorError::device_unavailable(format!(
            "cannot parse the DevTools page listing from {}: {error}",
            endpoint.authority()
        ))
    })
}

/// Keep `type == "page"` entries, apply `page_match`, require exactly one
/// (spec 10.3). Zero or several candidates is `DEVICE_UNAVAILABLE` listing
/// them.
pub fn select_page(
    pages: &[PageInfo],
    page_match: Option<&str>,
) -> Result<PageInfo, ConnectorError> {
    let mut candidates: Vec<&PageInfo> = pages
        .iter()
        .filter(|page| match page_match {
            Some(needle) => page.url.contains(needle) || page.title.contains(needle),
            None => true,
        })
        .collect();
    if candidates.len() == 1 {
        if let Some(page) = candidates.pop() {
            return Ok(page.clone());
        }
    }
    let listing = candidates
        .iter()
        .map(|page| format!("{} ({})", page.title, page.url))
        .collect::<Vec<_>>()
        .join(", ");
    Err(ConnectorError::device_unavailable(format!(
        "expected exactly one page target, found {}: [{}]{}",
        candidates.len(),
        listing,
        page_match.map_or(String::new(), |needle| format!(" (page_match '{needle}')"))
    )))
}

/// The target id of a page websocket URL (`.../devtools/page/<id>`), when the
/// URL carries one.
#[must_use]
pub fn page_id_from_ws(ws_url: &str) -> Option<String> {
    let (_, tail) = ws_url.split_once("/devtools/page/")?;
    let id = tail.split(['/', '?']).next().unwrap_or("").trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// The browser-level websocket of an endpoint via `GET /json/version`. The
/// `raw` backend never needs it; `chromiumoxide` connects at browser level
/// (spec 10.1 `ChromiumoxideBackend`).
pub async fn fetch_browser_ws(endpoint: &HostPort) -> Result<String, ConnectorError> {
    let body = http_get(endpoint, "/json/version").await?;
    let ws_url = parse_version_listing(&body).map_err(|error| {
        ConnectorError::device_unavailable(format!(
            "cannot parse the DevTools version reply from {}: {error}",
            endpoint.authority()
        ))
    })?;
    Ok(rewrite_ws_host(&ws_url, endpoint))
}

/// Parse a `/json/version` body: the `webSocketDebuggerUrl` field.
pub fn parse_version_listing(body: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let ws_url = value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ws_url.is_empty() {
        return Err("the reply carries no webSocketDebuggerUrl".to_string());
    }
    Ok(ws_url.to_string())
}

/// Rewrite the websocket URL's host:port to the address we reached the device
/// on (spec 10.3: TVs may report an unroutable host).
#[must_use]
pub fn rewrite_ws_host(ws_url: &str, endpoint: &HostPort) -> String {
    let Ok(mut url) = url::Url::parse(ws_url) else {
        return ws_url.to_string();
    };
    let same_host = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case(&endpoint.host) || host.eq_ignore_ascii_case("localhost")
    }) && url.port() == Some(endpoint.port);
    if same_host {
        return ws_url.to_string();
    }
    if url.set_host(Some(&endpoint.host)).is_err() {
        return ws_url.to_string();
    }
    if url.set_port(Some(endpoint.port)).is_err() {
        return ws_url.to_string();
    }
    url.to_string()
}

/// The `webSocketDebuggerUrl` of a page entry, or a `DEVICE_UNAVAILABLE` error
/// when the list did not carry one.
pub fn page_ws_url(page: &PageInfo) -> Result<String, ConnectorError> {
    if page.ws_url.is_empty() {
        return Err(ConnectorError::device_unavailable(format!(
            "the page '{}' ({}) exposes no webSocketDebuggerUrl",
            page.title, page.url
        )));
    }
    Ok(page.ws_url.clone())
}

/// Parse a `/json/list` body: a JSON array (or a single object) of entries,
/// tolerating unknown and missing fields.
pub fn parse_page_listing(body: &str) -> Result<Vec<PageInfo>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(_) => vec![value],
        _ => return Err("the listing is neither an array nor an object".to_string()),
    };
    let mut pages = Vec::new();
    for entry in entries {
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "page" {
            continue;
        }
        pages.push(PageInfo {
            id: string_field(&entry, "id"),
            url: string_field(&entry, "url"),
            title: string_field(&entry, "title"),
            ws_url: string_field(&entry, "webSocketDebuggerUrl"),
        });
    }
    Ok(pages)
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Minimal HTTP/1.1 GET returning the response body.
pub async fn http_get(endpoint: &HostPort, path: &str) -> Result<String, ConnectorError> {
    let connect = TcpStream::connect((endpoint.host.as_str(), endpoint.port));
    let mut stream = tokio::time::timeout(HTTP_TIMEOUT, connect)
        .await
        .map_err(|_| {
            ConnectorError::device_unavailable(format!(
                "connecting to {} timed out",
                endpoint.authority()
            ))
        })?
        .map_err(|error| {
            ConnectorError::device_unavailable(format!(
                "cannot connect to {}: {error}",
                endpoint.authority()
            ))
        })?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        endpoint.authority()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| {
            ConnectorError::device_unavailable(format!(
                "cannot write the HTTP request to {}: {error}",
                endpoint.authority()
            ))
        })?;

    let raw = read_http_reply(&mut stream, &endpoint.authority()).await?;
    parse_http_response(&raw)
}

/// Read one HTTP/1.1 reply without waiting for EOF: DevTools endpoints keep
/// the connection open even when asked to close (`Content-Length` or chunked
/// framing decides when the reply is complete; EOF covers the rest).
async fn read_http_reply<S: AsyncReadExt + Unpin>(
    stream: &mut S,
    authority: &str,
) -> Result<Vec<u8>, ConnectorError> {
    let deadline = tokio::time::Instant::now() + HTTP_TIMEOUT;
    let mut raw: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; 8192];
    loop {
        if let Some(length) = reply_complete(&raw) {
            raw.truncate(length);
            return Ok(raw);
        }
        if raw.len() as u64 >= MAX_BODY {
            return Ok(raw);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let read = tokio::time::timeout(remaining, stream.read(&mut buffer))
            .await
            .map_err(|_| {
                ConnectorError::device_unavailable(format!(
                    "reading the HTTP reply from {authority} timed out"
                ))
            })?
            .map_err(|error| {
                ConnectorError::device_unavailable(format!(
                    "reading the HTTP reply from {authority} failed: {error}"
                ))
            })?;
        if read == 0 {
            if raw.is_empty() {
                return Err(ConnectorError::device_unavailable(format!(
                    "the HTTP connection to {authority} closed without a reply"
                )));
            }
            return Ok(raw);
        }
        raw.extend_from_slice(buffer.get(..read).unwrap_or_default());
    }
}

/// How many bytes of `raw` form a complete HTTP reply, when that is knowable
/// from the headers (framing by `Content-Length` or chunked encoding).
fn reply_complete(raw: &[u8]) -> Option<usize> {
    let (header_end, body_start) = find_subsequence(raw, b"\r\n\r\n")?;
    let headers = String::from_utf8_lossy(raw.get(..header_end).unwrap_or_default());
    let lower = headers.to_ascii_lowercase();
    if lower.contains("transfer-encoding: chunked") {
        let body = raw.get(body_start..)?;
        return chunked_length(body).map(|length| body_start + length);
    }
    for line in lower.lines() {
        if let Some(value) = line.strip_prefix("content-length:") {
            let length: usize = value.trim().parse().ok()?;
            let total = body_start.saturating_add(length);
            if raw.len() >= total {
                return Some(total);
            }
            return None;
        }
    }
    None
}

/// Length of a complete chunked body (including the terminating chunk and
/// any trailers within the searched window), else `None` while incomplete.
fn chunked_length(body: &[u8]) -> Option<usize> {
    let mut position = 0usize;
    loop {
        let (line_end, after_line) = {
            let rest = body.get(position..)?;
            let (line_end, consumed) =
                find_subsequence(rest, b"\r\n").or_else(|| find_subsequence(rest, b"\n"))?;
            (position + line_end, position + consumed)
        };
        let size_text = String::from_utf8_lossy(body.get(position..line_end)?);
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        if size == 0 {
            // The terminating chunk: expect the final CRLF (trailers end the
            // stream too, so accept the first blank line).
            let rest = body.get(after_line..)?;
            let end = find_subsequence(rest, b"\r\n").or_else(|| find_subsequence(rest, b"\n"))?;
            return Some(after_line + end.1);
        }
        let data_end = after_line.checked_add(size)?;
        let rest = body.get(data_end..)?;
        let (_, consumed) =
            find_subsequence(rest, b"\r\n").or_else(|| find_subsequence(rest, b"\n"))?;
        position = data_end + consumed;
    }
}

/// Split headers from body, enforce 2xx, de-chunk when needed.
pub fn parse_http_response(raw: &[u8]) -> Result<String, ConnectorError> {
    let separator = find_subsequence(raw, b"\r\n\r\n")
        .or_else(|| find_subsequence(raw, b"\n\n"))
        .ok_or_else(|| ConnectorError::device_unavailable("malformed HTTP reply"))?;
    let header_end = separator.0;
    let body_start = separator.1;
    let headers = String::from_utf8_lossy(raw.get(..header_end).unwrap_or_default()).to_string();
    let mut status = 0u16;
    if let Some(line) = headers.lines().next() {
        status = line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
    }
    if !(200..300).contains(&status) {
        return Err(ConnectorError::device_unavailable(format!(
            "the DevTools endpoint answered HTTP {status}"
        )));
    }
    let lower = headers.to_ascii_lowercase();
    let body = raw.get(body_start..).unwrap_or_default();
    if lower.contains("transfer-encoding: chunked") {
        return dechunk(body);
    }
    Ok(String::from_utf8_lossy(body).to_string())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<(usize, usize)> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|start| (start, start + needle.len()))
}

fn dechunk(body: &[u8]) -> Result<String, ConnectorError> {
    let mut text = String::new();
    let mut rest = body;
    loop {
        let line_end = find_subsequence(rest, b"\r\n")
            .or_else(|| find_subsequence(rest, b"\n"))
            .ok_or_else(|| ConnectorError::device_unavailable("malformed chunked body"))?;
        let size_text = String::from_utf8_lossy(rest.get(..line_end.0).unwrap_or_default());
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| ConnectorError::device_unavailable("malformed chunk size"))?;
        rest = rest.get(line_end.1..).unwrap_or_default();
        if size == 0 {
            break;
        }
        let chunk = rest
            .get(..size)
            .ok_or_else(|| ConnectorError::device_unavailable("truncated chunked body"))?;
        text.push_str(&String::from_utf8_lossy(chunk));
        rest = rest
            .get(size..)
            .and_then(|tail| {
                find_subsequence(tail, b"\r\n")
                    .map(|(_, consumed)| tail.get(consumed..).unwrap_or_default())
            })
            .unwrap_or_default();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"[
        {"id":"1","type":"page","url":"http://localhost:8080/index.html","title":"Shop","webSocketDebuggerUrl":"ws://localhost:9222/devtools/page/1"},
        {"id":"2","type":"other","url":"","title":"","webSocketDebuggerUrl":"ws://localhost:9222/devtools/browser/2"},
        {"id":"3","type":"page","url":"http://localhost:8080/tv.html","title":"TV","webSocketDebuggerUrl":"ws://10.0.0.7:9222/devtools/page/3"}
    ]"#;

    #[test]
    fn listing_keeps_only_pages() {
        let pages = parse_page_listing(LISTING).expect("parse");
        assert_eq!(pages.len(), 2);
        assert_eq!(pages.first().map(|p| p.id.as_str()), Some("1"));
        assert_eq!(pages.get(1).map(|p| p.title.as_str()), Some("TV"));
    }

    #[test]
    fn selection_needs_exactly_one_candidate() {
        let pages = parse_page_listing(LISTING).expect("parse");
        let error = select_page(&pages, None).expect_err("two pages");
        assert_eq!(
            error.code,
            impresario_core::command::WireErrorCode::DeviceUnavailable
        );
        assert!(error.message.contains("expected exactly one"));
        let page = select_page(&pages, Some("tv.html")).expect("one match");
        assert_eq!(page.id, "3");
        assert!(select_page(&pages, Some("nope")).is_err());
    }

    #[test]
    fn ws_host_rewrite_keeps_path() {
        let endpoint = HostPort::parse("192.168.1.50:9998").expect("endpoint");
        assert_eq!(
            rewrite_ws_host("ws://10.0.0.7:9222/devtools/page/3", &endpoint),
            "ws://192.168.1.50:9998/devtools/page/3"
        );
        assert_eq!(
            rewrite_ws_host("ws://192.168.1.50:9998/devtools/page/3", &endpoint),
            "ws://192.168.1.50:9998/devtools/page/3"
        );
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(
            HostPort::parse("127.0.0.1:9222").expect("ok").authority(),
            "127.0.0.1:9222"
        );
        assert_eq!(
            HostPort::parse("[::1]:9222").expect("ok").host,
            "::1".to_string()
        );
        assert!(HostPort::parse("nohost").is_err());
        assert!(HostPort::parse(":9222").is_err());
        assert!(HostPort::parse("host:notaport").is_err());
    }

    #[test]
    fn http_response_parsing() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";
        assert_eq!(parse_http_response(raw).expect("ok"), "hello");
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).expect("ok"), "hello");
        let failed = b"HTTP/1.1 404 Not Found\r\n\r\n";
        assert!(parse_http_response(failed).is_err());
    }

    #[test]
    fn reply_framing_waits_for_the_body() {
        let partial = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel";
        assert_eq!(reply_complete(partial), None);
        let complete = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(reply_complete(complete), Some(complete.len()));
        // Extra bytes after the body (a next reply, or junk) are cut off.
        let mut padded = complete.to_vec();
        padded.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
        assert_eq!(reply_complete(&padded), Some(complete.len()));
    }

    #[test]
    fn chunked_framing_waits_for_the_terminator() {
        let partial = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n";
        assert_eq!(reply_complete(partial), None);
        let complete =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert_eq!(reply_complete(complete), Some(complete.len()));
        let two = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n3\r\ncde\r\n0\r\n\r\n";
        assert_eq!(reply_complete(two), Some(two.len()));
    }

    #[test]
    fn unframed_replies_need_eof() {
        let raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nwhatever";
        assert_eq!(reply_complete(raw), None);
    }

    #[test]
    fn page_path_detection() {
        assert!(has_page_path("ws://host:9222/devtools/page/AB"));
        assert!(!has_page_path("ws://host:9222/devtools/browser/AB"));
        assert!(!has_page_path("ws://host:9222"));
    }

    #[test]
    fn page_ids_are_extracted_from_ws_urls() {
        assert_eq!(
            page_id_from_ws("ws://host:9222/devtools/page/ABC-123"),
            Some("ABC-123".to_string())
        );
        assert_eq!(
            page_id_from_ws("wss://host:9222/devtools/page/ABC-123?x=1"),
            Some("ABC-123".to_string())
        );
        assert_eq!(page_id_from_ws("ws://host:9222/devtools/browser/ABC"), None);
        assert_eq!(page_id_from_ws("ws://host:9222/devtools/page/"), None);
    }

    #[test]
    fn version_listing_yields_the_browser_ws() {
        let body = r#"{"Browser":"Chrome/140.0","webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/abc"}"#;
        assert_eq!(
            parse_version_listing(body).expect("ok"),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
        assert!(parse_version_listing("{}").is_err());
        assert!(parse_version_listing("not json").is_err());
    }
}
