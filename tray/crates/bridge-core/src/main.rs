//! Bridge sem interface (para testes e para correr em terminal). A app de tray
//! usa a mesma biblioteca.

use bridge_core::{cli, hub::log, pairing, shim};

#[tokio::main]
async fn main() {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(if msg.starts_with("Uso:") { 0 } else { 2 });
        }
    };
    if args.forget {
        pairing::clear(&args.config.state_dir);
        log("Emparelhamento apagado.");
        return;
    }
    if args.stdio {
        if let Err(e) = shim::run_stdio(&args.config.state_dir).await {
            log(&format!("Erro no modo stdio: {e}"));
            std::process::exit(1);
        }
        return;
    }
    let running = match bridge_core::start(args.config).await {
        Ok(r) => r,
        Err(e) => {
            log(&format!("Falha ao arrancar: {e:#}"));
            std::process::exit(1);
        }
    };
    log(&format!("Abre no browser, com a sessão do 1fluxo desbloqueada: {}", running.hub.pair_url()));
    let _ = tokio::signal::ctrl_c().await;
    running.stop().await;
}
