//! What the page keeps across reloads, in the browser's local storage: your name (Preferences ▸
//! Your name), so a reload or a later visit joins live sessions and comments as you, and the
//! sessions of the servers you signed in to (one per server address; never a password), so a
//! reload stays signed in. Signing out, or a session the server ended, takes it out again.

use figcraft_engine::Session;
use figcraft_engine::accounts::{MAX_SIGNED_IN, SignedIn};
use serde_json::json;

/// The sessions to keep, as the page's storage holds them.
pub fn keep_accounts(session: &Session) -> String {
    serde_json::to_string(&session.prefs.accounts).unwrap_or_default()
}

/// Put back the sessions the page kept: only ones for `ws://` or `wss://` servers (the page's
/// storage is anyone's to change), at most one per server.
pub fn restore_accounts(session: &mut Session, kept: Option<String>) {
    let Some(list) = kept.and_then(|k| serde_json::from_str::<Vec<SignedIn>>(&k).ok()) else { return };
    let mut out: Vec<SignedIn> = Vec::new();
    for a in list {
        let ok = (a.server.starts_with("wss://") || a.server.starts_with("ws://")) && !a.token.is_empty() && a.token.len() <= 256;
        if ok && !out.iter().any(|b| b.server == a.server) {
            out.push(a);
        }
    }
    out.truncate(MAX_SIGNED_IN);
    session.prefs.accounts = out;
}

/// Put back the name the page kept, checked as one typed in Preferences: the page's
/// storage is anyone's to change.
pub fn restore_name(session: &mut Session, kept: Option<String>) {
    if let Some(name) = kept
        && let Err(e) = session.execute("pref.userName", &json!({"name": name}))
    {
        log::warn!("the kept name wasn't used: {e}");
    }
}

#[cfg(test)]
mod tests {
    use figcraft_engine::Session;
    use serde_json::json;

    use super::{keep_accounts, restore_accounts, restore_name};

    /// What one page kept, the next one restores; junk in the storage is cleaned like typing.
    #[test]
    fn the_name_survives_a_reload() {
        let mut before = Session::new();
        before.execute("pref.userName", &json!({"name": "  Alice "})).unwrap();
        let kept = before.prefs.user_name.clone();
        let mut after = Session::new();
        restore_name(&mut after, Some(kept));
        assert_eq!(after.prefs.user_name, "Alice");
        let mut fresh = Session::new();
        restore_name(&mut fresh, None);
        assert_eq!(fresh.prefs.user_name, Session::new().prefs.user_name, "nothing kept: the default");
        restore_name(&mut fresh, Some(format!("Bob\u{7}{}", "x".repeat(500))));
        assert!(fresh.prefs.user_name.starts_with("Bobx") && fresh.prefs.user_name.chars().count() == 64);
    }

    /// A reload stays signed in; what isn't a server's session is dropped.
    #[test]
    fn sessions_survive_a_reload_and_junk_is_dropped() {
        let mut before = Session::new();
        figcraft_engine::accounts::use_token(&mut before.prefs, "wss://figcraft.example.com/", "a-session");
        let kept = keep_accounts(&before);
        let mut after = Session::new();
        restore_accounts(&mut after, Some(kept));
        assert_eq!(after.prefs.accounts, before.prefs.accounts);
        let junk = r#"[{"server":"javascript:alert(1)","token":"x","id":1,"email":"","name":""},{"server":"wss://a.example/","token":"","id":1,"email":"","name":""}]"#;
        let mut fresh = Session::new();
        restore_accounts(&mut fresh, Some(junk.into()));
        assert!(fresh.prefs.accounts.is_empty());
        restore_accounts(&mut fresh, Some("not json".into()));
        assert!(fresh.prefs.accounts.is_empty());
    }
}
