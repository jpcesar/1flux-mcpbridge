//! Liga o Claude Code (CLI e separador Code do Claude Desktop, que partilham o
//! `~/.claude.json`) a esta app: regista o servidor `1fluxo` com
//! `claude mcp add --scope user … -- <executável> --stdio`, ou seja, o mesmo
//! shim do Claude Desktop, sem token na configuração.
//!
//! Corre-se o próprio `claude` em vez de editar `~/.claude.json`: esse ficheiro
//! guarda o estado de todos os projectos e o Claude Code reescreve-o a toda a hora.
//! Se o `claude` não for encontrado, ou falhar, quem chama copia o comando
//! (`manual_command`) para o utilizador colar no terminal.

use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SERVER_NAME: &str = "1fluxo";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// Argumentos de `claude` que registam o servidor para o utilizador todo.
pub fn add_args(exe: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = ["mcp", "add", "--scope", "user", SERVER_NAME, "--"].iter().map(OsString::from).collect();
    v.push(exe.as_os_str().to_owned());
    v.push("--stdio".into());
    v
}

fn remove_args() -> Vec<OsString> {
    ["mcp", "remove", "--scope", "user", SERVER_NAME].iter().map(OsString::from).collect()
}

/// O comando para colar à mão no terminal.
pub fn manual_command(exe: &Path) -> String {
    format!("claude mcp add --scope user {SERVER_NAME} -- {} --stdio", shell_quote(&exe.to_string_lossy()))
}

fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// Sítios habituais do `claude`. Uma app lançada do Finder não herda o `PATH` do terminal.
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home() {
        if cfg!(windows) {
            v.push(h.join(".local/bin/claude.exe"));
        } else {
            v.push(h.join(".local/bin/claude"));
            v.push(h.join(".claude/local/claude"));
        }
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(a).join("npm/claude.cmd"));
        }
    } else {
        v.push("/opt/homebrew/bin/claude".into());
        v.push("/usr/local/bin/claude".into());
    }
    v
}

/// Corre um comando com limite de tempo; devolve (sucesso, stdout, stderr).
fn run_with_timeout(mut cmd: Command, limit: Duration) -> std::io::Result<(bool, String, String)> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let ok = loop {
        if let Some(status) = child.try_wait()? {
            break status.success();
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok((ok, t_out.join().unwrap_or_default(), t_err.join().unwrap_or_default()))
}

/// Procura o `claude`: primeiro os sítios habituais, depois o `PATH` de um shell de login.
pub fn find_claude() -> Option<PathBuf> {
    if let Some(p) = candidates().into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    if cfg!(windows) {
        let mut c = Command::new("where");
        c.arg("claude");
        let (ok, out, _) = run_with_timeout(c, LOOKUP_TIMEOUT).ok()?;
        return ok.then(|| out.lines().next().map(PathBuf::from)).flatten().filter(|p| p.is_file());
    }
    let shell = std::env::var("SHELL").ok().filter(|s| s.ends_with("/zsh") || s.ends_with("/bash")).unwrap_or_else(|| "/bin/zsh".into());
    let mut c = Command::new(shell);
    c.args(["-lc", "command -v claude"]);
    let (ok, out, _) = run_with_timeout(c, LOOKUP_TIMEOUT).ok()?;
    ok.then(|| out.lines().last().map(|l| PathBuf::from(l.trim()))).flatten().filter(|p| p.is_absolute() && p.is_file())
}

/// `PATH` para o `claude`: o actual mais os sítios onde costumam estar o próprio
/// `claude` e o `node` (os `claude` instalados por npm precisam dele).
fn path_for(claude: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(d) = claude.parent() {
        dirs.push(d.to_path_buf());
    }
    if !cfg!(windows) {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn run_claude(claude: &Path, args: &[OsString]) -> std::io::Result<(bool, String)> {
    let mut c = Command::new(claude);
    c.args(args).env("PATH", path_for(claude));
    let (ok, out, err) = run_with_timeout(c, RUN_TIMEOUT)?;
    Ok((ok, format!("{}{}", out.trim(), err.trim())))
}

/// Regista (ou volta a registar) o servidor `1fluxo` no âmbito do utilizador.
/// Só mexe na entrada com este nome nesse âmbito; as dos projectos ficam como estão.
pub fn install(claude: &Path, exe: &Path) -> Result<(), String> {
    // Se já existia, o `add` recusava-se; o `remove` falhar (não havia) é normal.
    let _ = run_claude(claude, &remove_args());
    match run_claude(claude, &add_args(exe)) {
        Ok((true, _)) => Ok(()),
        Ok((false, msg)) => Err(if msg.is_empty() { "o claude recusou o registo".into() } else { msg }),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    /// Um «claude» de mentira que regista os argumentos de cada chamada.
    fn fake_claude(dir: &Path, fail_add: bool) -> PathBuf {
        let path = dir.join("claude");
        let log = dir.join("calls.log");
        let body = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\n{}",
            log.display(),
            if fail_add { "case \"$2\" in add) echo 'já existe' >&2; exit 1;; esac\n" } else { "" }
        );
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn manual_command_quotes_the_path() {
        let exe = PathBuf::from("/Applications/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge");
        assert_eq!(
            manual_command(&exe),
            "claude mcp add --scope user 1fluxo -- '/Applications/1fluxo Bridge.app/Contents/MacOS/1fluxo-bridge' --stdio"
        );
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn install_removes_then_adds_with_user_scope() {
        let dir = tempfile::tempdir().unwrap();
        let claude = fake_claude(dir.path(), false);
        install(&claude, Path::new("/Applications/1fluxo Bridge.app/x")).unwrap();
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        let lines: Vec<_> = calls.lines().collect();
        assert_eq!(lines[0], "mcp remove --scope user 1fluxo");
        assert_eq!(lines[1], "mcp add --scope user 1fluxo -- /Applications/1fluxo Bridge.app/x --stdio");
    }

    #[test]
    fn install_reports_the_claude_error() {
        let dir = tempfile::tempdir().unwrap();
        let claude = fake_claude(dir.path(), true);
        assert_eq!(install(&claude, Path::new("/x")).unwrap_err(), "já existe");
    }
}
