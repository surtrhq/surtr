//! MCP over streamable HTTP for `surtr serve --mcp` (plan, section 6.1):
//! loopback only, every request needs the bearer token, and requests from
//! web pages are refused.

use std::sync::Arc;

use axum::{Router, middleware};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use surtr_api::guard;
use tokio_util::sync::CancellationToken;

use crate::Server;

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

/// The `/mcp` endpoint, answering with `server` to requests that pass the
/// same checks as the REST API: the bearer token `expected`, a loopback
/// `Host`, and no browser `Origin`.
pub fn router(server: Server, expected: String) -> (Router, Stop) {
    let stop = Stop::default();
    // rmcp checks Host and Origin as well; the guard below makes the rules
    // identical for MCP and the REST API.
    let config = StreamableHttpServerConfig::default()
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
        .layer(middleware::from_fn(move |request, next| {
            guard::check(Arc::clone(&expected), request, next)
        }));
    (router, stop)
}
