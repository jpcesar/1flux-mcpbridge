//! Bridge local: liga clientes MCP (HTTP ou stdio) ao 1fluxo aberto no browser.
//!
//! Zero-knowledge é face ao 1fluxo: a decifragem acontece no tab. Este processo
//! vê o plaintext a passar, mas corre na máquina do utilizador, ao lado do
//! cliente MCP que o vai receber de qualquer maneira; só fala por loopback e
//! não escreve conteúdo em logs.

pub mod cli;
pub mod http;
pub mod hub;
pub mod open;
pub mod pairing;
pub mod proto;
pub mod shim;
pub mod ws;

use anyhow::Context;
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub use hub::{Config, Hub, Status};

/// Um bridge em marcha. `stop()` fecha tudo (é o botão OFF).
pub struct Running {
    pub hub: Arc<Hub>,
    status: watch::Receiver<Status>,
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

impl Running {
    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        for t in self.tasks {
            let _ = t.await;
        }
        self.hub.mark_stopped();
    }
}

/// O endereço do endpoint MCP, sem token.
pub fn endpoint_url(http_port: u16) -> String {
    format!("http://127.0.0.1:{http_port}/mcp")
}

/// Arranca o bridge: WebSocket para o tab, endpoint HTTP para os clientes.
pub async fn start(cfg: Config) -> anyhow::Result<Running> {
    let (hub, status) = Hub::new(cfg);
    let cancel = CancellationToken::new();

    let ws_listener = ws::bind(hub.cfg.ws_port).await.context("não consegui abrir o WebSocket")?;
    let ws_port = ws_listener.local_addr()?.port();
    let http_listener = tokio::net::TcpListener::bind(("127.0.0.1", hub.cfg.http_port))
        .await
        .with_context(|| format!("a porta {} do endpoint MCP está ocupada", hub.cfg.http_port))?;
    let http_port = http_listener.local_addr()?.port();
    hub.set_ports(ws_port, http_port);

    // O shim `--stdio` descobre o endpoint por aqui.
    shim::write_endpoint_info(&hub.cfg.state_dir, &shim::EndpointInfo { port: http_port, token: hub.endpoint_token() })
        .context("não consegui escrever o ficheiro do endpoint")?;

    hub::log(&format!("v{} — WebSocket em ws://127.0.0.1:{ws_port}, endpoint MCP em {}", proto::VERSION, endpoint_url(http_port)));

    let tasks = vec![
        tokio::spawn(ws::serve(hub.clone(), ws_listener, cancel.clone())),
        tokio::spawn(http::serve(hub.clone(), http_listener, cancel.clone())),
    ];
    Ok(Running { hub, status, cancel, tasks })
}
