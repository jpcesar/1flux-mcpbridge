//! 1fluxo Bridge — app de tray (macOS e Windows). Um só executável:
//!
//! * sem argumentos: ícone no tray com Ligar/Desligar, Emparelhar, estado e o
//!   endereço MCP local;
//! * `--stdio`: shim para os clientes que só lançam processos (Claude Desktop),
//!   reencaminha para o bridge que a app já tem a correr;
//! * `--forget`: apaga o emparelhamento e termina.

// No Windows a app de tray não abre consola. Os handles de stdio herdados
// continuam a funcionar para o modo `--stdio`.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use bridge_core::{cli, endpoint_url, hub::log, pairing, shim, Config, Running, Status};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Manager, RunEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tokio::sync::Mutex;

const APP_STATE_FILE: &str = "bridge-app.json";

/* ── ícones ─────────────────────────────────────────────────────────────── */

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Off,
    On,
    Connected,
}

/// macOS: modelos monocromáticos (o sistema pinta-os para o modo claro/escuro).
/// Windows: as cores da marca.
fn icon_for(look: Look) -> Image<'static> {
    let bytes: &[u8] = match (cfg!(target_os = "macos"), look) {
        (true, Look::Off) => include_bytes!("../icons/tray-off.png"),
        (true, Look::On) => include_bytes!("../icons/tray-on.png"),
        (true, Look::Connected) => include_bytes!("../icons/tray-connected.png"),
        (false, Look::Off) => include_bytes!("../icons/color-off.png"),
        (false, Look::On) => include_bytes!("../icons/color-on.png"),
        (false, Look::Connected) => include_bytes!("../icons/color-connected.png"),
    };
    Image::from_bytes(bytes).expect("ícone embebido inválido")
}

/* ── estado da app (sobrevive a reinícios) ──────────────────────────────── */

fn read_app_state(dir: &Path) -> Option<bool> {
    let raw = std::fs::read_to_string(dir.join(APP_STATE_FILE)).ok()?;
    serde_json::from_str::<serde_json::Value>(&raw).ok()?.get("enabled")?.as_bool()
}

fn write_app_state(dir: &Path, enabled: bool) {
    let _ = pairing::write_private(dir, APP_STATE_FILE, &serde_json::json!({ "enabled": enabled }).to_string());
}

/* ── interface ──────────────────────────────────────────────────────────── */

#[derive(Clone)]
struct Ui {
    tray: TrayIcon<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    /// «Desligar» com o bridge ligado, «Ligar» com ele desligado.
    toggle: MenuItem<tauri::Wry>,
    /// «Esquecer emparelhamento» se houver um, «Emparelhar» se não houver.
    pair: MenuItem<tauri::Wry>,
    copy_url: MenuItem<tauri::Wry>,
    copy_cmd: MenuItem<tauri::Wry>,
    state_dir: PathBuf,
}

impl Ui {
    fn show(&self, s: &Status, error: Option<&str>) {
        let (text, look) = match (error, s.running, s.tab_connected) {
            (Some(e), _, _) => (format!("Erro: {e}"), Look::Off),
            (None, false, _) => ("Desligado".to_string(), Look::Off),
            (None, true, false) => ("Ligado — à espera do 1fluxo no browser".to_string(), Look::On),
            (None, true, true) => (
                match &s.client {
                    Some(c) => format!("1fluxo ligado — {c}"),
                    None => "1fluxo ligado".to_string(),
                },
                Look::Connected,
            ),
        };
        let _ = self.status.set_text(&text);
        let _ = self.toggle.set_text(if s.running { "Desligar" } else { "Ligar" });
        // Esquecer funciona sempre (apaga o ficheiro); Emparelhar precisa do
        // bridge ligado, porque é ele que abre a página /connect com o token.
        if paired(s, &self.state_dir) {
            let _ = self.pair.set_text("Esquecer emparelhamento");
            let _ = self.pair.set_enabled(true);
        } else {
            let _ = self.pair.set_text("Emparelhar");
            let _ = self.pair.set_enabled(s.running);
        }
        for item in [&self.copy_url, &self.copy_cmd] {
            let _ = item.set_enabled(s.running);
        }
        let _ = self.tray.set_icon(Some(icon_for(look)));
        #[cfg(target_os = "macos")]
        let _ = self.tray.set_icon_as_template(true);
        let _ = self.tray.set_tooltip(Some(format!("1fluxo Bridge — {text}")));
    }
}

/// Há emparelhamento válido? Com o bridge ligado sabe-o o próprio bridge; com
/// ele desligado, lê-se o ficheiro.
fn paired(s: &Status, state_dir: &Path) -> bool {
    if s.running {
        s.paired
    } else {
        pairing::load(state_dir).is_some_and(|p| p.valid())
    }
}

struct Shared {
    cfg: Config,
    running: Mutex<Option<Running>>,
    ui: Ui,
}

async fn start_bridge(shared: &Arc<Shared>) {
    let mut slot = shared.running.lock().await;
    if slot.is_some() {
        return;
    }
    match bridge_core::start(shared.cfg.clone()).await {
        Ok(running) => {
            write_app_state(&shared.cfg.state_dir, true);
            let mut rx = running.status();
            shared.ui.show(&rx.borrow(), None);
            let ui = shared.ui.clone();
            tauri::async_runtime::spawn(async move {
                while rx.changed().await.is_ok() {
                    let s = rx.borrow().clone();
                    if !s.running {
                        break;
                    }
                    ui.show(&s, None);
                }
            });
            *slot = Some(running);
        }
        Err(e) => {
            log(&format!("Falha ao arrancar: {e:#}"));
            shared.ui.show(&Status::default(), Some(&format!("{e:#}")));
        }
    }
}

async fn stop_bridge(shared: &Arc<Shared>, remember: bool) {
    let running = shared.running.lock().await.take();
    if let Some(r) = running {
        r.stop().await;
    }
    if remember {
        write_app_state(&shared.cfg.state_dir, false);
    }
    shared.ui.show(&Status::default(), None);
}

fn copy_to_clipboard(text: &str) {
    let mut cmd = if cfg!(target_os = "macos") { Command::new("pbcopy") } else { Command::new("clip") };
    if let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

async fn current(shared: &Arc<Shared>) -> Option<(u16, String)> {
    let guard = shared.running.lock().await;
    let r = guard.as_ref()?;
    Some((r.status().borrow().http_port, r.hub.endpoint_token()))
}

fn handle_menu(app: &AppHandle<tauri::Wry>, shared: &Arc<Shared>, id: &str) {
    let shared = shared.clone();
    let id = id.to_string();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match id.as_str() {
            "toggle" => {
                let on = shared.running.lock().await.is_some();
                if on {
                    stop_bridge(&shared, true).await;
                } else {
                    start_bridge(&shared).await;
                }
            }
            "pair" => {
                let status = shared.running.lock().await.as_ref().map(|r| r.status().borrow().clone());
                let status = status.unwrap_or_default();
                if paired(&status, &shared.cfg.state_dir) {
                    pairing::clear(&shared.cfg.state_dir);
                    // Reinicia para o bridge largar o emparelhamento em memória.
                    if status.running {
                        stop_bridge(&shared, false).await;
                        start_bridge(&shared).await;
                    } else {
                        shared.ui.show(&Status::default(), None);
                    }
                } else if let Some(r) = shared.running.lock().await.as_ref() {
                    r.hub.open_connect_page();
                }
            }
            "copy_url" => {
                if let Some((port, token)) = current(&shared).await {
                    // Com o token no caminho para os clientes que não deixam configurar headers.
                    copy_to_clipboard(&format!("{}/{token}", endpoint_url(port)));
                }
            }
            "copy_cmd" => {
                if let Some((port, token)) = current(&shared).await {
                    copy_to_clipboard(&format!(
                        "claude mcp add --transport http 1fluxo {} --header \"Authorization: Bearer {token}\"",
                        endpoint_url(port)
                    ));
                }
            }
            "autostart" => {
                let launcher = app.autolaunch();
                let enabled = launcher.is_enabled().unwrap_or(false);
                let _ = if enabled { launcher.disable() } else { launcher.enable() };
            }
            "quit" => {
                stop_bridge(&shared, false).await;
                app.exit(0);
            }
            _ => {}
        }
    });
}

fn build_ui(app: &AppHandle<tauri::Wry>, state_dir: PathBuf) -> tauri::Result<(Ui, Menu<tauri::Wry>)> {
    let status = MenuItem::with_id(app, "status", "Desligado", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Ligar", true, None::<&str>)?;
    let pair = MenuItem::with_id(app, "pair", "Emparelhar", false, None::<&str>)?;
    let copy_url = MenuItem::with_id(app, "copy_url", "Copiar endereço MCP", false, None::<&str>)?;
    let copy_cmd = MenuItem::with_id(app, "copy_cmd", "Copiar comando para o Claude Code", false, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Arrancar com o sistema",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[&status, &sep()?, &toggle, &pair, &copy_url, &copy_cmd, &sep()?, &autostart, &sep()?, &quit],
    )?;
    let tray = TrayIconBuilder::with_id("main")
        .icon(icon_for(Look::Off))
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("1fluxo Bridge")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;
    let ui = Ui { tray, status, toggle, pair, copy_url, copy_cmd, state_dir };
    // Estado inicial (desligado): o item do emparelhamento já lê o ficheiro.
    ui.show(&Status::default(), None);
    Ok((ui, menu))
}

fn run_tray(config: Config) {
    let state_dir: PathBuf = config.state_dir.clone();
    let first_run = read_app_state(&state_dir).is_none();
    // Por defeito ligado; depois de um OFF deliberado, fica desligado após reiniciar.
    let enabled = read_app_state(&state_dir).unwrap_or(true);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Arranque com o sistema ligado por defeito. Só
            // em builds de release, e `FLUXO_BRIDGE_NO_AUTOSTART=1` desliga-o (testes).
            if first_run && !cfg!(debug_assertions) && std::env::var_os("FLUXO_BRIDGE_NO_AUTOSTART").is_none() {
                let _ = app.autolaunch().enable();
            }
            let (ui, _menu) = build_ui(app.handle(), config.state_dir.clone())?;
            let shared = Arc::new(Shared { cfg: config, running: Mutex::new(None), ui });

            let (handle, s) = (app.handle().clone(), shared.clone());
            app.on_menu_event(move |_app, event| {
                // O item de «Arrancar com o sistema» alterna o visto sozinho; o
                // sistema é actualizado no handler.
                handle_menu(&handle, &s, event.id().as_ref());
            });

            if enabled {
                let s = shared.clone();
                tauri::async_runtime::spawn(async move { start_bridge(&s).await });
            }
            app.manage(shared);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("falha ao criar a app");

    app.run(|_app, event| {
        // Sem janelas, o Tauri não deve sair sozinho: só o item «Sair».
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

fn main() {
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
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        if let Err(e) = rt.block_on(shim::run_stdio(&args.config.state_dir)) {
            log(&format!("Erro no modo stdio: {e}"));
            std::process::exit(1);
        }
        return;
    }
    run_tray(args.config);
}
