//! Lado do cliente MCP: o endpoint HTTP local (Streamable HTTP).
//!
//! `POST /mcp` com `Authorization: Bearer <token>` (preferido) ou
//! `POST /mcp/<token>` para os clientes que não deixam configurar headers —
//! nesse caso o token fica em ficheiros de configuração e logs do cliente.
//! Só aceita pedidos de `127.0.0.1`/`localhost` e sem `Origin` (um browser a
//! chamar isto é DNS rebinding ou um site a tentar a sorte).
//!
//! Sessões: a resposta ao `initialize` leva um `Mcp-Session-Id`, que o cliente
//! repete nos pedidos seguintes; é assim que o bridge sabe quem pediu o quê.
//! Uma sessão desconhecida dá 404 (o cliente volta a inicializar, como o MCP
//! manda). Pedidos sem sessão continuam a ser aceites; o shim `--stdio` diz
//! quem é o cliente no cabeçalho `X-1fluxo-Client`.

use crate::{hub::Hub, proto};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 4 * 1024 * 1024;
const SESSION_HEADER: &str = "mcp-session-id";
pub const CLIENT_HEADER: &str = "x-1fluxo-client";

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/mcp", post(post_header).get(not_allowed).delete(delete_header))
        .route("/mcp/{token}", post(post_path).get(not_allowed).delete(delete_path))
        .route("/health", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(hub)
}

pub async fn serve(hub: Arc<Hub>, listener: TcpListener, cancel: CancellationToken) {
    let _ = axum::serve(listener, router(hub))
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await;
}

async fn not_allowed() -> StatusCode {
    // Sem fluxo SSE do servidor para o cliente: o tab só responde a pedidos.
    StatusCode::METHOD_NOT_ALLOWED
}

async fn post_header(State(hub): State<Arc<Hub>>, headers: HeaderMap, body: Bytes) -> Response {
    handle(hub, headers, None, body).await
}

async fn post_path(State(hub): State<Arc<Hub>>, Path(token): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    handle(hub, headers, Some(token), body).await
}

async fn delete_header(State(hub): State<Arc<Hub>>, headers: HeaderMap) -> Response {
    end_session(hub, headers, None)
}

async fn delete_path(State(hub): State<Arc<Hub>>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    end_session(hub, headers, Some(token))
}

/// `DELETE /mcp`: o cliente termina a sessão.
fn end_session(hub: Arc<Hub>, headers: HeaderMap, path_token: Option<String>) -> Response {
    if let Err(r) = check_access(&hub, &headers, path_token.as_deref()) {
        return r;
    }
    match session_id(&headers) {
        Some(id) if hub.close_session(id) => StatusCode::OK.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn session_id(headers: &HeaderMap) -> Option<&str> {
    headers.get(SESSION_HEADER)?.to_str().ok()
}

/// Loopback, sem `Origin`, com o token certo.
fn check_access(hub: &Hub, headers: &HeaderMap, path_token: Option<&str>) -> Result<(), Response> {
    if !host_ok(headers) || headers.contains_key(header::ORIGIN) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    let supplied = path_token.or_else(|| bearer(headers)).unwrap_or("");
    if !hub.endpoint_token_matches(supplied) {
        return Err((StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")]).into_response());
    }
    Ok(())
}

fn host_ok(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))
}

async fn handle(hub: Arc<Hub>, headers: HeaderMap, path_token: Option<String>, body: Bytes) -> Response {
    if let Err(r) = check_access(&hub, &headers, path_token.as_deref()) {
        return r;
    }
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, -32700, "JSON inválido"),
    };
    if !msg.is_object() {
        // Lotes (arrays) não são suportados: o tab responde a um pedido de cada vez.
        return json_error(StatusCode::BAD_REQUEST, -32600, "Só são aceites pedidos JSON-RPC individuais");
    }

    // Quem é o cliente: o `initialize` abre uma sessão nova; os outros pedidos
    // vêm com a sessão (ou, pelo shim, com o cabeçalho).
    let from_header = headers
        .get(CLIENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(proto::decode_client_header);
    let mut new_session = None;
    let client = if proto::method(&msg) == Some("initialize") {
        let client = proto::client_info(&msg).or(from_header);
        new_session = Some(hub.open_session(client.clone()));
        client
    } else if let Some(id) = session_id(&headers) {
        match hub.session_client(id) {
            Some(client) => client.or(from_header),
            None => return json_error(StatusCode::NOT_FOUND, -32001, "Sessão desconhecida: volte a inicializar"),
        }
    } else {
        from_header
    };

    let mut response = match hub.handle_rpc(msg, client).await {
        Some(resp) => Json(resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    };
    if let Some(id) = new_session.and_then(|id| id.parse().ok()) {
        response.headers_mut().insert(SESSION_HEADER, id);
    }
    response
}

fn json_error(status: StatusCode, code: i64, message: &str) -> Response {
    (status, Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } }))).into_response()
}
