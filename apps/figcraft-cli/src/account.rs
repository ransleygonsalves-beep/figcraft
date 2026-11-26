//! `figcraft-cli account login | logout | status`: an agent's way into your team's server without
//! a password in the chat. `login` asks for your email and password in the terminal (the password
//! without echo), makes a token for agents with it (limited access, ends after 30 days, shows as
//! "Claude for <you>"), ends the password's own session, and keeps the token in the settings
//! folder, readable only by you. `figcraft-cli mcp` signs in with it ([`credentials`]);
//! `FIGCRAFT_TOKEN` and `FIGCRAFT_SERVER` take its place when set. The token is never printed.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use figcraft_mcp::Backend as _;
use serde_json::{Value, json};

use crate::CliResult;

/// The file the token is kept in, in the settings folder.
const TOKEN_FILE: &str = "agent-token.json";

/// The settings folder (the app's: `FigCraft` in the platform's config folder, or
/// `FIGCRAFT_CONFIG_DIR`).
fn settings_dir() -> Option<PathBuf> {
    use craft_settings::paths::{Location, Portable, Probe, resolve};
    resolve("FigCraft", &Portable::DEFAULT, &Location::Auto, &Probe::default()).dir
}

fn token_path() -> CliResult<PathBuf> {
    Ok(settings_dir().ok_or("there's no settings folder to keep the token in (set FIGCRAFT_CONFIG_DIR)")?.join(TOKEN_FILE))
}

/// Who signs in: the server and token, and where they came from (shown, never the token).
pub struct Credentials {
    pub server: String,
    pub token: String,
    pub from: &'static str,
}

/// What `figcraft-cli mcp` signs in with: `FIGCRAFT_TOKEN` (with `FIGCRAFT_SERVER`, else the kept
/// file's server), else the token `account login` kept. `None`: neither.
pub fn credentials() -> Option<Credentials> {
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let kept = token_path().ok().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let kept_field = |k: &str| kept.as_ref().and_then(|v| v[k].as_str()).map(str::to_string).filter(|s| !s.is_empty());
    if let Some(token) = env("FIGCRAFT_TOKEN") {
        let server = env("FIGCRAFT_SERVER").or_else(|| kept_field("server"))?;
        return Some(Credentials { server, token, from: "FIGCRAFT_TOKEN" });
    }
    let (server, token) = (kept_field("server")?, kept_field("token")?);
    // FIGCRAFT_SERVER names the server to use; the kept token is only for the server it was made on.
    if env("FIGCRAFT_SERVER").is_some_and(|s| figcraft_engine::online::plain_server(&s) != figcraft_engine::online::plain_server(&server)) {
        return None;
    }
    Some(Credentials { server, token, from: "the token figcraft-cli account login kept" })
}

/// A session signed in with `c` (no document is needed for account commands).
pub fn session_with(c: Option<&Credentials>) -> figcraft_engine::Session {
    let mut session = figcraft_engine::Session::new();
    if let Some(c) = c {
        figcraft_engine::accounts::use_token(&mut session.prefs, &c.server, &c.token);
    } else if let Ok(server) = std::env::var("FIGCRAFT_SERVER")
        && !server.trim().is_empty()
    {
        session.prefs.publish_server = figcraft_engine::online::plain_server(&server);
    }
    session
}

fn run(h: &mut figcraft_mcp::Headless, command: &str, params: Value) -> Result<Value, String> {
    h.call("engine.execute", json!({"command": command, "params": params}))
}

fn prompt(label: &str) -> CliResult<String> {
    eprint!("{label}: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| format!("couldn't read your answer: {e}"))?;
    Ok(line.trim().to_string())
}

/// `account login`.
pub fn login(server: Option<&str>, email: Option<&str>, access: &str, label: &str) -> CliResult<std::process::ExitCode> {
    let server = match server.map(str::to_string).or_else(|| std::env::var("FIGCRAFT_SERVER").ok()).filter(|s| !s.trim().is_empty()) {
        Some(s) => figcraft_engine::online::plain_server(&s),
        None => return Err(crate::CliError::Usage("give the server: --server wss://figcraft.example.com/ (or FIGCRAFT_SERVER)".into())),
    };
    let email = match email {
        Some(e) => e.to_string(),
        None => prompt("Email")?,
    };
    let password = rpassword::prompt_password("Password: ").map_err(|e| format!("couldn't read the password: {e}"))?;
    let mut h = figcraft_mcp::Headless::with_session(figcraft_engine::Session::new());
    let signed = run(&mut h, "account.signIn", json!({"server": server, "email": email, "password": password}))?;
    drop(password);
    let made = run(&mut h, "account.agentToken", json!({"server": server, "access": access, "label": label}));
    // The password's own session isn't needed any more, whatever happened.
    let _ = run(&mut h, "account.signOut", json!({"server": server}));
    let made = made?;
    let token = made["token"].as_str().ok_or("the server didn't give a token")?;
    let path = token_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    }
    let body = serde_json::to_vec_pretty(&json!({"server": server, "token": token, "expires": made["expires"]})).unwrap_or_default();
    write_private(&path, &body)?;
    let user = &signed["user"];
    println!(
        "Signed in to {server} as {} ({}). figcraft-cli mcp now works in your files as \"{label} for {}\" with {access} access, \
         using a token kept in {} (only you can read it). It ends in 30 days; end it sooner with figcraft-cli account logout.",
        user["name"].as_str().unwrap_or_default(),
        user["email"].as_str().unwrap_or_default(),
        user["name"].as_str().unwrap_or_default(),
        path.display()
    );
    Ok(std::process::ExitCode::SUCCESS)
}

/// `account logout`: end the kept token on the server and delete it here.
pub fn logout() -> CliResult<std::process::ExitCode> {
    let path = token_path()?;
    let Some(c) = credentials().filter(|c| !c.from.starts_with("FIGCRAFT_TOKEN")) else {
        println!("No token is kept here (nothing to sign out of).");
        return Ok(std::process::ExitCode::SUCCESS);
    };
    let mut h = figcraft_mcp::Headless::with_session(session_with(Some(&c)));
    let ended = run(&mut h, "account.signOut", json!({"server": c.server}));
    std::fs::remove_file(&path).map_err(|e| format!("couldn't delete {}: {e}", path.display()))?;
    match ended {
        Ok(_) => println!("Signed out of {}: the token was ended there and deleted here.", c.server),
        Err(e) => println!("Deleted the token here; the server couldn't be told ({e}), so it ends by itself within 30 days."),
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `account status`: which server and account agents sign in as, checked with the server.
pub fn status() -> CliResult<std::process::ExitCode> {
    let Some(c) = credentials() else {
        println!("Not signed in: run figcraft-cli account login --server wss://… (or set FIGCRAFT_TOKEN and FIGCRAFT_SERVER).");
        return Ok(std::process::ExitCode::from(1));
    };
    let mut h = figcraft_mcp::Headless::with_session(session_with(Some(&c)));
    match run(&mut h, "account.me", json!({"server": c.server})) {
        Ok(v) => {
            let u = &v["user"];
            println!(
                "Signed in to {} as {} ({}), with {}.",
                c.server,
                u["name"].as_str().unwrap_or_default(),
                u["email"].as_str().unwrap_or_default(),
                c.from
            );
            Ok(std::process::ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("The token for {} ({}) doesn't work: {}", c.server, c.from, crate::files::plain(&e));
            Ok(std::process::ExitCode::from(1))
        }
    }
}

/// Write `bytes` to `path` so only this user can read it: mode 0600 on Unix; on Windows the
/// file's inherited permissions are replaced with full control for this user only.
fn write_private(path: &Path, bytes: &[u8]) -> CliResult<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600)).map_err(|e| format!("couldn't protect {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    drop(f);
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").map_err(|_| "couldn't tell who you are to protect the token file (USERNAME isn't set)")?;
        let ok = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("{user}:F"))
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = std::fs::remove_file(path);
            return Err(format!("couldn't make {} readable only by you, so the token wasn't kept", path.display()).into());
        }
    }
    Ok(())
}
