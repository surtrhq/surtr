//! The local REST and WebSocket API (plan, section 5.6), served by
//! `surtr serve --api`, and the JSON answers it shares with the MCP server.
//!
//! Everything is read-only and under `/api/v1`. Requests need the bearer
//! token from `surtr token`; the server binds to loopback only and refuses
//! browser origins and non-loopback `Host` names (see [`guard`]).

use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use surtr_core::Hub;
use tokio::sync::watch;

pub mod answers;
pub mod guard;
pub mod token;

use answers::{HistoryQuery, SensorFilter, data};

/// What the handlers share.
#[derive(Clone)]
struct Api {
    hub: Arc<Hub>,
    db: PathBuf,
    /// Changes after every read of the daemon; live sockets push then.
    reads: watch::Receiver<u64>,
}

/// The `/api/v1` routes, answering from `hub` (the daemon's latest read)
/// and `db`, to requests that pass [`guard::check`] with `token`. `reads`
/// should change after every read; when its sender is dropped, live sockets
/// close.
pub fn router(hub: Arc<Hub>, db: PathBuf, reads: watch::Receiver<u64>, token: String) -> Router {
    let token = Arc::new(token);
    Router::new()
        .route("/api/v1/system", get(system))
        .route("/api/v1/sensors", get(sensors))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/gpu/processes", get(gpu_processes))
        .route("/api/v1/gpu/throttle", get(throttle))
        .route("/api/v1/history", get(history))
        .route("/api/v1/live", get(live))
        .with_state(Api { hub, db, reads })
        .layer(middleware::from_fn(move |request, next| {
            guard::check(Arc::clone(&token), request, next)
        }))
}

/// A JSON response, or a JSON error with `status`.
fn reply(result: Result<String, String>, status: StatusCode) -> Response {
    match result {
        Ok(text) => ([(header::CONTENT_TYPE, "application/json")], text).into_response(),
        Err(e) => (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "error": e }).to_string(),
        )
            .into_response(),
    }
}

/// Runs `f` with the hub on a blocking thread and replies with its JSON.
async fn answer<T: serde::Serialize + Send + 'static>(
    api: &Api,
    f: impl FnOnce(&Hub) -> T + Send + 'static,
) -> Response {
    let hub = Arc::clone(&api.hub);
    let result = tokio::task::spawn_blocking(move || f(&hub))
        .await
        .map_err(|e| e.to_string())
        .and_then(|v| data(&v));
    reply(result, StatusCode::INTERNAL_SERVER_ERROR)
}

async fn system(State(api): State<Api>) -> Response {
    answer(&api, answers::system).await
}

async fn sensors(State(api): State<Api>, Query(filter): Query<SensorFilter>) -> Response {
    answer(&api, move |hub| answers::sensors(hub.read(), &filter)).await
}

async fn capabilities(State(api): State<Api>) -> Response {
    answer(&api, Hub::capabilities).await
}

async fn gpu_processes(State(api): State<Api>) -> Response {
    answer(&api, Hub::gpu_processes).await
}

async fn throttle(State(api): State<Api>) -> Response {
    answer(&api, Hub::throttle_reasons).await
}

async fn history(State(api): State<Api>, Query(query): Query<HistoryQuery>) -> Response {
    let db = api.db.clone();
    let result = tokio::task::spawn_blocking(move || answers::history(&db, &query))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .and_then(|v| data(&v));
    reply(result, StatusCode::NOT_FOUND)
}

/// Sends the (filtered) sensors after every read until either side stops.
async fn live(
    State(api): State<Api>,
    Query(filter): Query<SensorFilter>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| push(api, filter, socket))
}

async fn push(api: Api, filter: SensorFilter, mut socket: WebSocket) {
    let mut reads = api.reads.clone();
    loop {
        let hub = Arc::clone(&api.hub);
        let filter = filter.clone();
        let text = tokio::task::spawn_blocking(move || {
            data(&answers::sensors(hub.read(), &filter)).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
        tokio::select! {
            changed = reads.changed() => if changed.is_err() { break },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                // Pings are answered by axum; other messages are ignored.
                Some(Ok(_)) => continue,
            },
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}
