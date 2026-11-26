//! Accounts on a running server: the key makes the first admin's invite, the admin publishes a
//! file and invites a designer, who can open it only once it is shared with them, joins under
//! their account's name whatever their app calls them, and is disconnected when it stops being
//! shared. Wrong passwords are refused alike, and an accounts-only server refuses share links.

use std::time::Duration;

use figcraft_collab::Replica;
use figcraft_collab::message::{ACCOUNT_TOKEN, Access, AccountAuth, AccountReply, AccountReq, Admin, AdminReply};
use figcraft_collab_net::{Endpoint, Invite, NetLimits, manage};

use crate::test_util::{TempDir, config, join, key, live, pump};

fn ask(endpoint: &Endpoint, auth: AccountAuth, reqs: Vec<AccountReq>) -> Result<(AccountReply, Vec<AccountReply>), String> {
    manage::account(endpoint, auth, reqs, &NetLimits::default(), Duration::from_secs(10))
}

/// Why `s`'s join failed, once it has.
fn failed(s: &figcraft_engine::Session) -> Option<String> {
    match &s.live().status {
        figcraft_engine::live::Status::Failed { reason } => Some(reason.clone()),
        _ => None,
    }
}

fn signed(reply: AccountReply) -> (String, u64) {
    match reply {
        AccountReply::SignedIn { token, user } => (token, user.id),
        other => panic!("not signed in: {other:?}"),
    }
}

#[test]
fn files_open_to_the_accounts_they_are_shared_with() {
    let dir = TempDir::new("accounts");
    let mut server = crate::start(&config(dir.path())).unwrap();
    let endpoint = Endpoint::plain(server.addr().to_string());
    // The operator's key makes the first admin's invite (`figcraft-server invite --admin`).
    let made = manage::request(&endpoint, &key(dir.path()), vec![Admin::CreateInvite { admin: true }], &NetLimits::default(), Duration::from_secs(5));
    let AdminReply::Invite { code, .. } = made.unwrap().remove(0) else { panic!("no invite") };
    let invite = |code: &str, email: &str, name: &str| AccountAuth::Invite {
        code: code.into(),
        email: email.into(),
        name: name.into(),
        password: "a long password".into(),
    };
    // The admin makes their account, publishes a file and invites a designer.
    let mut doc = figcraft_doc::Document::new("Brand");
    doc.add_page("Page 1");
    let snapshot = Replica::new(doc).snapshot().unwrap();
    let (opened, replies) = ask(
        &endpoint,
        invite(&code, "admin@example.com", "Ada"),
        vec![AccountReq::Publish { name: "Brand".into(), snapshot }, AccountReq::CreateInvite { admin: false }],
    )
    .unwrap();
    let (admin_token, _) = signed(opened);
    let AccountReply::Published { file } = replies[0].clone() else { panic!("{:?}", replies[0]) };
    let AccountReply::Invite { code: designer_code, .. } = replies[1].clone() else { panic!("{:?}", replies[1]) };
    let (designer_token, _) = signed(ask(&endpoint, invite(&designer_code, "dee@example.com", "Dee Signer"), vec![]).unwrap().0);
    // Not shared yet: the designer doesn't see it and can't open it.
    let mine = |token: &str| ask(&endpoint, AccountAuth::Session { token: token.into() }, vec![AccountReq::Files]).unwrap().1.remove(0);
    assert_eq!(mine(&designer_token), AccountReply::Files(vec![]));
    let link = |token: &str| Invite { server: endpoint.clone(), file: file.clone(), token: format!("{ACCOUNT_TOKEN}{token}") }.link();
    let mut refused = join(&link(&designer_token), "Dee");
    pump(&mut [&mut refused], "the refusal", |s| failed(s[0]).is_some());
    assert!(failed(&refused).unwrap().contains("isn't shared with your account"), "{:?}", failed(&refused));
    // Shared with edit access: they see it, and join under their account's name.
    let set = |access: Option<Access>| {
        let req = AccountReq::SetMember { file: file.clone(), email: "DEE@example.com".into(), access };
        ask(&endpoint, AccountAuth::Session { token: admin_token.clone() }, vec![req]).unwrap().1.remove(0)
    };
    assert_eq!(set(Some(Access::Edit)), AccountReply::Done);
    match mine(&designer_token) {
        AccountReply::Files(files) => assert!(files.len() == 1 && files[0].access == Access::Edit && !files[0].owner, "{files:?}"),
        other => panic!("{other:?}"),
    }
    let mut ada = join(&link(&admin_token), "Ada");
    let mut dee = join(&link(&designer_token), "Somebody Else");
    pump(&mut [&mut ada, &mut dee], "both in", |s| live(s[0]) && live(s[1]) && s[0].live().peers.len() == 1);
    assert_eq!(ada.live().peers.values().next().map(|p| p.name.as_str()), Some("Dee Signer"), "the account's name, not the app's");
    assert_eq!(dee.live_summary()["access"], "edit");
    // Only the owner or an admin manages who may open it.
    let theirs = ask(&endpoint, AccountAuth::Session { token: designer_token.clone() }, vec![AccountReq::Members { file: file.clone() }]);
    assert!(matches!(theirs.unwrap().1[0], AccountReply::Failed { .. }));
    // No longer shared: they are disconnected.
    assert_eq!(set(None), AccountReply::Done);
    pump(&mut [&mut ada, &mut dee], "Dee leaving", |s| !live(s[1]) && s[0].live().peers.is_empty());
    drop((ada, dee, refused));
    server.stop();
}

#[test]
fn wrong_details_are_refused_and_accounts_only_refuses_links() {
    let dir = TempDir::new("accounts-only");
    let mut cfg = config(dir.path());
    cfg.accounts_only = true;
    let mut server = crate::start(&cfg).unwrap();
    let endpoint = Endpoint::plain(server.addr().to_string());
    let made = manage::request(&endpoint, &key(dir.path()), vec![Admin::CreateInvite { admin: true }], &NetLimits::default(), Duration::from_secs(5));
    let AdminReply::Invite { code, .. } = made.unwrap().remove(0) else { panic!() };
    let auth = AccountAuth::Invite { code, email: "admin@example.com".into(), name: "Ada".into(), password: "a long password".into() };
    signed(ask(&endpoint, auth, vec![]).unwrap().0);
    let wrong = ask(&endpoint, AccountAuth::Password { email: "admin@example.com".into(), password: "not it".into() }, vec![]).unwrap_err();
    let unknown = ask(&endpoint, AccountAuth::Password { email: "who@example.com".into(), password: "not it".into() }, vec![]).unwrap_err();
    assert!(wrong.contains(crate::accounts::WRONG) && unknown.contains(crate::accounts::WRONG), "{wrong} / {unknown}");
    assert!(ask(&endpoint, AccountAuth::Session { token: "made-up".into() }, vec![]).unwrap_err().contains("signed out"));
    // A share link made with the key exists, but doesn't let anyone in here.
    let mut doc = figcraft_doc::Document::new("Plan");
    doc.add_page("Page 1");
    let snapshot = Replica::new(doc).snapshot().unwrap();
    let opened = ask(
        &endpoint,
        AccountAuth::Password { email: "admin@example.com".into(), password: "a long password".into() },
        vec![AccountReq::Publish { name: "Plan".into(), snapshot }],
    );
    let AccountReply::Published { file } = opened.unwrap().1.remove(0) else { panic!() };
    let made = manage::request(
        &endpoint,
        &key(dir.path()),
        vec![Admin::CreateLink { file: file.clone(), access: Access::View, label: "Old".into() }],
        &NetLimits::default(),
        Duration::from_secs(5),
    );
    let AdminReply::Link(link) = made.unwrap().remove(0) else { panic!() };
    let mut b = join(&Invite { server: endpoint.clone(), file, token: link.token }.link(), "Bo");
    pump(&mut [&mut b], "the refusal", |s| failed(s[0]).is_some());
    assert!(failed(&b).unwrap().contains("accounts only"), "{:?}", failed(&b));
    drop(b);
    server.stop();
}

/// The app's commands, as the UI and agents use them: make accounts, publish, share by email,
/// open under the account's name; the session's token never shows in what they return.
#[test]
fn the_apps_account_commands_sign_in_share_and_open() {
    let dir = TempDir::new("account-commands");
    let mut server = crate::start(&config(dir.path())).unwrap();
    let url = format!("ws://{}/", server.addr());
    let endpoint = Endpoint::plain(server.addr().to_string());
    let invite = |admin: bool| {
        let made = manage::request(&endpoint, &key(dir.path()), vec![Admin::CreateInvite { admin }], &NetLimits::default(), Duration::from_secs(5));
        let AdminReply::Invite { code, .. } = made.unwrap().remove(0) else { panic!() };
        code
    };
    let mut ada = crate::test_util::session("Ada");
    let code = invite(true);
    let joined = ada
        .execute(
            "account.join",
            &serde_json::json!({"server": url, "code": code, "email": "ada@example.com", "name": "Ada", "password": "a long password"}),
        )
        .unwrap();
    assert_eq!(joined["user"]["admin"], true);
    assert!(!serde_json::to_string(&ada.execute("account.status", &serde_json::json!({})).unwrap()).unwrap().contains(&ada.prefs.accounts[0].token));
    ada.execute("shape.rectangle", &serde_json::json!({"x": 0, "y": 0, "width": 50, "height": 50})).unwrap();
    let published = ada.execute("account.publish", &serde_json::json!({"server": url, "name": "Brand"})).unwrap();
    let file = published["file"].as_str().unwrap().to_string();
    pump(&mut [&mut ada], "Ada in her file", |s| live(s[0]));
    let link = ada.live_summary()["link"].as_str().unwrap_or_default().to_string();
    assert!(!link.contains(&ada.prefs.accounts[0].token), "the shown link hides the session: {link}");
    // Ada invites Dee from the app.
    let code = ada.execute("account.invite", &serde_json::json!({"server": url})).unwrap()["code"].as_str().unwrap().to_string();
    let mut dee = crate::test_util::session("Dee");
    dee.execute(
        "account.join",
        &serde_json::json!({"server": url, "code": code, "email": "dee@example.com", "name": "Dee", "password": "another password"}),
    )
    .unwrap();
    assert_eq!(dee.execute("account.files", &serde_json::json!({"server": url})).unwrap()["files"], serde_json::json!([]));
    let shared =
        ada.execute("account.share", &serde_json::json!({"server": url, "file": file, "email": "dee@example.com", "access": "comment"})).unwrap();
    assert_eq!(shared["members"].as_array().unwrap().len(), 2, "{shared}");
    let files = dee.execute("account.files", &serde_json::json!({"server": url})).unwrap();
    assert_eq!(files["files"][0]["access"], "comment", "{files}");
    dee.execute("account.open", &serde_json::json!({"server": url, "file": file})).unwrap();
    pump(&mut [&mut ada, &mut dee], "Dee in", |s| live(s[1]) && !s[0].live().peers.is_empty());
    assert_eq!(dee.live_summary()["access"], "comment");
    // Dee isn't an admin: admin requests are refused with a reason.
    let e = dee.execute("account.users", &serde_json::json!({"server": url})).unwrap_err().to_string();
    assert!(e.contains("only an admin"), "{e}");
    // Signing out forgets the session here and ends it on the server.
    assert_eq!(dee.execute("account.signOut", &serde_json::json!({"server": url})).unwrap()["serverEnded"], true);
    assert!(dee.prefs.accounts.is_empty());
    drop((ada, dee));
    server.stop();
}

/// Version history on the server, through the app's commands: a version saved by name says who
/// saved it and who edited since; a designer views and restores it for everyone in the file; and
/// when everyone has left, the server keeps a checkpoint naming who changed the file since.
#[test]
fn the_servers_version_history_says_who_edited_and_restores_for_everyone() {
    use serde_json::json;
    let dir = TempDir::new("account-history");
    let mut server = crate::start(&config(dir.path())).unwrap();
    let url = format!("ws://{}/", server.addr());
    let endpoint = Endpoint::plain(server.addr().to_string());
    let invite = |admin: bool| {
        let made = manage::request(&endpoint, &key(dir.path()), vec![Admin::CreateInvite { admin }], &NetLimits::default(), Duration::from_secs(5));
        let AdminReply::Invite { code, .. } = made.unwrap().remove(0) else { panic!() };
        code
    };
    let account = |name: &str, admin: bool| {
        let mut s = crate::test_util::session(name);
        let email = format!("{}@example.com", name.to_lowercase());
        s.execute("account.join", &json!({"server": url, "code": invite(admin), "email": email, "name": name, "password": "a long password"}))
            .unwrap();
        s
    };
    let mut ada = account("Ada", true);
    let mut dee = account("Dee", false);
    ada.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 50, "height": 50})).unwrap();
    let file = ada.execute("account.publish", &json!({"server": url, "name": "Brand"})).unwrap()["file"].as_str().unwrap().to_string();
    ada.execute("account.share", &json!({"server": url, "file": file, "email": "dee@example.com", "access": "edit"})).unwrap();
    dee.execute("account.open", &json!({"server": url, "file": file})).unwrap();
    pump(&mut [&mut ada, &mut dee], "both in", |s| live(s[0]) && live(s[1]) && !s[0].live().peers.is_empty());
    let layers = |s: &figcraft_engine::Session| s.doc().unwrap().node(s.page().unwrap()).unwrap().children.len();
    // Both edit, then Ada saves a version (the open tab's file: no `file` needed).
    dee.execute("shape.ellipse", &json!({"x": 100, "y": 0, "width": 40, "height": 40})).unwrap();
    pump(&mut [&mut ada, &mut dee], "Dee's ellipse", |s| layers(s[0]) == 2);
    ada.execute("shape.rectangle", &json!({"x": 0, "y": 100, "width": 20, "height": 20})).unwrap();
    pump(&mut [&mut ada, &mut dee], "Ada's rectangle", |s| layers(s[1]) == 3);
    let saved = ada.execute("account.saveVersion", &json!({"name": "Review"})).unwrap();
    let v = &saved["versions"][0];
    assert_eq!((v["name"].as_str(), v["by"].as_str(), v["auto"].as_bool()), (Some("Review"), Some("Ada"), Some(false)), "{saved}");
    let editors: Vec<&str> = v["editors"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
    assert!(editors.contains(&"Ada") && editors.contains(&"Dee"), "{saved}");
    let id = v["id"].as_str().unwrap().to_string();
    // Dee changes more, looks at the version, and restores it: Ada sees it too.
    dee.execute("shape.rectangle", &json!({"x": 200, "y": 0, "width": 20, "height": 20})).unwrap();
    pump(&mut [&mut ada, &mut dee], "Dee's second layer", |s| layers(s[0]) == 4);
    assert_eq!(dee.execute("account.versions", &json!({})).unwrap()["versions"].as_array().unwrap().len(), 1);
    dee.execute("account.previewVersion", &json!({"id": id})).unwrap();
    assert!(dee.ds().unwrap().preview.is_some());
    dee.execute("account.restoreVersion", &json!({"id": id})).unwrap();
    assert!(dee.ds().unwrap().preview.is_none());
    assert_eq!(layers(&dee), 3);
    pump(&mut [&mut ada, &mut dee], "the restore reaching Ada", |s| layers(s[0]) == 3);
    // Only the owner or an admin deletes versions.
    assert!(dee.execute("account.deleteVersion", &json!({"id": id})).is_err());
    // Everyone leaves: the server keeps a checkpoint of what changed since, naming who.
    drop((ada, dee));
    let admin = AccountAuth::Password { email: "ada@example.com".into(), password: "a long password".into() };
    let start = std::time::Instant::now();
    let history = loop {
        let reply = ask(&endpoint, admin.clone(), vec![AccountReq::Versions { file: file.clone() }]).unwrap().1.remove(0);
        let AccountReply::Versions(list) = reply else { panic!("{reply:?}") };
        if list.len() == 2 || start.elapsed() > Duration::from_secs(20) {
            break list;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(history.len(), 2, "{history:?}");
    assert!(history[0].auto && history[0].editors == vec!["Dee".to_string()], "{history:?}");
    server.stop();
}
