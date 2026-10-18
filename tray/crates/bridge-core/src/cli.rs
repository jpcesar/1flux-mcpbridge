//! Argumentos de linha de comandos, partilhados pelo binário headless e pela app de tray.

use crate::hub::{Config, DEFAULT_APP_URL, DEFAULT_HTTP_PORT, DEFAULT_WS_PORT};
use crate::pairing::default_state_dir;

#[derive(Debug)]
pub struct Args {
    pub config: Config,
    /// Reencaminhar stdin/stdout para o tray em vez de abrir o bridge.
    pub stdio: bool,
    /// Apagar o emparelhamento guardado e terminar.
    pub forget: bool,
}

pub fn usage() -> String {
    format!(
        "Uso: 1fluxo-bridge [--stdio] [--port N] [--http-port N] [--app-url URL] [--origin URL]... [--state-dir DIR] [--forget] [--no-open]

  --stdio          reencaminha stdin/stdout para o bridge que já está a correr (para o Claude Desktop)
  --port N         porta do WebSocket local (defeito {DEFAULT_WS_PORT}; se ocupada escolhe a seguinte)
  --http-port N    porta do endpoint MCP (defeito {DEFAULT_HTTP_PORT}); tem de ser estável
  --app-url URL    onde o 1fluxo está a correr (defeito {DEFAULT_APP_URL})
  --origin URL     origem adicional autorizada a emparelhar (repetível)
  --state-dir DIR  onde fica o estado (defeito {})
  --forget         apaga o emparelhamento guardado e termina
  --no-open        não abrir o browser sozinho quando um pedido chega sem tab ligado",
        default_state_dir().display()
    )
}

pub fn parse(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut it = argv.into_iter();
    let mut args = Args { config: Config::default(), stdio: false, forget: false };
    while let Some(a) = it.next() {
        let mut next = |name: &str| it.next().ok_or_else(|| format!("{name} precisa de um valor"));
        match a.as_str() {
            "--help" | "-h" => return Err(usage()),
            "--stdio" => args.stdio = true,
            "--forget" => args.forget = true,
            "--no-open" => args.config.open_browser = false,
            "--port" => args.config.ws_port = next("--port")?.parse().map_err(|_| "--port inválida".to_string())?,
            "--http-port" => args.config.http_port = next("--http-port")?.parse().map_err(|_| "--http-port inválida".to_string())?,
            "--app-url" => args.config.app_url = next("--app-url")?.trim_end_matches('/').to_string(),
            "--origin" => args.config.origins.push(next("--origin")?),
            "--state-dir" => {
                let dir = next("--state-dir")?;
                let dir = dir.trim();
                // Vazio (ex.: `${HOME}` por resolver num manifesto) → defeito.
                if !dir.is_empty() && !dir.contains("${") {
                    args.config.state_dir = dir.into();
                }
            }
            other => return Err(format!("argumento desconhecido: {other}\n\n{}", usage())),
        }
    }
    Ok(args)
}
