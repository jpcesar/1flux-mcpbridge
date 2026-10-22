//! O núcleo: um tab, vários clientes. Guarda o emparelhamento, o tab ligado,
//! os pedidos em voo e o remapeamento de ids.
//!
//! Remapeamento de ids: cada cliente MCP escolhe os seus ids (dois clientes
//! podem usar o `1`). Antes de um pedido ir ao tab o `id` é trocado por um
//! número interno único; quando a resposta volta repõe-se o original e entrega-se
//! ao cliente certo. Os pedidos pendentes de um cliente que desaparece são
//! limpos (guarda no `Drop`).
//!
//! Quem é cada cliente: o `initialize` abre uma sessão MCP (`Mcp-Session-Id`)
//! com o `clientInfo`; cada pedido dessa sessão segue para o tab com o nome do
//! cliente em `params._meta["1fluxo.com/client"]`, para o tab dizer quem pediu.

use crate::{
    open,
    pairing::{self, Pairing},
    proto::{self, error_response, ClientInfo, ERR_NO_TAB, VERSION},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot, watch, Notify},
    time::timeout_at,
};

pub const DEFAULT_WS_PORT: u16 = 17337;
/// Com a porta ocupada tenta-se as seguintes, por esta ordem, para o tab do
/// 1fluxo as poder varrer (espelha `BRIDGE_PORT_RANGE` na web app).
pub const WS_PORT_FALLBACKS: u16 = 10;
pub const DEFAULT_HTTP_PORT: u16 = 17350;
pub const DEFAULT_APP_URL: &str = "https://1fluxo.com";

const DEFAULT_ORIGINS: [&str; 2] = [
    "https://1fluxo.com",
    "https://www.1fluxo.com",
];
const MAX_PENDING_CONNECTIONS: usize = 3;
/// Entre aberturas automáticas do browser.
const OPEN_COOLDOWN: Duration = Duration::from_secs(60);
/// Um pedido sem tab espera isto; uma tool call espera mais, pelo «Permitir».
const NO_TAB_WAIT: Duration = Duration::from_secs(10);
const TOOL_CALL_WAIT: Duration = Duration::from_secs(60);
/// Sessões MCP em memória; acima disto sai a usada há mais tempo.
const MAX_SESSIONS: usize = 256;

pub fn log(message: &str) {
    eprintln!("[1flux-bridge] {message}");
}

/// `https://1fluxo.com/x` → `https://1fluxo.com`.
pub fn origin_of(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            format!("{}{}", &url[..i + 3], &rest[..end])
        }
        None => url.to_string(),
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Porta do WebSocket (0 = qualquer).
    pub ws_port: u16,
    /// Porta do endpoint MCP (0 = qualquer; só para testes: os clientes guardam o endereço).
    pub http_port: u16,
    pub app_url: String,
    pub origins: Vec<String>,
    pub state_dir: PathBuf,
    /// Abrir o browser sozinho quando uma tool call chega sem tab ligado.
    pub open_browser: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ws_port: DEFAULT_WS_PORT,
            http_port: DEFAULT_HTTP_PORT,
            app_url: DEFAULT_APP_URL.to_string(),
            origins: Vec::new(),
            state_dir: pairing::default_state_dir(),
            open_browser: true,
        }
    }
}

impl Config {
    pub fn allowed_origins(&self) -> Vec<String> {
        let mut v: Vec<String> = DEFAULT_ORIGINS.iter().map(|s| s.to_string()).collect();
        v.push(origin_of(&self.app_url));
        v.extend(self.origins.iter().map(|o| origin_of(o)));
        v
    }
}

/// O que o tray mostra.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub tab_connected: bool,
    pub paired: bool,
    pub ws_port: u16,
    pub http_port: u16,
    pub client: Option<String>,
}

struct TabHandle {
    id: u64,
    tx: mpsc::UnboundedSender<String>,
}

struct Pending {
    reply: oneshot::Sender<Value>,
    original_id: Value,
}

struct Session {
    client: Option<ClientInfo>,
    last_used: Instant,
}

struct Inner {
    tab: Option<TabHandle>,
    next_tab_id: u64,
    pairing: Option<Pairing>,
    /// Token de uso único do emparelhamento; renova-se quando o tab se desliga.
    token: String,
    token_used: bool,
    pending_connections: usize,
    ws_port: u16,
    http_port: u16,
    /// O último cliente que se apresentou ou pediu algo: vai no `paired`.
    client: Option<ClientInfo>,
    sessions: HashMap<String, Session>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    browser_opened_at: Option<Instant>,
    endpoint_token: String,
}

pub enum Attach {
    Pair { origin: String },
    Resume(Pairing),
}

pub struct Hub {
    pub cfg: Config,
    inner: Mutex<Inner>,
    tab_notify: Notify,
    status_tx: watch::Sender<Status>,
}

/// Remove o pedido pendente se o cliente desistir antes da resposta.
struct PendingGuard<'a> {
    hub: &'a Hub,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.hub.lock().pending.remove(&self.id);
    }
}

/// Uma ligação ainda por emparelhar; sem tecto qualquer processo local enchia
/// a fila.
pub struct PendingConn {
    hub: Arc<Hub>,
    settled: bool,
}

impl PendingConn {
    pub fn settle(&mut self) {
        if !self.settled {
            self.settled = true;
            let mut g = self.hub.lock();
            g.pending_connections = g.pending_connections.saturating_sub(1);
        }
    }
}

impl Drop for PendingConn {
    fn drop(&mut self) {
        self.settle();
    }
}

impl Hub {
    pub fn new(cfg: Config) -> (Arc<Hub>, watch::Receiver<Status>) {
        let stored = pairing::load(&cfg.state_dir);
        let pairing = match stored {
            Some(p) if !p.valid() => {
                pairing::clear(&cfg.state_dir);
                log("O emparelhamento guardado expirou; apagado.");
                None
            }
            other => other,
        };
        let (status_tx, status_rx) = watch::channel(Status::default());
        let hub = Arc::new(Hub {
            inner: Mutex::new(Inner {
                tab: None,
                next_tab_id: 1,
                pairing,
                token: pairing::random_b64(32),
                token_used: false,
                pending_connections: 0,
                ws_port: 0,
                http_port: 0,
                client: None,
                sessions: HashMap::new(),
                next_id: 1,
                pending: HashMap::new(),
                browser_opened_at: None,
                endpoint_token: load_or_create_endpoint_token(&cfg.state_dir),
            }),
            cfg,
            tab_notify: Notify::new(),
            status_tx,
        });
        (hub, status_rx)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self, g: &Inner, running: bool) {
        self.status_tx.send_replace(Status {
            running,
            tab_connected: g.tab.is_some(),
            paired: g.pairing.as_ref().is_some_and(Pairing::valid),
            ws_port: g.ws_port,
            http_port: g.http_port,
            client: g.client.as_ref().map(|(n, _)| n.clone()),
        });
    }

    pub fn set_ports(&self, ws_port: u16, http_port: u16) {
        let mut g = self.lock();
        g.ws_port = ws_port;
        g.http_port = http_port;
        self.publish(&g, true);
    }

    /// Chamado ao parar: o estado final vai para o tray.
    pub fn mark_stopped(&self) {
        let g = self.lock();
        self.publish(&g, false);
    }

    pub fn status(&self) -> Status {
        self.status_tx.borrow().clone()
    }

    pub fn pair_url(&self) -> String {
        let g = self.lock();
        format!("{}/connect?port={}&token={}", self.cfg.app_url, g.ws_port, g.token)
    }

    pub fn endpoint_token(&self) -> String {
        self.lock().endpoint_token.clone()
    }

    pub fn endpoint_token_matches(&self, candidate: &str) -> bool {
        pairing::safe_eq(candidate, &self.lock().endpoint_token)
    }

    /// Gera um token novo: os endereços já colados nos clientes deixam de servir.
    pub fn regenerate_endpoint_token(&self) -> String {
        let t = pairing::random_b64(32);
        let _ = pairing::write_private(&self.cfg.state_dir, "bridge-endpoint-token", &t);
        self.lock().endpoint_token = t.clone();
        t
    }

    fn paired_valid(g: &Inner) -> bool {
        g.pairing.as_ref().is_some_and(Pairing::valid)
    }

    fn pair_url_locked(&self, g: &Inner) -> String {
        format!("{}/connect?port={}&token={}", self.cfg.app_url, g.ws_port, g.token)
    }

    fn no_tab_message(&self, g: &Inner) -> String {
        let url = self.pair_url_locked(g);
        if Self::paired_valid(g) {
            format!(
                "O 1fluxo não está ligado neste momento. Abre o 1fluxo no browser ({}) com a sessão desbloqueada — a ligação faz-se sozinha. Se não ligar, abre {} para emparelhar de novo.",
                self.cfg.app_url, url
            )
        } else {
            format!("O 1fluxo não está aberto no browser. Abre {url} para ligar.")
        }
    }

    pub fn instructions(&self) -> String {
        let g = self.lock();
        let url = self.pair_url_locked(&g);
        if Self::paired_valid(&g) {
            format!(
                "As tools do 1fluxo ficam disponíveis quando o utilizador tiver o 1fluxo aberto no browser ({}) com a sessão desbloqueada — a ligação faz-se sozinha. Se uma tool falhar com esse aviso, pede-lhe que abra o 1fluxo; se continuar a falhar, que emparelhe de novo em {}.",
                self.cfg.app_url, url
            )
        } else {
            format!(
                "As tools do 1fluxo só ficam disponíveis depois de o utilizador abrir o 1fluxo no browser e autorizar a ligação em {url} — se uma tool falhar com esse aviso, pede-lhe que abra essa página."
            )
        }
    }

    pub fn has_tab(&self) -> bool {
        self.lock().tab.is_some()
    }

    /// Abre o browser do sistema na página de ligação, no máximo uma vez por
    /// minuto. Sempre o /connect: retoma o emparelhamento que o utilizador
    /// actual tiver e, se não houver, mostra o «Permitir».
    fn open_browser_for_tab(&self) {
        if !self.cfg.open_browser {
            return;
        }
        let url = {
            let mut g = self.lock();
            if g.browser_opened_at.is_some_and(|t| t.elapsed() < OPEN_COOLDOWN) {
                return;
            }
            g.browser_opened_at = Some(Instant::now());
            self.pair_url_locked(&g)
        };
        if open::open_url(&url) {
            log(&format!("A abrir o browser: {url}"));
        } else {
            log(&format!("Não consegui abrir o browser; abre à mão: {url}"));
        }
    }

    /// Abre o `/connect` a pedido do utilizador (botão do tray), sem cooldown.
    pub fn open_connect_page(&self) -> bool {
        open::open_url(&self.pair_url())
    }

    async fn wait_for_tab(&self, wait: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.tab_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.has_tab() {
                return true;
            }
            if timeout_at(deadline, notified).await.is_err() {
                return self.has_tab();
            }
        }
    }

    /* ── sessões MCP (quem é cada cliente) ───────────────────────────────── */

    /// Abre uma sessão para um cliente que acabou de fazer `initialize`.
    pub fn open_session(&self, client: Option<ClientInfo>) -> String {
        let id = pairing::random_b64(24);
        let mut g = self.lock();
        if g.sessions.len() >= MAX_SESSIONS {
            let oldest = g.sessions.iter().min_by_key(|(_, s)| s.last_used).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                g.sessions.remove(&k);
            }
        }
        g.sessions.insert(id.clone(), Session { client, last_used: Instant::now() });
        id
    }

    /// O cliente de uma sessão; `None` se a sessão não existe (o bridge
    /// reiniciou, ou o id é inventado) e o cliente tem de voltar a inicializar.
    pub fn session_client(&self, id: &str) -> Option<Option<ClientInfo>> {
        let mut g = self.lock();
        let s = g.sessions.get_mut(id)?;
        s.last_used = Instant::now();
        Some(s.client.clone())
    }

    pub fn close_session(&self, id: &str) -> bool {
        self.lock().sessions.remove(id).is_some()
    }

    /// Ponto de entrada de uma mensagem JSON-RPC vinda de um cliente MCP, com
    /// o cliente que a mandou (se se souber). `None` quer dizer «sem resposta».
    pub async fn handle_rpc(&self, mut msg: Value, client: Option<ClientInfo>) -> Option<Value> {
        let method = proto::method(&msg).map(str::to_owned);
        let id = proto::request_id(&msg);

        // O `initialize` traz o nome; nos outros pedidos vem da sessão.
        let client = client.or_else(|| {
            (method.as_deref() == Some("initialize")).then(|| proto::client_info(&msg)).flatten()
        });
        if let Some(c) = &client {
            let mut g = self.lock();
            if g.client.as_ref() != Some(c) {
                g.client = Some(c.clone());
                self.publish(&g, true);
            }
            drop(g);
            proto::with_client_meta(&mut msg, c);
        }

        let Some(id) = id else {
            // Notificação (ou resposta do cliente a um pedido do servidor, que o
            // bridge não faz): segue para o tab se houver; senão descarta-se.
            if method.is_some() {
                self.send_to_tab(&msg);
            }
            return None;
        };
        let Some(method) = method else { return None };

        if !self.has_tab() {
            if let Some(answer) = proto::local_answer(&msg, &self.instructions()) {
                return Some(answer);
            }
            // Alguém perguntou algo e não há tab: é a altura de pôr o 1fluxo à
            // frente. Só nas tool calls — o `tools/list` do arranque não é a
            // pessoa a pedir nada.
            let is_call = method == "tools/call";
            if is_call {
                self.open_browser_for_tab();
            }
            let wait = if is_call { TOOL_CALL_WAIT } else { NO_TAB_WAIT };
            if !self.wait_for_tab(wait).await {
                let g = self.lock();
                return Some(error_response(&id, ERR_NO_TAB, &self.no_tab_message(&g)));
            }
        }
        Some(self.forward(msg, id).await)
    }

    fn send_to_tab(&self, msg: &Value) {
        if let Some(tab) = &self.lock().tab {
            let _ = tab.tx.send(msg.to_string());
        }
    }

    async fn forward(&self, mut msg: Value, original_id: Value) -> Value {
        let (internal, rx) = {
            let mut g = self.lock();
            let Some(tab) = &g.tab else {
                return error_response(&original_id, ERR_NO_TAB, &self.no_tab_message(&g));
            };
            let tx = tab.tx.clone();
            let internal = g.next_id;
            g.next_id += 1;
            msg["id"] = json!(internal);
            let (reply, rx) = oneshot::channel();
            g.pending.insert(internal, Pending { reply, original_id: original_id.clone() });
            let _ = tx.send(msg.to_string());
            (internal, rx)
        };
        let _guard = PendingGuard { hub: self, id: internal };
        match rx.await {
            Ok(resp) => resp,
            Err(_) => {
                let g = self.lock();
                error_response(&original_id, ERR_NO_TAB, &self.no_tab_message(&g))
            }
        }
    }

    /// Mensagem recebida do tab, já em texto.
    pub fn on_tab_message(&self, raw: &str) {
        let Ok(mut msg) = serde_json::from_str::<Value>(raw) else { return };
        if !msg.is_object() {
            return;
        }
        // Controlo do próprio emparelhamento (sem `jsonrpc`). «forget» é o
        // utilizador a esquecer este bridge em Definições.
        if msg.get("type").and_then(Value::as_str) == Some("forget") {
            pairing::clear(&self.cfg.state_dir);
            let mut g = self.lock();
            g.pairing = None;
            self.publish(&g, true);
            log("Emparelhamento esquecido a pedido do 1fluxo.");
            return;
        }
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return;
        }
        // Só as respostas (id sem method) têm destino; as notificações do tab
        // não têm cliente a quem chegar neste transporte.
        if msg.get("method").is_some() {
            return;
        }
        let Some(internal) = msg.get("id").and_then(Value::as_u64) else { return };
        let Some(p) = self.lock().pending.remove(&internal) else { return };
        msg["id"] = p.original_id;
        let _ = p.reply.send(msg);
    }

    /* ── emparelhamento ─────────────────────────────────────────────────── */

    pub fn begin_pending(self: &Arc<Self>) -> Option<PendingConn> {
        let mut g = self.lock();
        if g.pending_connections >= MAX_PENDING_CONNECTIONS {
            return None;
        }
        g.pending_connections += 1;
        Some(PendingConn { hub: self.clone(), settled: false })
    }

    /// Validade da retoma; devolve o código de fecho em caso de recusa.
    pub fn check_resume(&self, pairing_id: &str, proof: &str, nonce: &str) -> Result<Pairing, (u16, &'static str)> {
        let mut g = self.lock();
        let Some(p) = g.pairing.clone() else {
            log("Retoma recusada: não há emparelhamento guardado.");
            return Err((4011, "unknown pairing"));
        };
        if !p.valid() {
            log("Retoma recusada: o emparelhamento expirou.");
            pairing::clear(&self.cfg.state_dir);
            g.pairing = None;
            return Err((4010, "pairing expired"));
        }
        let expected = pairing::hmac_b64(&p.secret, &format!("tab:{nonce}"));
        // `&` (não `&&`): os dois lados correm sempre.
        if !(pairing::safe_eq(pairing_id, &p.id) & pairing::safe_eq(proof, &expected)) {
            log("Retoma recusada: prova de emparelhamento inválida.");
            return Err((4011, "bad pairing proof"));
        }
        Ok(p)
    }

    /// O token serve um emparelhamento; depois disso só um token novo entra
    /// (um token reutilizável deixava o emparelhamento ser repetido).
    pub fn token_matches(&self, candidate: &str) -> bool {
        let g = self.lock();
        !g.token_used && pairing::safe_eq(candidate, &g.token)
    }

    pub fn tab_present(&self) -> bool {
        self.has_tab()
    }

    /// Liga o tab. `None` se já houver um (fica o primeiro).
    pub fn attach_tab(&self, nonce: &str, mode: Attach, tx: mpsc::UnboundedSender<String>) -> Option<(u64, String)> {
        let mut g = self.lock();
        if g.tab.is_some() {
            return None;
        }
        let client = g
            .client
            .as_ref()
            .map(|(name, version)| match version {
                Some(v) => json!({ "name": name, "version": v }),
                None => json!({ "name": name }),
            })
            .unwrap_or(Value::Null);
        let hello = match mode {
            Attach::Pair { origin } => {
                g.token_used = true;
                let p = Pairing::new(&origin);
                if let Err(e) = pairing::save(&self.cfg.state_dir, &p) {
                    log(&format!("Não consegui guardar o emparelhamento: {e}"));
                }
                log(&format!("Tab do 1fluxo emparelhado (válido até {}).", p.expires_day()));
                // O segredo só passa no fio aqui, uma vez, para o tab o guardar.
                let hello = json!({
                    "type": "paired",
                    "bridge": VERSION,
                    "client": client,
                    "proof": pairing::hmac_b64(&g.token, nonce),
                    "pairing": { "id": p.id, "secret": p.secret, "expiresAt": p.expires_at },
                });
                g.pairing = Some(p);
                hello
            }
            Attach::Resume(p) => {
                log("Tab do 1fluxo ligado (emparelhamento retomado).");
                json!({
                    "type": "paired",
                    "bridge": VERSION,
                    "client": client,
                    "proof": pairing::hmac_b64(&p.secret, &format!("bridge:{nonce}")),
                    "pairing": { "id": p.id, "expiresAt": p.expires_at },
                })
            }
        };
        let tab_id = g.next_tab_id;
        g.next_tab_id += 1;
        g.tab = Some(TabHandle { id: tab_id, tx });
        self.publish(&g, true);
        drop(g);
        self.tab_notify.notify_waiters();
        Some((tab_id, hello.to_string()))
    }

    pub fn detach_tab(&self, tab_id: u64) {
        let mut g = self.lock();
        if g.tab.as_ref().map(|t| t.id) != Some(tab_id) {
            return;
        }
        g.tab = None;
        log("Tab do 1fluxo desligado.");
        // Quem ficou à espera de resposta não pode ficar pendurado.
        let msg = format!(
            "O 1fluxo fechou a ligação no browser antes de responder. {}",
            self.no_tab_message(&g)
        );
        for (_, p) in g.pending.drain() {
            let _ = p.reply.send(error_response(&p.original_id, ERR_NO_TAB, &msg));
        }
        // O token que emparelhou este tab morre com ele.
        g.token = pairing::random_b64(32);
        g.token_used = false;
        if Self::paired_valid(&g) {
            log("Volta a ligar sozinho quando o 1fluxo estiver aberto no browser.");
        } else {
            log("Token de emparelhamento renovado. Para voltar a ligar:");
            log(&format!("  {}", self.pair_url_locked(&g)));
        }
        self.publish(&g, true);
    }
}

fn load_or_create_endpoint_token(dir: &std::path::Path) -> String {
    let path = dir.join("bridge-endpoint-token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return t;
        }
    }
    let t = pairing::random_b64(32);
    if let Err(e) = pairing::write_private(dir, "bridge-endpoint-token", &t) {
        log(&format!("Não consegui guardar o token do endpoint: {e}"));
    }
    t
}
