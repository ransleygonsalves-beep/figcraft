//! FigCraft's settings folder and its preferences file (`state.json`), shared by the desktop app
//! and figcraft-cli.
//!
//! The recent files are kept by applying what a session did to them ([`Prefs::recent_changes`])
//! to the list as it is on disk now, so two windows, or the app and an agent, add up instead of
//! the last one to write keeping only its own list.

use std::path::{Path, PathBuf};

use figcraft_engine::{Prefs, RecentChange, apply_recent_changes};
use serde_json::{Value, json};

/// The settings folder's name in the platform folder, as it always was (the suite keeps existing
/// folder names), and craft-settings' app id: `FIGCRAFT_CONFIG_DIR` names another folder.
const APP_ID: &str = "FigCraft";

/// The preferences' file in the config folder.
pub const STATE: &str = "state.json";

/// Where the preferences, recovery copies and installed plugins live ([`settings_dir`]).
pub fn config_dir() -> Option<PathBuf> {
    settings_dir(&craft_settings::Probe { platform_config_dir: platform_dir(), ..Default::default() })
}

/// The platform's config folder, read from the environment as before: `%APPDATA%` on Windows,
/// so a test's own `APPDATA` keeps the app away from the user's settings.
fn platform_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base
}

/// The settings folder by the suite's rules (craft-settings), from what `probe` sees:
/// `FIGCRAFT_CONFIG_DIR` (set but empty: none, nothing read or written); a `portable` file next
/// to the executable (next to the `.app` on macOS), which puts them in `config/` beside it; else
/// `FigCraft` in the platform folder, where they always were. A portable folder without
/// preferences yet starts with a copy of the platform folder's, so an existing copy that becomes
/// portable keeps its settings; the original stays for an installed app. No platform folder: none.
pub fn settings_dir(probe: &craft_settings::Probe) -> Option<PathBuf> {
    use craft_settings::{Location, Portable, Rule};
    let resolved = craft_settings::paths::resolve(APP_ID, &Portable::DEFAULT, &Location::Auto, probe);
    match (&resolved.rule, &resolved.dir, &probe.platform_config_dir) {
        (Rule::Portable, Some(dir), Some(platform)) => seed(dir, &platform.join(APP_ID)),
        // The current directory instead: settings would follow wherever the app was started.
        (Rule::Fallback, ..) => return None,
        _ => {}
    }
    resolved.dir
}

/// Copy `from`'s preferences into the folder `to` when it has none (owner-only, as written).
fn seed(to: &Path, from: &Path) {
    let (target, source) = (to.join(STATE), from.join(STATE));
    if target.exists() || !source.is_file() {
        return;
    }
    let copied = std::fs::read(&source).and_then(|bytes| std::fs::create_dir_all(to).and_then(|()| write_private(&target, &bytes)));
    match copied {
        Ok(()) => log::info!("preferences: copied {} into the portable folder {}", source.display(), to.display()),
        Err(e) => log::warn!("preferences: couldn't copy them into the portable folder {}: {e}", to.display()),
    }
}

/// The preferences file in `dir` as JSON (`{}` when there is none or it can't be read).
pub fn read_state(dir: &Path) -> Value {
    std::fs::read_to_string(dir.join(STATE)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

/// The recent files as kept in `state`, if it keeps them.
fn kept_recents(state: &Value) -> Option<Vec<String>> {
    let list = state.get("prefs")?.get("recent_files")?.as_array()?;
    Some(list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

/// Keep `prefs` in `dir`: its recent-file changes are applied to the list as kept now (what
/// another window or an agent added stays), and `prefs` then holds that list with no changes
/// left. A file that keeps no list yet (none, or one from before the list moved into the
/// preferences) takes this session's.
pub fn write_prefs(dir: &Path, prefs: &mut Prefs) {
    if let Some(mut kept) = kept_recents(&read_state(dir)) {
        apply_recent_changes(&mut kept, &prefs.recent_changes);
        prefs.recent_files = kept;
    }
    prefs.recent_changes.clear();
    write_state(dir, &json!({"prefs": prefs}));
}

/// Add what an agent's session did to the recent files (`changes`, paths as it named them) to
/// the app's list in `dir`, unless the user turned that off (`pref.agentRecents`). Only the list
/// changes: the rest of the file is left as it is. Paths are made absolute, so the app can open
/// them whatever folder the agent ran in.
pub fn record_recents(dir: &Path, changes: &[RecentChange]) {
    if changes.is_empty() {
        return;
    }
    // A file that's there but can't be read is left alone: writing a new one would lose every
    // preference in it for a recent file.
    let mut state = match std::fs::read_to_string(dir.join(STATE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return log::warn!("recent files: couldn't read {}: {e}", dir.join(STATE).display()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => v,
            _ => return log::warn!("recent files: {} isn't preferences; left as it is", dir.join(STATE).display()),
        },
    };
    if state.get("prefs").and_then(|p| p.get("agent_recents")).and_then(Value::as_bool) == Some(false) {
        return;
    }
    let absolute = |p: &String| std::path::absolute(p).map_or_else(|_| p.clone(), |a| a.to_string_lossy().into_owned());
    let changes: Vec<RecentChange> = changes
        .iter()
        .map(|c| match c {
            RecentChange::Opened(p) => RecentChange::Opened(absolute(p)),
            RecentChange::Removed(p) => RecentChange::Removed(absolute(p)),
        })
        .collect();
    let mut list = kept_recents(&state).unwrap_or_default();
    apply_recent_changes(&mut list, &changes);
    if !state.get("prefs").is_some_and(Value::is_object) {
        state["prefs"] = json!({});
    }
    state["prefs"]["recent_files"] = json!(list);
    write_state(dir, &state);
}

/// Write `state` to `dir` atomically (a temporary file renamed over the old one: a crash leaves
/// either version whole; each process has a temporary file of its own). It holds online owner
/// tokens, so on Unix only the owner can read it.
fn write_state(dir: &Path, state: &Value) {
    let tmp = dir.join(format!("{STATE}.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| write_private(&tmp, state.to_string().as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, dir.join(STATE)));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("preferences: couldn't save them in {}: {e}", dir.display());
    }
}

/// Write `bytes` to `path`, owner read/write only on Unix from before the first byte (a leftover
/// file from a crash is narrowed too: the mode given at open only applies to new files).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    f.write_all(bytes)
}

#[cfg(test)]
mod tests;
