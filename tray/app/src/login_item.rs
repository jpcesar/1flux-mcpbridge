//! «Arrancar com o sistema».
//!
//! * macOS 13+: `SMAppService.mainAppService` (o item aparece em Definições do
//!   Sistema → Geral → Itens de início, com o nome e o ícone da app). O estado vem
//!   do sistema, que o utilizador também pode mudar lá.
//! * Windows: chave `Run` do registo, pelo `tauri-plugin-autostart`.
//!
//! O LaunchAgent que as versões anteriores criavam em ~/Library/LaunchAgents é
//! removido na migração, para a app não ficar com dois itens de início.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};

/// O que o sistema diz do item de início.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Registado e aprovado: arranca no próximo início de sessão.
    Enabled,
    /// Registado, mas o utilizador tem de o aprovar nas Definições do Sistema
    /// (também acontece quando o desliga lá).
    RequiresApproval,
    /// Não registado.
    Off,
    /// O sistema não encontra a app (p. ex. fora de um bundle).
    NotFound,
}

impl State {
    /// O visto do menu reflete o que vai mesmo acontecer: só `Enabled` arranca.
    pub fn checked(self) -> bool {
        self == State::Enabled
    }

    pub fn needs_approval(self) -> bool {
        self == State::RequiresApproval
    }
}

/// O que um clique no item do menu faz, dado o estado.
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    Register,
    Unregister,
    /// Já está registado e só falta aprovar: registar outra vez não adianta.
    OpenSettings,
}

pub fn click_action(state: State) -> Click {
    match state {
        State::Enabled => Click::Unregister,
        State::RequiresApproval => Click::OpenSettings,
        State::Off | State::NotFound => Click::Register,
    }
}

/* ── migração do LaunchAgent do plugin antigo ───────────────────────────── */

/// O plugin chamava ao ficheiro «<nome do produto>.plist».
const LEGACY_PLIST: &str = "1fluxo Bridge.plist";

pub fn legacy_plist(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents").join(LEGACY_PLIST)
}

/// Apaga o plist antigo se for nosso (aponta para o executável `1fluxo-bridge`).
/// Devolve `true` se havia um, isto é, se o utilizador tinha o arranque ligado.
///
/// Não faz `launchctl bootout`: se esta instância foi lançada por esse agente,
/// descarregá-lo mataria a própria app. Sem o ficheiro ele deixa de correr no
/// próximo início de sessão.
pub fn remove_legacy_agent(home: &Path) -> bool {
    let path = legacy_plist(home);
    match std::fs::read_to_string(&path) {
        Ok(raw) if raw.contains("1fluxo-bridge") => std::fs::remove_file(&path).is_ok(),
        _ => false,
    }
}

/* ── macOS ──────────────────────────────────────────────────────────────── */

#[cfg(target_os = "macos")]
mod backend {
    use super::State;
    use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};

    fn service() -> AppService {
        AppService::new(ServiceType::MainApp)
    }

    pub fn state() -> State {
        match service().status() {
            ServiceStatus::Enabled => State::Enabled,
            ServiceStatus::RequiresApproval => State::RequiresApproval,
            ServiceStatus::NotRegistered => State::Off,
            ServiceStatus::NotFound => State::NotFound,
        }
    }

    pub fn set_enabled(on: bool) -> Result<(), String> {
        let svc = service();
        let r = if on { svc.register() } else { svc.unregister() };
        match r {
            Ok(()) | Err(ServiceManagementError::AlreadyRegistered) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn open_settings() {
        AppService::open_system_settings_login_items();
    }
}

/* ── Windows (e o resto) ────────────────────────────────────────────────── */

#[cfg(not(target_os = "macos"))]
mod backend {
    use super::State;
    use tauri_plugin_autostart::ManagerExt;

    pub fn state(app: &tauri::AppHandle<tauri::Wry>) -> State {
        if app.autolaunch().is_enabled().unwrap_or(false) {
            State::Enabled
        } else {
            State::Off
        }
    }

    pub fn set_enabled(app: &tauri::AppHandle<tauri::Wry>, on: bool) -> Result<(), String> {
        let launcher = app.autolaunch();
        let r = if on { launcher.enable() } else { launcher.disable() };
        r.map_err(|e| e.to_string())
    }
}

pub fn state(app: &tauri::AppHandle<tauri::Wry>) -> State {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        backend::state()
    }
    #[cfg(not(target_os = "macos"))]
    backend::state(app)
}

pub fn set_enabled(app: &tauri::AppHandle<tauri::Wry>, on: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        backend::set_enabled(on)
    }
    #[cfg(not(target_os = "macos"))]
    backend::set_enabled(app, on)
}

/// «Aprovar nas Definições»: abre Itens de início. Só existe no macOS.
pub fn open_settings() {
    #[cfg(target_os = "macos")]
    backend::open_settings();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enabled_is_checked() {
        assert!(State::Enabled.checked());
        for s in [State::RequiresApproval, State::Off, State::NotFound] {
            assert!(!s.checked(), "{s:?}");
        }
        assert!(State::RequiresApproval.needs_approval());
        assert!(!State::Enabled.needs_approval());
    }

    #[test]
    fn clicking_does_the_sensible_thing_for_each_state() {
        assert_eq!(click_action(State::Enabled), Click::Unregister);
        assert_eq!(click_action(State::Off), Click::Register);
        assert_eq!(click_action(State::NotFound), Click::Register);
        assert_eq!(click_action(State::RequiresApproval), Click::OpenSettings);
    }

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("fluxo-home-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
        home
    }

    #[test]
    fn removes_our_old_launch_agent() {
        let home = temp_home("ours");
        // O mesmo formato que o auto-launch escrevia.
        std::fs::write(
            legacy_plist(&home),
            "<plist version=\"1.0\"><dict><key>Label</key><string>1fluxo Bridge</string>\
             <key>ProgramArguments</key><array><string>/Applications/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge</string></array>\
             <key>RunAtLoad</key><true/></dict></plist>",
        )
        .unwrap();
        assert!(remove_legacy_agent(&home));
        assert!(!legacy_plist(&home).exists());
        // Sem plist: nada a migrar (o utilizador tinha o arranque desligado).
        assert!(!remove_legacy_agent(&home));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn leaves_a_foreign_file_with_the_same_name_alone() {
        let home = temp_home("foreign");
        std::fs::write(legacy_plist(&home), "<plist><dict><key>Label</key><string>outra coisa</string></dict></plist>").unwrap();
        assert!(!remove_legacy_agent(&home));
        assert!(legacy_plist(&home).exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
