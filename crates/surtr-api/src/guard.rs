//! The checks every local HTTP request passes before it is answered: the
//! bearer token, a loopback `Host`, and no browser `Origin`.

use std::sync::Arc;

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::token;

/// Whether `host` (a `Host` header value) names this machine: `localhost`,
/// `127.0.0.1` or `[::1]`, with or without a port. Anything else may be a
/// DNS-rebinding attack, where a website's name resolves to 127.0.0.1.
pub fn loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

/// Why `headers` are refused, if they are.
pub fn refuse(expected: &str, headers: &HeaderMap) -> Option<(StatusCode, &'static str)> {
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !token::matches(expected, given) {
        return Some((
            StatusCode::UNAUTHORIZED,
            "missing or wrong bearer token; see `surtr token`\n",
        ));
    }
    // Browsers always send Origin; command-line clients and agents don't.
    if headers.contains_key(header::ORIGIN) {
        return Some((
            StatusCode::FORBIDDEN,
            "requests from web pages are not accepted\n",
        ));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !loopback_host(host) {
        return Some((StatusCode::FORBIDDEN, "Host must be a loopback name\n"));
    }
    None
}

/// Middleware: passes the request on only if [`refuse`] finds nothing.
pub async fn check(expected: Arc<String>, request: Request, next: Next) -> Response {
    match refuse(&expected, request.headers()) {
        None => next.run(request).await,
        Some((status, text)) => {
            (status, [(header::WWW_AUTHENTICATE, "Bearer")], text).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn hosts() {
        for ok in [
            "localhost",
            "localhost:7878",
            "127.0.0.1:1",
            "[::1]:80",
            "[::1]",
        ] {
            assert!(loopback_host(ok), "{ok}");
        }
        for bad in [
            "",
            "evil.example",
            "evil.example:7878",
            "127.0.0.2",
            "localhost.evil.example",
        ] {
            assert!(!loopback_host(bad), "{bad}");
        }
    }

    #[test]
    fn refusals() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7878"));
        assert_eq!(refuse("t", &h).unwrap().0, StatusCode::UNAUTHORIZED);
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer t"));
        assert_eq!(refuse("t", &h), None);
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert_eq!(refuse("t", &h).unwrap().0, StatusCode::FORBIDDEN);
        h.remove(header::ORIGIN);
        h.insert(header::HOST, HeaderValue::from_static("evil.example:7878"));
        assert_eq!(refuse("t", &h).unwrap().0, StatusCode::FORBIDDEN);
    }
}
