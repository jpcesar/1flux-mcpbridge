//! Liga o Claude Desktop (separador Chat) a esta app: acrescenta uma entrada
//! `1fluxo` ao `claude_desktop_config.json` que lança o próprio executável com
//! `--stdio` (o shim de `shim.rs`). A entrada não leva token: o shim descobre a
//! porta e o token em `<state-dir>/bridge-endpoint.json`.
//!
//! O ficheiro é de outra app e do utilizador, por isso: só se mexe na chave
//! `mcpServers.1fluxo`, o resto fica como estava (ordem das chaves incluída);
//! JSON ilegível nunca é reescrito; guarda-se uma cópia antes da primeira
//! alteração; e a escrita é atómica (ficheiro temporário + rename).

use serde_json::{json, Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const SERVER_KEY: &str = "1fluxo";
const CONFIG_FILE: &str = "claude_desktop_config.json";
const BACKUP_FILE: &str = "claude_desktop_config.json.1fluxo.bak";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Não havia entrada `1fluxo`; foi acrescentada.
    Added,
    /// Havia uma entrada diferente (outro caminho); foi actualizada.
    Updated,
    /// Já estava como devia.
    Unchanged,
}

#[derive(Debug)]
pub enum Error {
    /// O executável corre de um sítio temporário: o caminho deixaria de existir.
    UnstableExe(PathBuf),
    /// O ficheiro existe mas não é um objecto JSON válido; não se mexe.
    BadConfig(String),
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnstableExe(p) => write!(
                f,
                "A app está a correr de um sítio temporário ({}). Move-a para a pasta Aplicações, abre-a de lá e tenta outra vez.",
                p.display()
            ),
            Error::BadConfig(why) => write!(
                f,
                "Não consegui ler o ficheiro de configuração do Claude Desktop ({why}). Não lhe toquei."
            ),
            Error::Io(e) => write!(f, "Erro ao escrever a configuração do Claude Desktop: {e}"),
        }
    }
}

/// Onde o Claude Desktop guarda a configuração, se este sistema o suportar.
pub fn config_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/Claude").join(CONFIG_FILE))
    } else if cfg!(windows) {
        let roaming = PathBuf::from(std::env::var_os("APPDATA")?).join("Claude");
        // A versão da Microsoft Store guarda-a numa pasta virtualizada.
        if !roaming.exists() {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                if let Ok(entries) = fs::read_dir(PathBuf::from(local).join("Packages")) {
                    for e in entries.flatten() {
                        let name = e.file_name();
                        if name.to_string_lossy().starts_with("Claude_") {
                            let p = e.path().join("LocalCache/Roaming/Claude");
                            if p.exists() {
                                return Some(p.join(CONFIG_FILE));
                            }
                        }
                    }
                }
            }
        }
        Some(roaming.join(CONFIG_FILE))
    } else {
        None
    }
}

/// O caminho do executável tem de sobreviver: no macOS, uma app aberta a partir
/// da pasta Transferências corre «translocada» (caminho aleatório e temporário).
pub fn exe_is_stable(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    !(s.contains("/AppTranslocation/") || s.contains("/private/var/folders/"))
}

fn entry_for(exe: &Path) -> Value {
    json!({ "command": exe.to_string_lossy(), "args": ["--stdio"] })
}

/// Acrescenta ou actualiza `mcpServers.1fluxo` em `path`, a apontar para `exe`.
pub fn install(path: &Path, exe: &Path) -> Result<Outcome, Error> {
    if !exe_is_stable(exe) {
        return Err(Error::UnstableExe(exe.to_path_buf()));
    }

    let existing = match fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => None,
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut root: Value = match &existing {
        Some(raw) => serde_json::from_str(raw).map_err(|e| Error::BadConfig(e.to_string()))?,
        None => Value::Object(Map::new()),
    };
    let obj = root.as_object_mut().ok_or_else(|| Error::BadConfig("não é um objecto JSON".into()))?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::BadConfig("«mcpServers» não é um objecto".into()))?;

    let wanted = entry_for(exe);
    let outcome = match servers.get(SERVER_KEY) {
        Some(cur) if *cur == wanted => return Ok(Outcome::Unchanged),
        Some(_) => Outcome::Updated,
        None => Outcome::Added,
    };
    servers.insert(SERVER_KEY.to_string(), wanted);

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Cópia do original, uma vez: se já existe uma, é a mais antiga e fica.
    if let (Some(raw), Some(dir)) = (&existing, path.parent()) {
        let bak = dir.join(BACKUP_FILE);
        if !bak.exists() {
            fs::write(bak, raw)?;
        }
    }
    let tmp = path.with_extension("json.1fluxo.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&root).map_err(|e| Error::BadConfig(e.to_string()))? + "\n")?;
    fs::rename(&tmp, path)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from("/Applications/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge")
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Claude").join(CONFIG_FILE);
        assert_eq!(install(&path, &exe()).unwrap(), Outcome::Added);
        let v = read(&path);
        assert_eq!(v["mcpServers"]["1fluxo"]["command"], exe().to_string_lossy().as_ref());
        assert_eq!(v["mcpServers"]["1fluxo"]["args"], json!(["--stdio"]));
        assert!(!path.parent().unwrap().join(BACKUP_FILE).exists(), "sem original, sem cópia");
    }

    #[test]
    fn keeps_everything_else_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let original = r#"{
  "zeta": 1,
  "mcpServers": { "outro": { "command": "x" } },
  "alpha": { "a": true }
}"#;
        fs::write(&path, original).unwrap();
        assert_eq!(install(&path, &exe()).unwrap(), Outcome::Added);
        let v = read(&path);
        assert_eq!(v["mcpServers"]["outro"]["command"], "x");
        assert_eq!(v["zeta"], 1);
        assert_eq!(v["alpha"]["a"], true);
        // A ordem das chaves do utilizador mantém-se.
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["zeta", "mcpServers", "alpha"]);
        assert_eq!(fs::read_to_string(dir.path().join(BACKUP_FILE)).unwrap(), original);
    }

    #[test]
    fn second_run_is_unchanged_and_path_change_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        install(&path, &exe()).unwrap();
        assert_eq!(install(&path, &exe()).unwrap(), Outcome::Unchanged);
        let other = PathBuf::from("/Applications/Outra/1fluxo-bridge");
        assert_eq!(install(&path, &other).unwrap(), Outcome::Updated);
        assert_eq!(read(&path)["mcpServers"]["1fluxo"]["command"], "/Applications/Outra/1fluxo-bridge");
    }

    #[test]
    fn refuses_invalid_json_and_leaves_it_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, "{ isto não é json").unwrap();
        assert!(matches!(install(&path, &exe()), Err(Error::BadConfig(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ isto não é json");
        fs::write(&path, r#"{"mcpServers": []}"#).unwrap();
        assert!(matches!(install(&path, &exe()), Err(Error::BadConfig(_))));
    }

    #[test]
    fn refuses_translocated_executable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let temp = PathBuf::from("/private/var/folders/0y/x/T/AppTranslocation/ID/d/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge");
        assert!(matches!(install(&path, &temp), Err(Error::UnstableExe(_))));
        assert!(!path.exists());
    }
}
