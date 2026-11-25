//! Keeping the recent files when several writers share one preferences file: two windows, or the
//! app and an agent, each add what they did instead of the last writer's list winning.

use std::path::PathBuf;

use figcraft_engine::{Prefs, RecentChange};
use serde_json::json;

use super::{read_state, record_recents, write_prefs};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("figcraft-settings-recents-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A window's preferences as read from `dir` at its start.
fn window(dir: &std::path::Path) -> Prefs {
    read_state(dir).get("prefs").and_then(|p| serde_json::from_value(p.clone()).ok()).unwrap_or_default()
}

fn opened(prefs: &mut Prefs, path: &str) {
    let change = RecentChange::Opened(path.into());
    figcraft_engine::apply_recent_changes(&mut prefs.recent_files, std::slice::from_ref(&change));
    prefs.recent_changes.push(change);
}

fn kept(dir: &std::path::Path) -> Vec<String> {
    window(dir).recent_files
}

#[test]
fn two_windows_open_files_and_both_are_kept() {
    let d = dir("windows");
    let (mut a, mut b) = (window(&d), window(&d));
    opened(&mut a, "/work/a.figcraft");
    write_prefs(&d, &mut a);
    // B started before A kept its file, so its own list doesn't have it.
    opened(&mut b, "/work/b.figcraft");
    write_prefs(&d, &mut b);
    assert_eq!(kept(&d), ["/work/b.figcraft", "/work/a.figcraft"]);
    assert_eq!(b.recent_files, kept(&d), "the window now shows the list as kept");
    assert!(b.recent_changes.is_empty());

    // A quits with nothing new: it keeps B's file rather than writing its old list over it.
    write_prefs(&d, &mut a);
    assert_eq!(kept(&d), ["/work/b.figcraft", "/work/a.figcraft"]);
}

#[test]
fn a_file_taken_off_the_list_stays_off() {
    let d = dir("removed");
    let mut a = window(&d);
    opened(&mut a, "/work/a.figcraft");
    opened(&mut a, "/work/b.figcraft");
    write_prefs(&d, &mut a);
    let mut b = window(&d);
    b.recent_changes.push(RecentChange::Removed("/work/a.figcraft".into()));
    write_prefs(&d, &mut b);
    write_prefs(&d, &mut a);
    assert_eq!(kept(&d), ["/work/b.figcraft"], "the other window doesn't bring it back");
}

#[test]
fn an_older_file_without_the_list_keeps_this_sessions() {
    let d = dir("older");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(super::STATE), json!({"recents": ["/old/x.figcraft"]}).to_string()).unwrap();
    let mut prefs = Prefs { recent_files: vec!["/old/x.figcraft".into()], ..Prefs::default() };
    write_prefs(&d, &mut prefs);
    assert_eq!(kept(&d), ["/old/x.figcraft"]);
}

#[test]
fn an_agents_files_join_the_list_and_nothing_else_changes() {
    let d = dir("agent");
    let mut app = window(&d);
    app.user_name = "Ada".into();
    opened(&mut app, "/work/a.figcraft");
    write_prefs(&d, &mut app);

    record_recents(&d, &[RecentChange::Opened("made-by-agent.figcraft".into())]);
    let list = kept(&d);
    assert_eq!(list.len(), 2);
    assert!(std::path::Path::new(&list[0]).is_absolute() && list[0].ends_with("made-by-agent.figcraft"), "{list:?}");
    assert_eq!(window(&d).user_name, "Ada", "the other preferences are left as they are");

    // A preferences file that can't be read is never written over for a recent file.
    let broken = dir("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join(super::STATE), r#"{"prefs":{"user_name":"Ada","recent_files":["C:\work"]}}"#).unwrap();
    record_recents(&broken, &[RecentChange::Opened("/work/x.figcraft".into())]);
    assert!(std::fs::read_to_string(broken.join(super::STATE)).unwrap().contains("Ada"), "left as it was");

    // Turned off, an agent's files stay out.
    let mut app = window(&d);
    app.agent_recents = false;
    write_prefs(&d, &mut app);
    record_recents(&d, &[RecentChange::Opened("/work/hidden.figcraft".into())]);
    assert_eq!(kept(&d), list);
}
