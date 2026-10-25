//! macOS: a app tem de viver em /Applications ou ~/Applications antes de se
//! registar como item de início. Se correr do DMG ou de Descarregamentos (onde o
//! macOS a copia para uma pasta temporária, «AppTranslocation»), oferece-se
//! copiá-la e relançá-la de lá.
//!
//! A lógica de caminhos é pura (e testada em todos os sistemas); só `offer_move`
//! mexe no sistema.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};

const DECLINED_FILE: &str = "move-declined.json";

/// O `.app` que contém `exe` (`…/X.app/Contents/MacOS/x`). `None` fora de um
/// bundle (`cargo run`, `target/release`).
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let ok = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|e| e == "app");
    ok.then(|| bundle.to_path_buf())
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Está em /Applications ou ~/Applications (ou numa subpasta)?
pub fn is_installed(bundle: &Path, home: Option<&Path>) -> bool {
    let b = canon(bundle);
    let mut roots = vec![PathBuf::from("/Applications")];
    roots.extend(home.map(|h| h.join("Applications")));
    roots.iter().any(|r| b.starts_with(canon(r)) || bundle.starts_with(r))
}

/// O Gatekeeper corre as apps descarregadas de uma cópia só de leitura em
/// `/private/var/folders/…/AppTranslocation/…`.
pub fn is_translocated(bundle: &Path) -> bool {
    bundle.to_string_lossy().contains("/AppTranslocation/")
}

/// Onde pôr a app: /Applications se for gravável, senão ~/Applications.
pub fn destinations(name: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/Applications").join(name)];
    v.extend(home.map(|h| h.join("Applications").join(name)));
    v
}

/// O executável de uma instalação, para terminar a versão antiga antes de a trocar.
fn exe_in(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS/1fluxo-bridge")
}

/* ── «Agora não», lembrado por caminho ──────────────────────────────────── */

pub fn declined_for(state_dir: &Path, bundle: &Path) -> bool {
    std::fs::read_to_string(state_dir.join(DECLINED_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(|p| p == bundle.to_string_lossy()))
        .unwrap_or(false)
}

pub fn remember_decline(state_dir: &Path, bundle: &Path) {
    let body = serde_json::json!({ "path": bundle.to_string_lossy() }).to_string();
    let _ = bridge_core::pairing::write_private(state_dir, DECLINED_FILE, &body);
}

/* ── o sistema ──────────────────────────────────────────────────────────── */

#[cfg(target_os = "macos")]
mod system {
    use super::*;
    use bridge_core::hub::log;
    use std::process::{Command, Stdio};

    /// «Mover» ou «Agora não»; só uma falha do próprio diálogo conta como «Agora não».
    pub fn ask(from: &Path, translocated: bool) -> bool {
        let origem = if translocated {
            "Está a correr a partir de uma pasta temporária do macOS (a app foi aberta de Descarregamentos)."
        } else {
            "Está a correr fora da pasta Aplicações (por exemplo, de dentro do DMG)."
        };
        let text = format!(
            "{origem}\n\nPara arrancar com o sistema e se manter atualizada, a app deve ficar em Aplicações. Queres mover o 1fluxo Bridge para lá agora?\n\n{}",
            from.display()
        );
        let out = Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "button returned of (display dialog (item 1 of argv) with title \"1fluxo Bridge\" buttons {\"Agora não\", \"Mover para Aplicações\"} default button \"Mover para Aplicações\")",
                "-e",
                "end run",
                &text,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "Mover para Aplicações")
    }

    /// Copia `bundle` para o primeiro destino onde dê, substitui a versão que lá
    /// estiver e devolve o novo caminho.
    pub fn copy_to_applications(bundle: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
        let name = bundle.file_name().and_then(|n| n.to_str()).ok_or("nome da app inválido")?;
        let mut last_err = String::from("sem destino");
        for dest in destinations(name, home) {
            let Some(dir) = dest.parent() else { continue };
            if std::fs::create_dir_all(dir).is_err() {
                continue;
            }
            // Copia para um nome temporário ao lado e só depois troca: uma falha a
            // meio não deixa a instalação anterior estragada.
            let tmp = dir.join(format!(".{name}.novo"));
            let _ = std::fs::remove_dir_all(&tmp);
            let copied = Command::new("ditto").arg(bundle).arg(&tmp).status();
            if !matches!(copied, Ok(s) if s.success()) {
                let _ = std::fs::remove_dir_all(&tmp);
                last_err = format!("não consegui copiar para {}", dir.display());
                continue;
            }
            if dest.exists() {
                // Termina a instância que lá corre (só o tray, não os `--stdio`).
                let pattern = format!("^{}$", exe_in(&dest).display());
                let _ = Command::new("pkill").args(["-f", &pattern]).status();
                if let Err(e) = std::fs::remove_dir_all(&dest) {
                    let _ = std::fs::remove_dir_all(&tmp);
                    last_err = format!("não consegui substituir {}: {e}", dest.display());
                    continue;
                }
            }
            return std::fs::rename(&tmp, &dest).map(|_| dest).map_err(|e| e.to_string());
        }
        Err(last_err)
    }

    pub fn relaunch(app: &Path) -> bool {
        matches!(Command::new("open").arg("-n").arg(app).status(), Ok(s) if s.success())
    }

    pub fn log_err(msg: &str) {
        log(msg);
    }
}

/// Resultado da verificação no arranque.
#[derive(Debug, PartialEq, Eq)]
pub enum Placement {
    /// Em Aplicações, ou fora de um bundle (desenvolvimento): seguir em frente.
    Settled,
    /// Fora de Aplicações e o utilizador disse «Agora não»: correr, mas sem se
    /// registar como item de início (apontaria para o sítio errado).
    Declined,
    /// Copiada e relançada de Aplicações: esta instância deve terminar.
    Relaunched,
}

/// Ver o módulo. `home` é a pasta do utilizador.
#[cfg(target_os = "macos")]
pub fn offer_move(state_dir: &Path, home: Option<&Path>) -> Placement {
    let Some(bundle) = std::env::current_exe().ok().and_then(|e| bundle_of(&e)) else {
        return Placement::Settled;
    };
    if is_installed(&bundle, home) {
        return Placement::Settled;
    }
    if declined_for(state_dir, &bundle) {
        return Placement::Declined;
    }
    if !system::ask(&bundle, is_translocated(&bundle)) {
        remember_decline(state_dir, &bundle);
        return Placement::Declined;
    }
    match system::copy_to_applications(&bundle, home) {
        Ok(dest) if system::relaunch(&dest) => Placement::Relaunched,
        Ok(dest) => {
            system::log_err(&format!("Copiada para {}, mas não consegui abri-la.", dest.display()));
            Placement::Declined
        }
        Err(e) => {
            system::log_err(&format!("Não consegui mover para Aplicações: {e}"));
            Placement::Declined
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn bundle_is_found_from_the_executable() {
        assert_eq!(
            bundle_of(&p("/Applications/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge")),
            Some(p("/Applications/1fluxo Bridge.app"))
        );
        assert_eq!(bundle_of(&p("/x/tray/target/release/1fluxo-bridge")), None);
        // Uma pasta «.app» sem a estrutura Contents/MacOS não é um bundle.
        assert_eq!(bundle_of(&p("/x/Foo.app/bin/1fluxo-bridge")), None);
    }

    #[test]
    fn applications_folders_count_as_installed() {
        let home = p("/Users/ana");
        let ok = |s: &str| is_installed(&p(s), Some(&home));
        assert!(ok("/Applications/1fluxo Bridge.app"));
        assert!(ok("/Applications/Utilitários/1fluxo Bridge.app"));
        assert!(ok("/Users/ana/Applications/1fluxo Bridge.app"));
        assert!(!ok("/Volumes/1fluxo Bridge/1fluxo Bridge.app"));
        assert!(!ok("/Users/ana/Downloads/1fluxo Bridge.app"));
        assert!(!ok("/private/var/folders/0y/T/AppTranslocation/ABC/d/1fluxo Bridge.app"));
        // Prefixo parecido não chega: «/ApplicationsX» não é «/Applications».
        assert!(!ok("/ApplicationsX/1fluxo Bridge.app"));
    }

    #[test]
    fn translocation_is_detected() {
        assert!(is_translocated(&p("/private/var/folders/0y/T/AppTranslocation/ABC/d/1fluxo Bridge.app")));
        assert!(!is_translocated(&p("/Volumes/1fluxo Bridge/1fluxo Bridge.app")));
    }

    #[test]
    fn destinations_prefer_the_system_folder() {
        let d = destinations("X.app", Some(&p("/Users/ana")));
        assert_eq!(d, vec![p("/Applications/X.app"), p("/Users/ana/Applications/X.app")]);
        assert_eq!(destinations("X.app", None), vec![p("/Applications/X.app")]);
    }

    #[test]
    fn declining_is_remembered_per_path() {
        let dir = std::env::temp_dir().join(format!("fluxo-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dmg = p("/Volumes/1fluxo Bridge/1fluxo Bridge.app");
        assert!(!declined_for(&dir, &dmg));
        remember_decline(&dir, &dmg);
        assert!(declined_for(&dir, &dmg));
        // Outro sítio (p. ex. uma nova transferência): volta a perguntar.
        assert!(!declined_for(&dir, &p("/Users/ana/Downloads/1fluxo Bridge.app")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
