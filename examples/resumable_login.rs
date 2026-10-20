//! The SMS step split across two processes, which is what the crate is for.
//!
//! ```text
//! cargo run --example resumable_login -- start          # asks for NIF + password, the AT sends an SMS
//! cargo run --example resumable_login -- finish 123456  # a different process, minutes later
//! ```
//!
//! `start` stores the half-done login (`.efatura-pending.json`, mode 600: it holds
//! live session cookies, so a real application must encrypt it at rest); `finish`
//! restores it, submits the code and writes the session where the `efatura`
//! example expects it (`$EFATURA_SESSION`, default `.efatura-session.json`).
//!
//! The pending state is single-use: `finish` deletes it before submitting the
//! code. A wrong code or an expired SMS means starting over, on purpose: the AT
//! suspends credentials after a few failures, so never retry automatically.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use efatura::{
    Authenticator, Client, Config, Credentials, LoginOutcome, PendingLogin, PendingLoginState,
};

type BoxError = Box<dyn std::error::Error>;

const PENDING_FILE: &str = ".efatura-pending.json";

fn session_path() -> PathBuf {
    std::env::var_os("EFATURA_SESSION")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".efatura-session.json"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), BoxError> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn save_session(client: &Client) -> Result<(), BoxError> {
    write_private(
        &session_path(),
        &serde_json::to_vec_pretty(&client.session()?)?,
    )?;
    eprintln!(
        "logged in as {}; session saved to {}",
        client.nif(),
        session_path().display()
    );
    Ok(())
}

fn prompt(label: &str) -> io::Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

async fn start() -> Result<(), BoxError> {
    let nif = match std::env::var("EFATURA_NIF") {
        Ok(nif) => nif,
        Err(_) => prompt("NIF: ")?,
    };
    let credentials = Credentials::new(nif, rpassword::prompt_password("Senha: ")?)?;

    match Authenticator::new().login(&credentials).await? {
        LoginOutcome::Authenticated(client) => save_session(&client)?,
        LoginOutcome::SmsRequired(pending) => {
            let state = pending.state()?;
            write_private(Path::new(PENDING_FILE), &serde_json::to_vec_pretty(&state)?)?;
            eprintln!(
                "SMS sent to {}. Within a few minutes run: cargo run --example resumable_login -- finish <CODE>",
                pending.phone().unwrap_or("?")
            );
        }
    }
    Ok(())
}

async fn finish(code: &str) -> Result<(), BoxError> {
    let raw = std::fs::read(PENDING_FILE)
        .map_err(|e| format!("no pending login ({e}); run `start` first"))?;
    // Single use: gone before the code is submitted, whatever happens next.
    std::fs::remove_file(PENDING_FILE)?;
    let state: PendingLoginState = serde_json::from_slice(&raw)?;

    let mut pending = PendingLogin::restore(state, Config::default())?;
    match pending.submit_code(code).await {
        Ok(client) => save_session(&client),
        Err(e) if e.is_auth_failure() => {
            Err(format!("the AT rejected the login ({e}); start again, and don't hammer it").into())
        }
        Err(e) if e.needs_login() => Err(format!("{e}; run `start` again").into()),
        Err(e) => Err(e.into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("start") => start().await,
        Some("finish") => match args.get(1) {
            Some(code) => finish(code).await,
            None => Err("usage: resumable_login finish <6-digit code>".into()),
        },
        _ => Err("usage: resumable_login start | finish <code>".into()),
    }
}
