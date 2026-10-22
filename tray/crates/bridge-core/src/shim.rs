//! Modo `--stdio`: para os clientes que só sabem lançar um processo e falar
//! por stdin/stdout (o Claude Desktop). Não abre bridge nem tray: reencaminha
//! cada linha JSON-RPC para o endpoint HTTP do tray que já está a correr e
//! devolve a resposta no stdout.
//!
//! Se o tray não estiver a correr, `initialize`, `ping` e `tools/list`
//! respondem-se aqui (senão o cliente dava o servidor como morto ao arrancar)
//! e o resto falha com um erro legível a pedir para abrir a app.
//!
//! O shim guarda o `clientInfo` do `initialize` e manda-o em cada pedido no
//! cabeçalho `X-1fluxo-Client`, para o bridge dizer ao tab quem pediu (sem
//! depender de sessões, que se perdem quando o tray reinicia).

use crate::{
    pairing::ENDPOINT_FILE,
    proto::{self, error_response, ERR_NO_TAB},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
};

const NOT_RUNNING: &str =
    "A app do bridge do 1fluxo não está a correr. Abre-a (ícone no tray ou na barra de menus) e tenta outra vez.";

/// O que o tray escreve ao arrancar para o shim o encontrar.
#[derive(Serialize, Deserialize)]
pub struct EndpointInfo {
    pub port: u16,
    pub token: String,
}

pub fn write_endpoint_info(dir: &Path, info: &EndpointInfo) -> std::io::Result<()> {
    crate::pairing::write_private(dir, ENDPOINT_FILE, &serde_json::to_string(info)?)
}

pub fn read_endpoint_info(dir: &Path) -> Option<EndpointInfo> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(ENDPOINT_FILE)).ok()?).ok()
}

/// `POST /mcp` mínimo sobre TCP, sem dependências: é loopback, uma ligação por pedido.
pub async fn post_json(port: u16, token: &str, body: &str) -> std::io::Result<(u16, String)> {
    post_json_with(port, token, body, &[]).await
}

/// Como `post_json`, com cabeçalhos extra (o cliente, para o shim).
pub async fn post_json_with(port: u16, token: &str, body: &str, extra: &[(&str, &str)]) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
    let extra: String = extra.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await?;
    parse_response(&raw).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "resposta HTTP inválida"))
}

fn parse_response(raw: &[u8]) -> Option<(u16, String)> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = &raw[split + 4..];
    let chunked = head.lines().any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    let body = if chunked { dechunk(body)? } else { body.to_vec() };
    Some((status, String::from_utf8(body).ok()?))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&b[..eol]).ok()?.split(';').next()?.trim(), 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

/// Lê stdin linha a linha; cada pedido segue numa tarefa própria (uma tool
/// call lenta não bloqueia as seguintes) e as respostas saem pelo stdout.
pub async fn run_stdio(state_dir: &Path) -> anyhow::Result<()> {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.write_all(b"\n").await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let dir = state_dir.to_path_buf();
    // O cliente apresenta-se no `initialize`; guarda-se antes de o pedido seguir.
    let client: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if proto::method(&msg) == Some("initialize") {
            if let Some(info) = proto::client_info(&msg) {
                *client.lock().unwrap_or_else(|e| e.into_inner()) = Some(proto::encode_client_header(&info));
            }
        }
        let header = client.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (dir, tx) = (dir.clone(), out_tx.clone());
        tokio::spawn(async move {
            if let Some(resp) = forward(&dir, &line, &msg, header.as_deref()).await {
                let _ = tx.send(resp);
            }
        });
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

async fn forward(dir: &Path, line: &str, msg: &Value, client: Option<&str>) -> Option<String> {
    let id = proto::request_id(msg);
    if let Some(info) = read_endpoint_info(dir) {
        let extra: Vec<(&str, &str)> = client.map(|c| (crate::http::CLIENT_HEADER, c)).into_iter().collect();
        if let Ok((status, body)) = post_json_with(info.port, &info.token, line, &extra).await {
            return match status {
                200 if !body.trim().is_empty() => Some(body.trim().to_string()),
                200 | 202 => None,
                _ => id.map(|id| error_response(&id, ERR_NO_TAB, &format!("O bridge recusou o pedido (HTTP {status}).")).to_string()),
            };
        }
    }
    // Tray em baixo: o handshake responde-se aqui; o resto falha com erro legível.
    let id = id?;
    proto::local_answer(msg, NOT_RUNNING)
        .or_else(|| Some(error_response(&id, ERR_NO_TAB, NOT_RUNNING)))
        .map(|v| v.to_string())
}
