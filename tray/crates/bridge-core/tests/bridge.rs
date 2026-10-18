//! Testes de ponta a ponta sobre o bridge real, a falar-lhe como o tab (WebSocket)
//! e como um cliente MCP (HTTP). Portam os de `test/pairing.test.mjs` do bridge
//! em TypeScript e acrescentam o endpoint HTTP e o remapeamento de ids.

use bridge_core::{pairing, shim::post_json, start, Config, Running};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
    MaybeTlsStream, WebSocketStream,
};

const ORIGIN: &str = "http://localhost:3000";
type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn boot(dir: &Path) -> Running {
    start(Config {
        ws_port: 0,
        http_port: 0,
        app_url: ORIGIN.into(),
        origins: vec![],
        state_dir: dir.into(),
        open_browser: false,
    })
    .await
    .unwrap()
}

fn nonce() -> String {
    pairing::random_b64(24)
}

async fn connect(port: u16, origin: &str) -> Ws {
    let mut req = format!("ws://127.0.0.1:{port}").into_client_request().unwrap();
    req.headers_mut().insert("origin", origin.parse().unwrap());
    connect_async(req).await.unwrap().0
}

enum Reply {
    Message(Value),
    Closed(u16),
}

async fn handshake(port: u16, hello: Value) -> (Ws, Reply) {
    let mut ws = connect(port, ORIGIN).await;
    ws.send(Message::text(hello.to_string())).await.unwrap();
    let reply = loop {
        match timeout(Duration::from_secs(5), ws.next()).await.expect("handshake sem resposta") {
            Some(Ok(Message::Text(t))) => break Reply::Message(serde_json::from_str(t.as_str()).unwrap()),
            Some(Ok(Message::Close(Some(f)))) => break Reply::Closed(u16::from(f.code)),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break Reply::Closed(0),
        }
    };
    (ws, reply)
}

fn message(r: Reply) -> Value {
    match r {
        Reply::Message(v) => v,
        Reply::Closed(c) => panic!("fechado com {c}"),
    }
}

fn closed(r: Reply) -> u16 {
    match r {
        Reply::Closed(c) => c,
        Reply::Message(v) => panic!("devia ter fechado, veio {v}"),
    }
}

/// Parâmetros de ligação lidos do estado do bridge.
struct Info {
    ws_port: u16,
    http_port: u16,
    token: String,
    endpoint_token: String,
}

fn info(run: &Running) -> Info {
    let s = run.status().borrow().clone();
    let url = run.hub.pair_url();
    let token = url.split("token=").nth(1).unwrap().to_string();
    Info { ws_port: s.ws_port, http_port: s.http_port, token, endpoint_token: run.hub.endpoint_token() }
}

async fn rpc(i: &Info, body: Value) -> (u16, Value) {
    let (status, text) = post_json(i.http_port, &i.endpoint_token, &body.to_string()).await.unwrap();
    (status, if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap() })
}

#[tokio::test]
async fn primeiro_emparelhamento_devolve_o_segredo_e_grava_em_disco() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    // O cliente MCP apresenta-se primeiro; o nome tem de chegar ao tab.
    rpc(&i, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cliente-de-teste","version":"9"}}})).await;

    let n = nonce();
    let (_ws, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":n})).await;
    let m = message(r);
    assert_eq!(m["type"], "paired");
    assert_eq!(m["client"], json!({"name":"cliente-de-teste","version":"9"}));
    assert_eq!(m["proof"], pairing::hmac_b64(&i.token, &n), "prova do token");
    let p = pairing::load(dir.path()).expect("ficheiro de emparelhamento");
    assert_eq!(p.id, m["pairing"]["id"]);
    assert_eq!(p.secret, m["pairing"]["secret"]);
    assert_eq!(p.origin, ORIGIN);
    assert!(p.valid());
    run.stop().await;
}

#[tokio::test]
async fn retoma_com_a_prova_certa_e_recusa_a_errada_com_4011() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (mut first, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    let m = message(r);
    let (id, secret) = (m["pairing"]["id"].as_str().unwrap().to_string(), m["pairing"]["secret"].as_str().unwrap().to_string());
    first.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (_w, bad) = handshake(i.ws_port, json!({"type":"resume","pairingId":id,"nonce":nonce(),"proof":"nope"})).await;
    assert_eq!(closed(bad), 4011);

    let n = nonce();
    let proof = pairing::hmac_b64(&secret, &format!("tab:{n}"));
    let (_w, good) = handshake(i.ws_port, json!({"type":"resume","pairingId":id,"nonce":n,"proof":proof})).await;
    let g = message(good);
    assert_eq!(g["type"], "paired");
    assert_eq!(g["proof"], pairing::hmac_b64(&secret, &format!("bridge:{n}")));
    assert_eq!(g["pairing"]["id"], id.as_str());
    assert!(g["pairing"].get("secret").is_none(), "o segredo não volta a passar no fio");
    run.stop().await;
}

#[tokio::test]
async fn retoma_depois_de_reiniciar_o_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (mut w, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    let m = message(r);
    let (id, secret) = (m["pairing"]["id"].as_str().unwrap().to_string(), m["pairing"]["secret"].as_str().unwrap().to_string());
    w.close(None).await.unwrap();
    run.stop().await;

    let run = boot(dir.path()).await;
    let i = info(&run);
    assert!(run.status().borrow().paired);
    let n = nonce();
    let proof = pairing::hmac_b64(&secret, &format!("tab:{n}"));
    let (_w, r) = handshake(i.ws_port, json!({"type":"resume","pairingId":id,"nonce":n,"proof":proof})).await;
    assert_eq!(message(r)["type"], "paired");
    run.stop().await;
}

#[tokio::test]
async fn emparelhamento_expirado_e_apagado_no_arranque() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pairing::Pairing::new(ORIGIN);
    p.expires_at = "2020-01-01T00:00:00.000Z".into();
    pairing::save(dir.path(), &p).unwrap();
    let run = boot(dir.path()).await;
    assert!(pairing::load(dir.path()).is_none());
    let i = info(&run);
    let n = nonce();
    let proof = pairing::hmac_b64(&p.secret, &format!("tab:{n}"));
    let (_w, r) = handshake(i.ws_port, json!({"type":"resume","pairingId":p.id,"nonce":n,"proof":proof})).await;
    assert_eq!(closed(r), 4011);
    run.stop().await;
}

#[tokio::test]
async fn forget_do_tab_apaga_o_emparelhamento() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (mut w, _) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    assert!(pairing::load(dir.path()).is_some());
    w.send(Message::text(json!({"type":"forget"}).to_string())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(pairing::load(dir.path()).is_none());
    run.stop().await;
}

#[tokio::test]
async fn origem_nao_autorizada_e_um_segundo_tab_sao_recusados() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);

    let mut evil = connect(i.ws_port, "https://evil.example").await;
    let code = match timeout(Duration::from_secs(5), evil.next()).await.unwrap() {
        Some(Ok(Message::Close(Some(f)))) => u16::from(f.code),
        other => panic!("esperava fecho, veio {other:?}"),
    };
    assert_eq!(code, 4003);

    let (_first, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    message(r);
    let mut second = connect(i.ws_port, ORIGIN).await;
    let code = match timeout(Duration::from_secs(5), second.next()).await.unwrap() {
        Some(Ok(Message::Close(Some(f)))) => u16::from(f.code),
        other => panic!("esperava fecho, veio {other:?}"),
    };
    assert_eq!(code, 4009);
    run.stop().await;
}

#[tokio::test]
async fn sem_tab_initialize_e_tools_list_respondem_localmente() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (status, init) = rpc(&i, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"teste","version":"1"}}})).await;
    assert_eq!(status, 200);
    assert_eq!(init["result"]["serverInfo"]["name"], "1fluxo");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("connect?port="));
    let (_, list) = rpc(&i, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_months") && names.contains(&"search_movements"), "{names:?}");
    assert_eq!(names.len(), 7);
    let (status, _) = rpc(&i, json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    assert_eq!(status, 202);
    run.stop().await;
}

#[tokio::test]
async fn endpoint_exige_token_e_host_local() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let body = json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string();
    let (status, _) = post_json(i.http_port, "errado", &body).await.unwrap();
    assert_eq!(status, 401);
    let (status, _) = post_json(i.http_port, "", &body).await.unwrap();
    assert_eq!(status, 401);
    let (status, text) = post_json(i.http_port, &i.endpoint_token, &body).await.unwrap();
    assert_eq!((status, serde_json::from_str::<Value>(&text).unwrap()["result"].clone()), (200, json!({})));
    run.stop().await;
}

/// O tab de mentira responde a cada pedido com o nome da tool, na ordem inversa
/// à de chegada, para provar que a resposta certa vai ao cliente certo.
#[tokio::test]
async fn dois_clientes_com_o_mesmo_id_recebem_cada_um_a_sua_resposta() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (mut tab, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    message(r);

    let call = |name: &'static str| {
        let (port, token) = (i.http_port, i.endpoint_token.clone());
        tokio::spawn(async move {
            let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":{}}});
            let (s, t) = post_json(port, &token, &body.to_string()).await.unwrap();
            (s, serde_json::from_str::<Value>(&t).unwrap())
        })
    };
    let a = call("claude_code");
    let b = call("cursor");

    // O tab recebe dois pedidos com ids internos distintos e diferentes de 1.
    let mut received = Vec::new();
    while received.len() < 2 {
        if let Some(Ok(Message::Text(t))) = timeout(Duration::from_secs(5), tab.next()).await.unwrap() {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["method"] == "tools/call" {
                received.push(v);
            }
        }
    }
    assert_ne!(received[0]["id"], received[1]["id"]);
    for v in received.iter().rev() {
        let name = v["params"]["name"].as_str().unwrap();
        let reply = json!({"jsonrpc":"2.0","id":v["id"],"result":{"content":[{"type":"text","text":name}]}});
        tab.send(Message::text(reply.to_string())).await.unwrap();
    }

    let (sa, ra) = a.await.unwrap();
    let (sb, rb) = b.await.unwrap();
    assert_eq!((sa, sb), (200, 200));
    assert_eq!(ra["id"], 1);
    assert_eq!(rb["id"], 1);
    assert_eq!(ra["result"]["content"][0]["text"], "claude_code");
    assert_eq!(rb["result"]["content"][0]["text"], "cursor");
    run.stop().await;
}

#[tokio::test]
async fn tab_que_fecha_a_meio_devolve_erro_legivel() {
    let dir = tempfile::tempdir().unwrap();
    let run = boot(dir.path()).await;
    let i = info(&run);
    let (mut tab, r) = handshake(i.ws_port, json!({"type":"pair","token":i.token,"nonce":nonce()})).await;
    message(r);
    let (port, token) = (i.http_port, i.endpoint_token.clone());
    let call = tokio::spawn(async move {
        let body = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_months","arguments":{}}});
        post_json(port, &token, &body.to_string()).await.unwrap()
    });
    // Espera que o pedido chegue ao tab e fecha sem responder.
    loop {
        if let Some(Ok(Message::Text(t))) = tab.next().await {
            if t.as_str().contains("tools/call") {
                break;
            }
        }
    }
    tab.close(None).await.unwrap();
    let (status, text) = call.await.unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(status, 200);
    assert_eq!(v["id"], 7);
    assert_eq!(v["error"]["code"], -32000);
    assert!(v["error"]["message"].as_str().unwrap().contains("fechou a ligação"));
    run.stop().await;
}
