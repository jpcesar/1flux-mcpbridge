//! Lado do tab: o servidor WebSocket em 127.0.0.1.
//!
//! Só aceita uma ligação de cada vez, e só se o `Origin` estiver na lista
//! autorizada **e** o handshake for válido (token de uso único, ou prova do
//! segredo). Códigos de fecho como no bridge em TypeScript:
//! 4001 token inválido · 4003 origem · 4008 sem handshake a tempo ·
//! 4009 já há um tab · 4010 emparelhamento expirado · 4011 prova inválida ·
//! 4029 demasiadas ligações por emparelhar.

use crate::hub::{log, Attach, Hub, WS_PORT_FALLBACKS};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::{TcpListener, TcpStream}, sync::mpsc, time::timeout};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        protocol::{frame::coding::CloseCode, CloseFrame},
        Message,
    },
};
use tokio_util::sync::CancellationToken;

/// Tempo que o tab tem para mandar o handshake depois de abrir o socket.
const PAIR_TIMEOUT: Duration = Duration::from_secs(5);

/// Porta pedida, depois as seguintes (o tab varre a mesma gama); só no fim uma
/// qualquer. `0` é «qualquer», sem gama.
pub async fn bind(port: u16) -> io::Result<TcpListener> {
    if port != 0 {
        for p in port..port.saturating_add(WS_PORT_FALLBACKS) {
            match TcpListener::bind(("127.0.0.1", p)).await {
                Ok(l) => return Ok(l),
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => log(&format!("Porta {p} ocupada; a tentar a seguinte.")),
                Err(e) => return Err(e),
            }
        }
    }
    TcpListener::bind(("127.0.0.1", 0)).await
}

pub async fn serve(hub: Arc<Hub>, listener: TcpListener, cancel: CancellationToken) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let (hub, cancel) = (hub.clone(), cancel.clone());
                    tokio::spawn(async move { connection(hub, stream, cancel).await });
                }
                Err(e) => log(&format!("Erro no servidor WebSocket: {e}")),
            }
        }
    }
}

async fn close(ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>, code: u16, reason: &'static str) {
    let _ = ws
        .close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }))
        .await;
}

async fn connection(hub: Arc<Hub>, stream: TcpStream, cancel: CancellationToken) {
    let origin_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot = origin_slot.clone();
    let callback = move |req: &Request, resp: Response| {
        *slot.lock().unwrap() = req
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(resp)
    };
    let Ok(mut ws) = accept_hdr_async(stream, callback).await else { return };
    let origin = origin_slot.lock().unwrap().clone();

    let allowed = hub.cfg.allowed_origins();
    let Some(origin) = origin.filter(|o| allowed.contains(o)) else {
        log("Ligação recusada: origem não autorizada.");
        close(&mut ws, 4003, "origin not allowed").await;
        return;
    };
    if hub.tab_present() {
        log("Ligação recusada: já há um tab ligado.");
        close(&mut ws, 4009, "already paired").await;
        return;
    }
    let Some(mut slot) = hub.begin_pending() else {
        log("Ligação recusada: demasiados emparelhamentos pendentes.");
        close(&mut ws, 4029, "too many pending").await;
        return;
    };

    let first = match timeout(PAIR_TIMEOUT, ws.next()).await {
        Err(_) => {
            log("Emparelhamento expirou sem token.");
            close(&mut ws, 4008, "pairing timeout").await;
            return;
        }
        Ok(Some(Ok(Message::Text(t)))) => t,
        Ok(_) => return,
    };
    slot.settle();

    let hello: Value = serde_json::from_str(first.as_str()).unwrap_or(Value::Null);
    let nonce = hello
        .get("nonce")
        .and_then(Value::as_str)
        .filter(|n| (16..=128).contains(&n.len()))
        .map(str::to_owned);
    let Some(nonce) = nonce else {
        log("Ligação recusada: pedido de emparelhamento inválido.");
        close(&mut ws, 4001, "bad token").await;
        return;
    };
    let str_field = |k: &str| hello.get(k).and_then(Value::as_str).unwrap_or("").to_owned();

    let mode = match hello.get("type").and_then(Value::as_str) {
        Some("resume") => match hub.check_resume(&str_field("pairingId"), &str_field("proof"), &nonce) {
            Ok(p) => Attach::Resume(p),
            Err((code, reason)) => {
                close(&mut ws, code, reason).await;
                return;
            }
        },
        Some("pair") if hub.token_matches(&str_field("token")) => Attach::Pair { origin },
        _ => {
            log("Ligação recusada: token de emparelhamento inválido.");
            close(&mut ws, 4001, "bad token").await;
            return;
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let Some((tab_id, hello_json)) = hub.attach_tab(&nonce, mode, tx) else {
        // Dois tabs a emparelhar ao mesmo tempo — fica o primeiro.
        close(&mut ws, 4009, "already paired").await;
        return;
    };
    if ws.send(Message::text(hello_json)).await.is_err() {
        hub.detach_tab(tab_id);
        return;
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                close(&mut ws, 1001, "bridge shutting down").await;
                break;
            }
            out = rx.recv() => match out {
                Some(text) => if ws.send(Message::text(text)).await.is_err() { break },
                None => break,
            },
            incoming = ws.next() => match incoming {
                Some(Ok(Message::Text(t))) => hub.on_tab_message(t.as_str()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    hub.detach_tab(tab_id);
}
