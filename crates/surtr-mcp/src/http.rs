//! MCP over streamable HTTP for `surtr serve --mcp` (plan, section 6.1):
//! loopback only, every request needs the bearer token, and requests from
//! web pages are refused.

use std::sync::Arc;

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use crate::{Server, token};

/// Largest request body accepted.
const MAX_BODY: usize = 64 * 1024;

/// Ends open MCP sessions (their event streams would otherwise keep the
/// HTTP server's graceful shutdown waiting).
#[derive(Debug, Clone, Default)]
pub struct Stop(CancellationToken);

impl Stop {
    /// Ends every session and refuses new requests.
    pub fn stop(&self) {
        self.0.cancel();
    }
}

/// The `/mcp` endpoint, answering with `server` to requests that carry
/// `Authorization: Bearer <expected>`.
pub fn router(server: Server, expected: String) -> (Router, Stop) {
    let stop = Stop::default();
    let config = StreamableHttpServerConfig::default()
        // Host must be a loopback name (the default), against DNS rebinding;
        // any browser Origin is refused, so web pages can't call us.
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY)
        .with_cancellation_token(stop.0.clone());
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let expected = Arc::new(expected);
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            let expected = Arc::clone(&expected);
            async move { authorize(&expected, request, next).await }
        }));
    (router, stop)
}

async fn authorize(expected: &str, request: Request, next: Next) -> Response {
    let given = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if token::matches(expected, given) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or wrong bearer token; see `surtr mcp token`\n",
        )
            .into_response()
    }
}
