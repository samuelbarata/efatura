//! Small CLI to exercise the crate against the real portal.
//!
//! ```text
//! cargo run --example efatura -- login
//! cargo run --example efatura -- pending 2026-01-01 2026-12-31
//! cargo run --example efatura -- list 2026-01-01 2026-12-31 [SECTOR] [--json]
//! cargo run --example efatura -- show <ID> <DATE>
//! cargo run --example efatura -- set <ID> <DATE> <SECTOR>
//! ```
//!
//! At the SMS prompt, `r` asks the AT for a new code and `c` cancels the login.
//! The AT counts wrong passwords and wrong codes towards an account lockout: only
//! ever use your own account here.
//!
//! The session is kept in `$EFATURA_SESSION` (default `.efatura-session.json`).
//! Set `RUST_LOG=efatura=debug` to see every request.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use chrono::NaiveDate;
use efatura::{
    Authenticator, Client, Config, Credentials, DocumentId, DocumentKey, DocumentQuery,
    LoginOutcome, Sector, SessionState,
};

type BoxError = Box<dyn std::error::Error>;

fn session_path() -> PathBuf {
    std::env::var_os("EFATURA_SESSION")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".efatura-session.json"))
}

fn prompt(label: &str) -> io::Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn save(client: &Client) -> Result<(), BoxError> {
    let path = session_path();
    std::fs::write(&path, serde_json::to_vec_pretty(&client.session()?)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    eprintln!("session saved to {}", path.display());
    Ok(())
}

async fn restore() -> Result<Client, BoxError> {
    let raw = std::fs::read(session_path())
        .map_err(|e| format!("no saved session ({e}); run `login` first"))?;
    let state: SessionState = serde_json::from_slice(&raw)?;
    Ok(Client::from_session(state, Config::default())?)
}

async fn login() -> Result<(), BoxError> {
    let nif = match std::env::var("EFATURA_NIF") {
        Ok(nif) => nif,
        Err(_) => prompt("NIF: ")?,
    };
    let password = rpassword::prompt_password("Senha: ")?;
    let credentials = Credentials::new(nif, password)?;

    let client = match Authenticator::new().login(&credentials).await? {
        LoginOutcome::Authenticated(client) => client,
        LoginOutcome::SmsRequired(mut pending) => {
            eprintln!(
                "SMS sent to {} ({} sends remaining)",
                pending.phone().unwrap_or("?"),
                pending
                    .sends_remaining()
                    .map_or("?".into(), |n| n.to_string())
            );
            loop {
                let code = prompt("Código SMS ('r' resend, 'c' cancel the login): ")?;
                if code == "c" {
                    pending.cancel().await?;
                    eprintln!("login cancelled on the AT side");
                    return Ok(());
                }
                if code == "r" {
                    let remaining = pending.resend_code().await?;
                    eprintln!("new code sent ({remaining:?} remaining)");
                    continue;
                }
                match pending.submit_code(&code).await {
                    Ok(client) => break client,
                    Err(efatura::Error::InvalidSmsCode { remaining }) => {
                        eprintln!("wrong code ({remaining:?} attempts remaining)");
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    };
    eprintln!("logged in as {}", client.nif());
    save(&client)
}

fn date(arg: Option<&String>) -> Result<NaiveDate, BoxError> {
    Ok(arg.ok_or("missing date (YYYY-MM-DD)")?.parse()?)
}

fn euros(cents: Option<i64>) -> String {
    cents.map_or_else(
        || "?".into(),
        |c| format!("{}.{:02}€", c / 100, (c % 100).abs()),
    )
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();

    match args.first().map(|s| s.as_str()) {
        Some("login") => login().await?,
        Some(cmd @ ("pending" | "list")) => {
            let client = restore().await?;
            let mut query =
                DocumentQuery::new(date(args.get(1).copied())?, date(args.get(2).copied())?);
            if cmd == "pending" {
                query = query.state(efatura::DocumentState::Pending);
            }
            if let Some(sector) = args.get(3) {
                query = query.sector(sector.parse()?);
            }
            let documents = client.documents(&query).await?;
            if json {
                let raw: Vec<_> = documents.iter().map(|d| &d.raw).collect();
                println!("{}", serde_json::to_string_pretty(&raw)?);
            } else {
                for d in &documents {
                    println!(
                        "{}  {}  {:>10}  {:<4} {}",
                        d.id,
                        d.issue_date,
                        euros(d.total_cents),
                        d.sector.as_ref().map_or("-", Sector::code),
                        d.issuer_name.as_deref().unwrap_or("?"),
                    );
                }
                eprintln!("{} documents", documents.len());
            }
            save(&client)?;
        }
        Some("show") => {
            let client = restore().await?;
            let key = DocumentKey {
                id: args.get(1).ok_or("missing id")?.parse::<DocumentId>()?,
                issue_date: date(args.get(2).copied())?,
            };
            let detail = client.document(key).await?;
            println!("{detail:#?}");
            save(&client)?;
        }
        Some("set") => {
            let client = restore().await?;
            let key = DocumentKey {
                id: args.get(1).ok_or("missing id")?.parse::<DocumentId>()?,
                issue_date: date(args.get(2).copied())?,
            };
            let sector: Sector = args.get(3).ok_or("missing sector, e.g. C05")?.parse()?;
            let detail = client.categorize(key, &sector).await?;
            println!("{} now in {:?}", key.id, detail.sector);
            save(&client)?;
        }
        _ => {
            eprintln!(
                "usage: efatura <login|pending FROM TO|list FROM TO [SECTOR]|show ID DATE|set ID DATE SECTOR> [--json]"
            );
            eprintln!("sectors:");
            for s in Sector::ALL {
                eprintln!("  {}  {}", s.code(), s.label());
            }
        }
    }
    Ok(())
}
