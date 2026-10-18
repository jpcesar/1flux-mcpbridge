//! Emparelhamento persistente: o segredo guardado em disco, as
//! provas HMAC e as comparações em tempo constante.
//!
//! O formato do ficheiro é o do bridge em TypeScript (`bridge-pairing.json`,
//! campos em camelCase), para um emparelhamento existente continuar válido.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{fs, io, path::Path};
use subtle::ConstantTimeEq;

pub const PAIRING_FILE: &str = "bridge-pairing.json";
pub const ENDPOINT_FILE: &str = "bridge-endpoint.json";
/// Validade de um emparelhamento: ao fim disto o tab volta a pedir «Permitir».
pub const PAIRING_TTL_DAYS: i64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub id: String,
    pub secret: String,
    pub origin: String,
    pub created_at: String,
    pub expires_at: String,
}

impl Pairing {
    pub fn new(origin: &str) -> Self {
        let now = Utc::now();
        Pairing {
            id: random_b64(12),
            secret: random_b64(32),
            origin: origin.to_string(),
            created_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            expires_at: (now + Duration::days(PAIRING_TTL_DAYS))
                .to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }

    pub fn valid(&self) -> bool {
        DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|t| t.with_timezone(&Utc) > Utc::now())
            .unwrap_or(false)
    }

    fn well_formed(&self) -> bool {
        self.secret.len() >= 32 && DateTime::parse_from_rfc3339(&self.expires_at).is_ok()
    }

    /// `AAAA-MM-DD` para as mensagens de log.
    pub fn expires_day(&self) -> &str {
        self.expires_at.get(..10).unwrap_or(&self.expires_at)
    }
}

pub fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn hmac_b64(key: &str, data: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC aceita qualquer tamanho");
    mac.update(data.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Comparação em tempo constante.
pub fn safe_eq(candidate: &str, expected: &str) -> bool {
    let (a, b) = (candidate.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.ct_eq(b).into()
}

/// Lê o emparelhamento guardado; ausente, ilegível ou malformado é «não há».
pub fn load(dir: &Path) -> Option<Pairing> {
    let raw = fs::read_to_string(dir.join(PAIRING_FILE)).ok()?;
    let p: Pairing = serde_json::from_str(&raw).ok()?;
    p.well_formed().then_some(p)
}

pub fn save(dir: &Path, p: &Pairing) -> io::Result<()> {
    write_private(dir, PAIRING_FILE, &(serde_json::to_string_pretty(p)? + "\n"))
}

pub fn clear(dir: &Path) {
    let _ = fs::remove_file(dir.join(PAIRING_FILE));
}

/// Escreve um ficheiro que só o utilizador lê (a pasta a 0700, o ficheiro a
/// 0600; no Windows o modo é ignorado e a pasta do utilizador já é privada).
pub fn write_private(dir: &Path, name: &str, contents: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let path = dir.join(name);
        // Se o ficheiro já existia com outro modo, aperta-o antes de escrever.
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        io::Write::write_all(&mut f, contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
        fs::write(dir.join(name), contents)
    }
}

/// Onde fica o estado por defeito: `~/.1fluxo` (como o bridge em TypeScript).
pub fn default_state_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".1fluxo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_node_digest() {
        // createHmac("sha256","key").update("data").digest("base64url") em Node.
        assert_eq!(hmac_b64("key", "data"), "UDH-PZicbRU3oBP6bnOdojRj_a7DtwE32Cjjas4iG9A");
    }

    #[test]
    fn safe_eq_checks_length_and_content() {
        assert!(safe_eq("abc", "abc"));
        assert!(!safe_eq("abc", "abd"));
        assert!(!safe_eq("abc", "abcd"));
    }

    #[test]
    fn roundtrip_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Pairing::new("http://localhost:3000");
        assert!(p.valid());
        save(dir.path(), &p).unwrap();
        assert_eq!(load(dir.path()), Some(p.clone()));
        p.expires_at = (Utc::now() - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
        assert!(!p.valid());
        clear(dir.path());
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn rejects_short_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Pairing::new("x");
        p.secret = "curto".into();
        save(dir.path(), &p).unwrap();
        assert!(load(dir.path()).is_none());
    }
}
