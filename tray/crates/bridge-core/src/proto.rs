//! JSON-RPC / MCP: constantes e as respostas que o bridge dá localmente.
//!
//! O bridge não interpreta tools. As únicas excepções, só quando não há tab:
//! `initialize`, `ping` e `tools/list` (com a cópia de `tools.json`), para o
//! cliente não dar o servidor como morto ao arrancar.

use serde_json::{json, Value};
use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Código JSON-RPC (gama "server error") para "sem tab ligado".
pub const ERR_NO_TAB: i64 = -32000;

/// Versões MCP conhecidas, a mais recente primeiro. Espelha
/// `src/lib/mcp/protocol.ts` na web app.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Cópia das tools da web app (`tools.json` na raiz do repo, verificada por
/// `tools-sync.test.ts` no 1flux-app). Serve o `tools/list` sem tab.
pub fn tools() -> &'static [Value] {
    static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        match serde_json::from_str::<Value>(include_str!("../../../../tools.json")) {
            Ok(Value::Array(a)) => a,
            _ => Vec::new(),
        }
    })
}

pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `id` válido de um pedido: string ou número.
pub fn request_id(msg: &Value) -> Option<Value> {
    match msg.get("id") {
        Some(v) if v.is_string() || v.is_number() => Some(v.clone()),
        _ => None,
    }
}

pub fn method(msg: &Value) -> Option<&str> {
    msg.get("method").and_then(Value::as_str)
}

/// Nome e versão do cliente MCP (`clientInfo` do `initialize`).
pub fn client_info(msg: &Value) -> Option<(String, Option<String>)> {
    let info = msg.get("params")?.get("clientInfo")?;
    let name: String = info.get("name")?.as_str()?.trim().chars().take(80).collect();
    if name.is_empty() {
        return None;
    }
    let version = info
        .get("version")
        .and_then(Value::as_str)
        .map(|v| v.chars().take(40).collect::<String>());
    Some((name, version))
}

pub fn local_initialize(id: &Value, msg: &Value, instructions: &str) -> Value {
    let requested = msg
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let protocol_version = if PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        PROTOCOL_VERSIONS[0]
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": protocol_version,
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": { "name": "1fluxo", "version": VERSION },
            "instructions": instructions,
        }
    })
}

/// Respostas locais sem tab: `initialize`, `ping`, `tools/list`.
pub fn local_answer(msg: &Value, instructions: &str) -> Option<Value> {
    let id = request_id(msg)?;
    match method(msg)? {
        "initialize" => Some(local_initialize(&id, msg, instructions)),
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/list" if !tools().is_empty() => {
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools() } }))
        }
        _ => None,
    }
}

/* ── quem é o cliente de cada pedido ────────────────────────────────────── */

/// Nome e versão de um cliente MCP (o `clientInfo` do `initialize`).
pub type ClientInfo = (String, Option<String>);

/// Chave em `params._meta` com o cliente que fez o pedido. O MCP reserva o
/// `_meta` para metadados e pede um prefixo próprio nas chaves; o tab que não a
/// conhece ignora-a.
pub const CLIENT_META_KEY: &str = "1fluxo.com/client";

pub fn client_json((name, version): &ClientInfo) -> Value {
    match version {
        Some(v) => json!({ "name": name, "version": v }),
        None => json!({ "name": name }),
    }
}

/// Acrescenta o cliente a `params._meta` de uma mensagem que segue para o tab,
/// sem tocar no resto dos `params`.
pub fn with_client_meta(msg: &mut Value, client: &ClientInfo) {
    let Some(obj) = msg.as_object_mut() else { return };
    let params = obj.entry("params").or_insert_with(|| json!({}));
    let Some(params) = params.as_object_mut() else { return };
    let meta = params.entry("_meta").or_insert_with(|| json!({}));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(CLIENT_META_KEY.to_string(), client_json(client));
    }
}

/// O shim `--stdio` diz ao endpoint quem é o cliente num cabeçalho: o JSON do
/// `clientInfo` em base64url (os nomes podem ter caracteres fora do ASCII).
pub fn encode_client_header(client: &ClientInfo) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(client_json(client).to_string())
}

pub fn decode_client_header(value: &str) -> Option<ClientInfo> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let raw = URL_SAFE_NO_PAD.decode(value.trim()).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    client_info(&json!({ "params": { "clientInfo": v } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_header_roundtrip() {
        let c: ClientInfo = ("Cliente é ótimo".into(), Some("2.1".into()));
        assert_eq!(decode_client_header(&encode_client_header(&c)), Some(c));
        assert_eq!(decode_client_header("lixo"), None);
    }

    #[test]
    fn client_meta_keeps_params() {
        let mut m = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": "list_months", "_meta": { "outro": 1 } } });
        with_client_meta(&mut m, &("claude-code".into(), None));
        assert_eq!(m["params"]["name"], "list_months");
        assert_eq!(m["params"]["_meta"]["outro"], 1);
        assert_eq!(m["params"]["_meta"][CLIENT_META_KEY], json!({ "name": "claude-code" }));

        let mut ping = json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" });
        with_client_meta(&mut ping, &("x".into(), Some("1".into())));
        assert_eq!(ping["params"]["_meta"][CLIENT_META_KEY]["version"], "1");
    }
}
