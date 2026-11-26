//! Accounts on a figcraft-server: signing in with an email and a password (or making an account
//! with an admin's invite), the files shared with you, opening them under your account's name,
//! publishing a file you own, sharing it with people by email, and, for admins, invites and
//! accounts. The session's token is kept in the preferences, one per server (a password never
//! is); it signs in as you, so it never shows in what commands return or in the links shown.
//!
//! The server does the checking ([`figcraft_collab::message::AccountReq`]); these commands send
//! its requests through the app's link ([`NetRequest::Account`]) and keep what it says.

use figcraft_collab::Replica;
use figcraft_collab::message::{ACCOUNT_TOKEN, Access, AccountAuth, AccountFile, AccountReply, AccountReq, AccountUser, Member};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::CommandSpec;
use crate::live::{NetReply, NetRequest};
use crate::params::{boolean, num, req_string, string};
use crate::{EngineError, Result, Session, bad, spec};

/// The most servers one is signed in to at once.
pub const MAX_SIGNED_IN: usize = 20;

/// Signed in to a server: its address, the session's token and who.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedIn {
    pub server: String,
    /// The session's token: it signs in as this account (a password, in effect).
    pub token: String,
    pub id: u64,
    pub email: String,
    pub name: String,
    #[serde(default)]
    pub admin: bool,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn").field("server", &self.server).field("email", &self.email).field("admin", &self.admin).finish_non_exhaustive()
    }
}

impl SignedIn {
    /// What commands show of it (never the token).
    pub fn shown(&self) -> Value {
        json!({"server": self.server, "id": self.id, "email": self.email, "name": self.name, "admin": self.admin})
    }
}

/// What asking as nobody says.
const SIGN_IN_FIRST: &str = "sign in to this server first (account.signIn), or make an account with an invite (account.join)";

/// The server a command means: its `server`, else the one in Preferences.
fn server_of(s: &Session, cmd: &str, p: &Value) -> Result<String> {
    let server = string(p, "server").unwrap_or_else(|| s.prefs.publish_server.clone());
    let server = crate::online::plain_server(&server);
    if server.is_empty() {
        return Err(bad(cmd, "give the server's address (`server`, ws:// or wss://), or choose one in Preferences"));
    }
    Ok(server)
}

/// Where one is signed in to `server`.
fn signed_in<'a>(s: &'a Session, server: &str) -> Option<&'a SignedIn> {
    s.prefs.accounts.iter().find(|a| crate::online::same_server(&a.server, server))
}

/// Ask the server at `server` with `auth`: who it says is signed in and its answers to `reqs`.
fn ask(s: &mut Session, server: &str, auth: AccountAuth, reqs: Vec<AccountReq>) -> Result<(AccountUser, String, Vec<AccountReply>)> {
    let req = NetRequest::Account { server: server.to_string(), auth, reqs };
    match s.net(req)?.0 {
        NetReply::Account(Ok((AccountReply::SignedIn { token, user }, replies))) => Ok((user, token, replies)),
        NetReply::Account(Ok((other, _))) => Err(EngineError::Other(format!("the server answered {other:?} instead of signing in"))),
        NetReply::Account(Err(why)) => Err(EngineError::Other(why)),
        _ => Err(EngineError::Other("the server answered something else".into())),
    }
}

/// Keep (or replace) the session for `server`.
fn remember(s: &mut Session, server: &str, token: String, user: &AccountUser) {
    s.prefs.accounts.retain(|a| !crate::online::same_server(&a.server, server));
    s.prefs.accounts.insert(
        0,
        SignedIn { server: server.to_string(), token, id: user.id, email: user.email.clone(), name: user.name.clone(), admin: user.admin },
    );
    s.prefs.accounts.truncate(MAX_SIGNED_IN);
}

/// Send `reqs` as the account signed in to the command's server: the replies, each `Failed` turned
/// into an error. A session the server ended is forgotten.
fn as_me(s: &mut Session, cmd: &str, p: &Value, reqs: Vec<AccountReq>) -> Result<(String, Vec<AccountReply>)> {
    let server = server_of(s, cmd, p)?;
    let token = signed_in(s, &server).map(|a| a.token.clone()).ok_or_else(|| bad(cmd, SIGN_IN_FIRST))?;
    match ask(s, &server, AccountAuth::Session { token: token.clone() }, reqs) {
        Ok((user, _, replies)) => {
            // Name or role changes show at once.
            remember(s, &server, token, &user);
            if let Some(AccountReply::Failed { reason }) = replies.iter().find(|r| matches!(r, AccountReply::Failed { .. })) {
                return Err(EngineError::Other(reason.clone()));
            }
            Ok((server, replies))
        }
        Err(e) => {
            if e.to_string().contains("signed out") {
                s.prefs.accounts.retain(|a| !crate::online::same_server(&a.server, &server));
            }
            Err(e)
        }
    }
}

fn file_json(f: &AccountFile) -> Value {
    json!({"file": f.file, "name": f.name, "access": f.access.id(), "owner": f.owner, "updated": f.updated})
}

fn member_json(m: &Member) -> Value {
    json!({"user": m.user, "email": m.email, "name": m.name, "access": m.access.id(), "owner": m.owner})
}

fn user_json(u: &AccountUser) -> Value {
    json!({"id": u.id, "email": u.email, "name": u.name, "admin": u.admin, "disabled": u.disabled})
}

/// The join link that opens `file` on `server` with session `token`.
fn account_link(server: &str, file: &str, token: &str) -> String {
    format!("figcraft://join?server={server}&file={file}&token={ACCOUNT_TOKEN}{token}")
}

fn sign_in(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.signIn";
    let server = server_of(s, CMD, p)?;
    let auth = AccountAuth::Password { email: req_string(CMD, p, "email")?, password: req_string(CMD, p, "password")? };
    let (user, token, _) = ask(s, &server, auth, vec![])?;
    remember(s, &server, token, &user);
    Ok(json!({"server": server, "user": user_json(&user)}))
}

fn join_with_invite(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.join";
    let server = server_of(s, CMD, p)?;
    let auth = AccountAuth::Invite {
        code: req_string(CMD, p, "code")?.trim().to_string(),
        email: req_string(CMD, p, "email")?,
        name: req_string(CMD, p, "name")?,
        password: req_string(CMD, p, "password")?,
    };
    let (user, token, _) = ask(s, &server, auth, vec![])?;
    remember(s, &server, token, &user);
    Ok(json!({"server": server, "user": user_json(&user)}))
}

fn sign_out(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.signOut";
    let server = server_of(s, CMD, p)?;
    if signed_in(s, &server).is_none() {
        return Err(bad(CMD, "you aren't signed in to this server"));
    }
    // The server ends the session; it is forgotten here either way.
    let ended = as_me(s, CMD, p, vec![AccountReq::SignOut]).is_ok();
    s.prefs.accounts.retain(|a| !crate::online::same_server(&a.server, &server));
    Ok(json!({"signedOut": true, "serverEnded": ended}))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"accounts": s.prefs.accounts.iter().map(SignedIn::shown).collect::<Vec<_>>()}))
}

fn files(s: &mut Session, p: &Value) -> Result<Value> {
    let (server, replies) = as_me(s, "account.files", p, vec![AccountReq::Files])?;
    let Some(AccountReply::Files(files)) = replies.first() else { return Err(EngineError::Other("the server answered something else".into())) };
    Ok(json!({"server": server, "files": files.iter().map(file_json).collect::<Vec<_>>()}))
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.open";
    let server = server_of(s, CMD, p)?;
    let file = req_string(CMD, p, "file")?.trim().to_string();
    let token = signed_in(s, &server).map(|a| a.token.clone()).ok_or_else(|| bad(CMD, SIGN_IN_FIRST))?;
    let session = crate::live::start_join(s, CMD, &account_link(&server, &file, &token), None, None, None)?;
    Ok(json!({"server": server, "file": file, "session": session, "status": "connecting"}))
}

fn publish(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.publish";
    let doc = s.doc()?.clone();
    let name = string(p, "name").unwrap_or_else(|| doc.name.clone());
    let snapshot = Replica::new(doc).snapshot().map_err(|e| bad(CMD, e.to_string()))?;
    let (server, replies) = as_me(s, CMD, p, vec![AccountReq::Publish { name, snapshot }])?;
    let Some(AccountReply::Published { file }) = replies.first().cloned() else {
        return Err(EngineError::Other("the server answered something else".into()));
    };
    // The server's copy opens in a tab of its own, so what anyone changes syncs.
    let token = signed_in(s, &server).map(|a| a.token.clone()).ok_or_else(|| bad(CMD, SIGN_IN_FIRST))?;
    crate::live::start_join(s, CMD, &account_link(&server, &file, &token), None, None, None)?;
    Ok(json!({"server": server, "file": file}))
}

fn members(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.members";
    let file = req_string(CMD, p, "file")?.trim().to_string();
    let (_, replies) = as_me(s, CMD, p, vec![AccountReq::Members { file }])?;
    let Some(AccountReply::Members(m)) = replies.first() else { return Err(EngineError::Other("the server answered something else".into())) };
    Ok(json!({"members": m.iter().map(member_json).collect::<Vec<_>>()}))
}

fn share(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.share";
    let file = req_string(CMD, p, "file")?.trim().to_string();
    let email = req_string(CMD, p, "email")?;
    let access = match req_string(CMD, p, "access")?.trim() {
        "none" | "remove" => None,
        a => Some(Access::parse(a).ok_or_else(|| bad(CMD, "`access` is edit, comment, view or none"))?),
    };
    as_me(s, CMD, p, vec![AccountReq::SetMember { file: file.clone(), email, access }, AccountReq::Members { file }]).map(|(_, r)| match r.get(1) {
        Some(AccountReply::Members(m)) => json!({"members": m.iter().map(member_json).collect::<Vec<_>>()}),
        _ => json!({"done": true}),
    })
}

fn invite(s: &mut Session, p: &Value) -> Result<Value> {
    let admin = boolean(p, "admin").unwrap_or(false);
    let (server, replies) = as_me(s, "account.invite", p, vec![AccountReq::CreateInvite { admin }])?;
    let Some(AccountReply::Invite { id, code }) = replies.first() else {
        return Err(EngineError::Other("the server answered something else".into()));
    };
    Ok(json!({"server": server, "id": id, "code": code, "admin": admin}))
}

fn invites(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, replies) = as_me(s, "account.invites", p, vec![AccountReq::Invites])?;
    let Some(AccountReply::Invites(list)) = replies.first() else { return Err(EngineError::Other("the server answered something else".into())) };
    Ok(json!({"invites": list.iter().map(|i| json!({"id": i.id, "admin": i.admin, "created": i.created, "expires": i.expires})).collect::<Vec<_>>()}))
}

fn users(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, replies) = as_me(s, "account.users", p, vec![AccountReq::Users])?;
    let Some(AccountReply::Users(list)) = replies.first() else { return Err(EngineError::Other("the server answered something else".into())) };
    Ok(json!({"users": list.iter().map(user_json).collect::<Vec<_>>()}))
}

fn set_user(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "account.setUser";
    let id = num(p, "id").filter(|n| *n >= 0.0 && n.fract() == 0.0).ok_or_else(|| bad(CMD, "`id` is an account's id (account.users)"))? as u64;
    let (admin, disabled) = (boolean(p, "admin"), boolean(p, "disabled"));
    if admin.is_none() && disabled.is_none() {
        return Err(bad(CMD, "say `admin` or `disabled` (true or false)"));
    }
    as_me(s, CMD, p, vec![AccountReq::SetUser { id, admin, disabled }]).map(|_| json!({"done": true}))
}

fn simple(s: &mut Session, cmd: &str, p: &Value, req: AccountReq) -> Result<Value> {
    as_me(s, cmd, p, vec![req]).map(|_| json!({"done": true}))
}

/// The account commands.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "account.signIn",
            "Sign in",
            "{server? (ws:// or wss://; default the server in Preferences), email, password}  signs in to the server's accounts and keeps the session (never the password) in the preferences: {server, user: {id, email, name, admin, disabled}}",
            false,
            sign_in
        ),
        spec!(
            "account.join",
            "Make an account with an invite",
            "{server?, code (the one-time invite code an admin gave you), email, name, password (at least 8 characters)}  makes your account and signs in: {server, user}",
            false,
            join_with_invite
        ),
        spec!("account.signOut", "Sign out", "{server?}  ends the session on the server and forgets it here", false, sign_out),
        spec!(
            "account.status",
            "Accounts signed in",
            "{}  the servers you are signed in to: {accounts: [{server, id, email, name, admin}]}",
            false,
            status
        ),
        spec!(
            "account.files",
            "Your files on the server",
            "{server?}  the files shared with your account (an admin: every file), most recently changed first: {server, files: [{file, name, access (edit|comment|view), owner, updated}]}",
            false,
            files
        ),
        spec!(
            "account.open",
            "Open a file with your account",
            "{server?, file}  joins the file's session under your account's name, with what it may do there",
            false,
            open
        ),
        spec!(
            "account.publish",
            "Publish to your server",
            "{server?, name? (default the file's)}  keeps a copy of the open file on the server, owned by your account, and opens it (in a tab of its own, live): {server, file}",
            false,
            publish
        ),
        spec!(
            "account.members",
            "People with access",
            "{server?, file}  who may open the file (its owner first): {members: [{user, email, name, access, owner}]} (its owner or an admin asks)",
            false,
            members
        ),
        spec!(
            "account.share",
            "Share a file with someone",
            "{server?, file, email, access (edit|comment|view|none)}  lets the account with that email open the file with that access, or (none) no longer; whoever loses access is disconnected. Returns {members} (its owner or an admin asks)",
            false,
            share
        ),
        spec!("account.renameFile", "Rename a file on the server", "{server?, file, name} (its owner or an admin)", false, |s, p| {
            let req =
                AccountReq::RenameFile { file: req_string("account.renameFile", p, "file")?, name: req_string("account.renameFile", p, "name")? };
            simple(s, "account.renameFile", p, req)
        }),
        spec!(
            "account.removeFile",
            "Delete a file from the server",
            "{server?, file}  deletes it for everyone (its owner or an admin)",
            false,
            |s, p| {
                let req = AccountReq::RemoveFile { file: req_string("account.removeFile", p, "file")? };
                simple(s, "account.removeFile", p, req)
            }
        ),
        spec!("account.changePassword", "Change your password", "{server?, old, new}  your other sessions end", false, |s, p| {
            let req = AccountReq::ChangePassword {
                old: req_string("account.changePassword", p, "old")?,
                new: req_string("account.changePassword", p, "new")?,
            };
            simple(s, "account.changePassword", p, req)
        }),
        spec!(
            "account.invite",
            "Invite someone",
            "{server?, admin? (bool: the account will be an admin)}  a one-time code that makes an account, good for 7 days: {server, id, code, admin} (admins)",
            false,
            invite
        ),
        spec!(
            "account.invites",
            "Open invites",
            "{server?}  the invites not used yet: {invites: [{id, admin, created, expires}]} (admins)",
            false,
            invites
        ),
        spec!("account.revokeInvite", "Withdraw an invite", "{server?, id} (admins)", false, |s, p| {
            let req = AccountReq::RevokeInvite { id: req_string("account.revokeInvite", p, "id")? };
            simple(s, "account.revokeInvite", p, req)
        }),
        spec!(
            "account.users",
            "Accounts on the server",
            "{server?}  every account: {users: [{id, email, name, admin, disabled}]} (admins)",
            false,
            users
        ),
        spec!(
            "account.setUser",
            "Change an account",
            "{server?, id, admin? (bool), disabled? (bool)}  makes an account an admin or not, or disables it (its sessions end) or enables it again; not your own, and the server keeps one admin (admins)",
            false,
            set_user
        ),
    ]
}
