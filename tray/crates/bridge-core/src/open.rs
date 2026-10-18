//! Abrir o browser do sistema. Sem shell pelo meio, para o `&` da URL chegar
//! inteiro. Falhar só tira a comodidade: o erro JSON-RPC continua a dizer que
//! URL abrir à mão.

use std::process::{Command, Stdio};

pub fn open_url(url: &str) -> bool {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(mut child) => {
            // Recolhe o processo para não ficar zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}
