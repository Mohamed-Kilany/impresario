//! Minimal HTTP/1.1 parsing and response writing for the embedded console.
//!
//! One request per connection (`Connection: close`), no chunked bodies, no
//! keep-alive. Limits and timeouts are enforced by the server loop in
//! [`super`].

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::{json, Value};

use super::{MAX_BODY_BYTES, MAX_HEADER_BYTES};

/// One parsed request. `path` and `query` are percent-decoded.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    /// One header value, by lower-case name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// One query value.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str)
    }
}

/// Parsed request line and headers, before the body is read.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub content_length: usize,
}

/// A request that could not be parsed.
#[derive(Debug)]
pub enum ParseError {
    BadRequest(String),
    TooLarge(String),
    Io(String),
}

impl ParseError {
    /// A response that explains the failure.
    #[must_use]
    pub fn response(self) -> Response {
        match self {
            ParseError::BadRequest(message) => Response::error(400, "bad_request", message),
            ParseError::TooLarge(message) => Response::error(413, "too_large", message),
            ParseError::Io(message) => Response::error(400, "bad_request", message),
        }
    }
}

/// Read one request from a connection.
pub fn read_request(stream: &mut TcpStream) -> Result<Request, ParseError> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(ParseError::TooLarge(format!(
                "request headers exceed {MAX_HEADER_BYTES} bytes"
            )));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ParseError::Io(format!("cannot read the request: {error}")))?;
        if read == 0 {
            return Err(ParseError::BadRequest(
                "the connection closed before the request was complete".to_string(),
            ));
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    };
    if header_end > MAX_HEADER_BYTES {
        return Err(ParseError::TooLarge(format!(
            "request headers exceed {MAX_HEADER_BYTES} bytes"
        )));
    }
    let header_bytes = buffer.get(..header_end).unwrap_or_default();
    let header_text = std::str::from_utf8(header_bytes)
        .map_err(|_| ParseError::BadRequest("request headers are not UTF-8".to_string()))?;
    let head = parse_request_head(header_text)?;
    if head.content_length > MAX_BODY_BYTES {
        return Err(ParseError::TooLarge(format!(
            "request body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    let mut body = buffer.get(header_end + 4..).unwrap_or_default().to_vec();
    if body.len() > head.content_length {
        return Err(ParseError::BadRequest(
            "the request body is longer than Content-Length".to_string(),
        ));
    }
    while body.len() < head.content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ParseError::Io(format!("cannot read the request body: {error}")))?;
        if read == 0 {
            return Err(ParseError::BadRequest(
                "the connection closed before the body was complete".to_string(),
            ));
        }
        body.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
    if body.len() > head.content_length {
        return Err(ParseError::BadRequest(
            "the request body is longer than Content-Length".to_string(),
        ));
    }
    Ok(Request {
        method: head.method,
        path: head.path,
        query: head.query,
        headers: head.headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parse the request line and headers (the text before the blank line).
pub(crate) fn parse_request_head(text: &str) -> Result<Head, ParseError> {
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ParseError::BadRequest("the request line is missing".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ParseError::BadRequest("the request method is missing".to_string()))?;
    let target = parts
        .next()
        .ok_or_else(|| ParseError::BadRequest("the request target is missing".to_string()))?;
    let version = parts
        .next()
        .ok_or_else(|| ParseError::BadRequest("the HTTP version is missing".to_string()))?;
    if parts.next().is_some() {
        return Err(ParseError::BadRequest(
            "the request line has too many fields".to_string(),
        ));
    }
    if !version.starts_with("HTTP/1.") {
        return Err(ParseError::BadRequest(format!(
            "unsupported HTTP version '{version}'"
        )));
    }
    if !method
        .bytes()
        .all(|value| value.is_ascii_uppercase() || value == b'-')
    {
        return Err(ParseError::BadRequest(
            "the request method is not valid".to_string(),
        ));
    }
    if !target.starts_with('/') {
        return Err(ParseError::BadRequest(
            "only origin-form request targets are supported".to_string(),
        ));
    }
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let path = percent_decode(raw_path, false)?;
    if path.contains('\0') {
        return Err(ParseError::BadRequest(
            "the request path contains a NUL byte".to_string(),
        ));
    }
    let mut query = BTreeMap::new();
    if !raw_query.is_empty() {
        for pair in raw_query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            query.insert(percent_decode(key, true)?, percent_decode(value, true)?);
        }
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ParseError::BadRequest(format!(
                "malformed header line '{line}'"
            )));
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ParseError::BadRequest(
                "a header name is not valid".to_string(),
            ));
        }
        headers.insert(name, value.trim().to_string());
    }
    if headers.contains_key("transfer-encoding") {
        return Err(ParseError::BadRequest(
            "Transfer-Encoding is not supported; send Content-Length".to_string(),
        ));
    }
    let content_length = match headers.get("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| {
            ParseError::BadRequest("Content-Length is not a non-negative integer".to_string())
        })?,
        None => 0,
    };
    Ok(Head {
        method: method.to_string(),
        path,
        query,
        headers,
        content_length,
    })
}

fn percent_decode(input: &str, plus_as_space: bool) -> Result<String, ParseError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = bytes
                .get(index + 1)
                .and_then(|value| hex_value(*value))
                .ok_or_else(|| ParseError::BadRequest("invalid percent escape".to_string()))?;
            let low = bytes
                .get(index + 2)
                .and_then(|value| hex_value(*value))
                .ok_or_else(|| ParseError::BadRequest("invalid percent escape".to_string()))?;
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        if plus_as_space && byte == b'+' {
            decoded.push(b' ');
        } else {
            decoded.push(byte);
        }
        index += 1;
    }
    String::from_utf8(decoded)
        .map_err(|_| ParseError::BadRequest("the request target is not UTF-8".to_string()))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One HTTP response, with the console's security headers added on write.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
}

impl Response {
    /// A JSON response.
    #[must_use]
    pub fn json(status: u16, value: Value) -> Self {
        let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
        Self {
            status,
            content_type: "application/json; charset=utf-8".to_string(),
            body,
            extra_headers: Vec::new(),
        }
    }

    /// A text response.
    #[must_use]
    pub fn text(status: u16, content_type: &str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            body: body.into().into_bytes(),
            extra_headers: Vec::new(),
        }
    }

    /// A JSON error envelope.
    #[must_use]
    pub fn error(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self::json(
            status,
            json!({
                "ok": false,
                "error": {
                    "code": code,
                    "message": message.into(),
                },
            }),
        )
    }
}

/// Write one response, including the shared security headers.
pub fn write_response(
    stream: &mut TcpStream,
    response: &Response,
    head_only: bool,
) -> std::io::Result<()> {
    let mut head = String::new();
    head.push_str(&format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    ));
    head.push_str(&format!("Content-Type: {}\r\n", response.content_type));
    head.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    head.push_str("Connection: close\r\n");
    head.push_str("Cache-Control: no-store\r\n");
    head.push_str("X-Content-Type-Options: nosniff\r\n");
    head.push_str("X-Frame-Options: DENY\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\n");
    head.push_str("Permissions-Policy: camera=(), microphone=(), geolocation=()\r\n");
    head.push_str(
        "Content-Security-Policy: default-src 'none'; script-src 'self'; \
         style-src 'self'; img-src 'self' data:; connect-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n",
    );
    for (name, value) in &response.extra_headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(&response.body)?;
    }
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_get_with_query_and_headers() {
        let head = parse_request_head(
            "GET /api/tests/file?path=plays%2FOne.play&ignore_case=1 HTTP/1.1\r\n\
             Host: 127.0.0.1:7810\r\n\
             Content-Length: 0\r\n",
        )
        .expect("head parses");
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/api/tests/file");
        assert_eq!(
            head.query.get("path").map(String::as_str),
            Some("plays/One.play")
        );
        assert_eq!(
            head.headers.get("host").map(String::as_str),
            Some("127.0.0.1:7810")
        );
        assert_eq!(head.content_length, 0);
    }

    #[test]
    fn rejects_transfer_encoding_and_bad_lengths() {
        assert!(matches!(
            parse_request_head("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n"),
            Err(ParseError::BadRequest(_))
        ));
        assert!(matches!(
            parse_request_head("POST / HTTP/1.1\r\nContent-Length: nope\r\n"),
            Err(ParseError::BadRequest(_))
        ));
        assert!(matches!(
            parse_request_head("GET http://example.com/ HTTP/1.1\r\n"),
            Err(ParseError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_bad_percent_escapes() {
        assert!(matches!(
            parse_request_head("GET /%zz HTTP/1.1\r\n"),
            Err(ParseError::BadRequest(_))
        ));
    }

    #[test]
    fn response_carries_the_security_headers() {
        let response = Response::error(403, "forbidden", "no");
        assert_eq!(response.status, 403);
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        let body = String::from_utf8(response.body).expect("json body");
        assert!(body.contains("\"ok\":false"));
        assert!(body.contains("\"code\":\"forbidden\""));
    }
}
