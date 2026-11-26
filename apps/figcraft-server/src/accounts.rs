//! Accounts: the people who sign in to this server, the one-time invites that make them, and
//! their sessions (`<data>/accounts.json`, owner-only on Unix). A password is kept as an Argon2id
//! hash (its PHC string), an invite code and a session token as their SHA-256; nothing readable
//! that lets anyone in is kept.
//!
//! An admin makes an invite (`figcraft-server invite --admin` makes the first admin's); its
//! one-time code makes an account with an email, a name and a password, and signs it in. A
//! session lasts while it is used at least every [`SESSION_IDLE_SECS`]; disabling an account,
//! changing its password or signing out ends sessions. Which files an account may open is kept
//! with each file (`meta.json`), not here.

use std::path::{Path, PathBuf};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use figcraft_collab::message::AccountUser;
use serde::{Deserialize, Serialize};

use crate::disk;
use crate::store::{hash, now};

/// The file the accounts are kept in, in the data directory.
pub const ACCOUNTS: &str = "accounts.json";
/// A session not used for this long ends (90 days).
pub const SESSION_IDLE_SECS: u64 = 90 * 24 * 3600;
/// An invite not used within this long stops working (7 days).
pub const INVITE_SECS: u64 = 7 * 24 * 3600;
/// The shortest password taken.
pub const MIN_PASSWORD: usize = 8;
/// The longest password, email and name taken (characters).
const MAX_PASSWORD: usize = 256;
const MAX_EMAIL: usize = 254;
const MAX_NAME: usize = 64;
/// The most accounts, open invites and sessions kept.
const MAX_USERS: usize = 10_000;
const MAX_INVITES: usize = 1_000;
const MAX_SESSIONS: usize = 50_000;
/// A session's last use is written at most this often (seconds), not on every request.
const TOUCH_EVERY: u64 = 3600;

/// What every wrong sign-in says, whichever part was wrong (an address isn't told which emails
/// have accounts).
pub const WRONG: &str = "wrong email or password";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    users: Vec<User>,
    #[serde(default)]
    invites: Vec<Invite>,
    #[serde(default)]
    sessions: Vec<SessionRec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct User {
    id: u64,
    /// Lower case.
    email: String,
    name: String,
    /// Argon2id, as a PHC string.
    password: String,
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    disabled: bool,
    created: u64,
}

impl User {
    fn shown(&self) -> AccountUser {
        AccountUser { id: self.id, email: self.email.clone(), name: self.name.clone(), admin: self.admin, disabled: self.disabled }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Invite {
    id: String,
    /// SHA-256 of the code (hex).
    hash: String,
    admin: bool,
    created: u64,
    expires: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SessionRec {
    /// SHA-256 of the token (hex): the session's id.
    hash: String,
    user: u64,
    created: u64,
    used: u64,
}

/// The accounts, as kept and in memory.
pub struct Accounts {
    path: PathBuf,
    kept: Kept,
}

/// A session that signed in: its token (given once) and who.
#[derive(Clone, Debug, PartialEq)]
pub struct Signed {
    pub token: String,
    /// Its id (the token's hash).
    pub session: String,
    pub user: AccountUser,
}

impl Accounts {
    /// The accounts kept under `root` (none yet: an empty file is made on the first change).
    pub fn load(root: &Path) -> Result<Accounts, String> {
        let path = root.join(ACCOUNTS);
        let kept = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{ACCOUNTS} is damaged ({e})"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Kept::default(),
            Err(e) => return Err(format!("couldn't read {ACCOUNTS}: {e}")),
        };
        Ok(Accounts { path, kept })
    }

    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.kept).map_err(|e| e.to_string())?;
        disk::write_atomic_private(&self.path, &bytes).map_err(|e| format!("couldn't keep the accounts: {e}"))
    }

    /// Change the accounts with `f`, keeping the change only when `f` went through and it could be
    /// written (otherwise they are as they were).
    fn change<T>(&mut self, f: impl FnOnce(&mut Kept) -> Result<T, String>) -> Result<T, String> {
        let before = self.kept.clone();
        let out = f(&mut self.kept).and_then(|out| self.save().map(|()| out));
        if out.is_err() {
            self.kept = before;
        }
        out
    }

    /// Whether any account exists (the first one is made from an invite the command line prints).
    pub fn is_empty(&self) -> bool {
        self.kept.users.is_empty()
    }

    pub fn user(&self, id: u64) -> Option<AccountUser> {
        self.kept.users.iter().find(|u| u.id == id).map(User::shown)
    }

    /// The account signed in with session token `token`, while it lasts and isn't disabled; using
    /// it keeps it going.
    pub fn session(&mut self, token: &str) -> Option<(String, AccountUser)> {
        let h = hash(token);
        let t = now();
        let rec = self.kept.sessions.iter().find(|s| s.hash == h)?;
        if t.saturating_sub(rec.used) > SESSION_IDLE_SECS {
            return None;
        }
        let user = self.kept.users.iter().find(|u| u.id == rec.user && !u.disabled)?.shown();
        if t.saturating_sub(rec.used) > TOUCH_EVERY {
            let _ = self.change(|k| {
                if let Some(s) = k.sessions.iter_mut().find(|s| s.hash == h) {
                    s.used = t;
                }
                Ok(())
            });
        }
        Some((h, user))
    }

    /// Sign in with `email` and `password`: a new session.
    pub fn sign_in(&mut self, email: &str, password: &str) -> Result<Signed, String> {
        let email = email.trim().to_lowercase();
        let Some(user) = self.kept.users.iter().find(|u| u.email == email).cloned() else {
            // As slow as a real check, so the time taken doesn't tell which emails have accounts.
            let _ = verify(password, dummy_hash());
            return Err(WRONG.into());
        };
        if !verify(password, &user.password) {
            return Err(WRONG.into());
        }
        if user.disabled {
            return Err("this account is disabled: ask your admin".into());
        }
        self.new_session(user.id)
    }

    fn new_session(&mut self, user: u64) -> Result<Signed, String> {
        let token = figcraft_collab_net::token::secret()?;
        let session = hash(&token);
        let t = now();
        let shown = self.user(user).ok_or("the account is gone")?;
        let s = session.clone();
        self.change(|k| {
            k.sessions.retain(|x| t.saturating_sub(x.used) <= SESSION_IDLE_SECS);
            if k.sessions.len() >= MAX_SESSIONS {
                // The least recently used one goes.
                if let Some(i) = k.sessions.iter().enumerate().min_by_key(|(_, x)| x.used).map(|(i, _)| i) {
                    k.sessions.remove(i);
                }
            }
            k.sessions.push(SessionRec { hash: s, user, created: t, used: t });
            Ok(())
        })?;
        Ok(Signed { token, session, user: shown })
    }

    /// End the session `session` (its id).
    pub fn sign_out(&mut self, session: &str) -> Result<(), String> {
        self.change(|k| {
            k.sessions.retain(|s| s.hash != session);
            Ok(())
        })
    }

    /// A one-time code that makes an account (an admin one when `admin`): its id and the code.
    pub fn create_invite(&mut self, admin: bool) -> Result<(String, String), String> {
        let code = figcraft_collab_net::token::secret()?;
        let id = figcraft_collab_net::token::file_id()?;
        let t = now();
        let (h, i) = (hash(&code), id.clone());
        self.change(|k| {
            k.invites.retain(|x| x.expires > t);
            if k.invites.len() >= MAX_INVITES {
                return Err(format!("there are {MAX_INVITES} invites not used yet: withdraw some first"));
            }
            k.invites.push(Invite { id: i, hash: h, admin, created: t, expires: t + INVITE_SECS });
            Ok(())
        })?;
        Ok((id, code))
    }

    /// The invites not used yet: (id, admin, created, expires).
    pub fn invites(&self) -> Vec<(String, bool, u64, u64)> {
        let t = now();
        self.kept.invites.iter().filter(|i| i.expires > t).map(|i| (i.id.clone(), i.admin, i.created, i.expires)).collect()
    }

    pub fn revoke_invite(&mut self, id: &str) -> Result<(), String> {
        if !self.kept.invites.iter().any(|i| i.id == id) {
            return Err("no such invite".into());
        }
        self.change(|k| {
            k.invites.retain(|i| i.id != id);
            Ok(())
        })
    }

    /// Whether `code` is an invite still open (checked before the work of making an account).
    pub fn invite_open(&self, code: &str) -> bool {
        let (h, t) = (hash(code), now());
        self.kept.invites.iter().any(|i| i.hash == h && i.expires > t)
    }

    /// Make an account with invite `code` and sign it in. The invite is used up.
    pub fn redeem(&mut self, code: &str, email: &str, name: &str, password: &str) -> Result<Signed, String> {
        let (h, t) = (hash(code), now());
        let Some(invite) = self.kept.invites.iter().find(|i| i.hash == h && i.expires > t).cloned() else {
            return Err("this invite doesn't work: it was used, withdrawn or is too old; ask for a new one".into());
        };
        let email = check_email(email)?;
        let name = clean_name(name);
        if name.is_empty() {
            return Err("give your name: it shows on your cursor and comments".into());
        }
        check_password(password)?;
        if self.kept.users.iter().any(|u| u.email == email) {
            return Err("there is already an account with that email: sign in instead".into());
        }
        if self.kept.users.len() >= MAX_USERS {
            return Err(format!("this server keeps at most {MAX_USERS} accounts"));
        }
        let password = hash_password(password)?;
        let id = self.change(|k| {
            k.next_id = k.next_id.max(k.users.iter().map(|u| u.id).max().unwrap_or(0)) + 1;
            let id = k.next_id;
            k.users.push(User { id, email, name, password, admin: invite.admin, disabled: false, created: t });
            k.invites.retain(|i| i.id != invite.id);
            Ok(id)
        })?;
        log::info!("event=account_made user={id} admin={}", invite.admin);
        self.new_session(id)
    }

    pub fn users(&self) -> Vec<AccountUser> {
        self.kept.users.iter().map(User::shown).collect()
    }

    /// The account with `email` (any case).
    pub fn by_email(&self, email: &str) -> Option<AccountUser> {
        let email = email.trim().to_lowercase();
        self.kept.users.iter().find(|u| u.email == email).map(User::shown)
    }

    /// Make account `id` an admin or not, or disable or enable it, as admin `by` asks. Disabling
    /// ends its sessions. The last admin can't stop being one, and no admin changes their own.
    pub fn set_user(&mut self, by: u64, id: u64, admin: Option<bool>, disabled: Option<bool>) -> Result<(), String> {
        if by == id {
            return Err("you can't change your own account here".into());
        }
        if !self.kept.users.iter().any(|u| u.id == id) {
            return Err("no such account".into());
        }
        self.change(|k| {
            if let Some(u) = k.users.iter_mut().find(|u| u.id == id) {
                if let Some(a) = admin {
                    u.admin = a;
                }
                if let Some(d) = disabled {
                    u.disabled = d;
                }
            }
            if !k.users.iter().any(|u| u.admin && !u.disabled) {
                return Err("the server needs at least one admin who isn't disabled".into());
            }
            if disabled == Some(true) {
                k.sessions.retain(|s| s.user != id);
            }
            Ok(())
        })
    }

    /// Change account `user`'s password; its sessions other than `keep` end.
    pub fn change_password(&mut self, user: u64, keep: &str, old: &str, new: &str) -> Result<(), String> {
        let current = self.kept.users.iter().find(|u| u.id == user).map(|u| u.password.clone()).ok_or("the account is gone")?;
        if !verify(old, &current) {
            return Err("the current password is wrong".into());
        }
        check_password(new)?;
        let hashed = hash_password(new)?;
        self.change(|k| {
            if let Some(u) = k.users.iter_mut().find(|u| u.id == user) {
                u.password = hashed;
            }
            k.sessions.retain(|s| s.user != user || s.hash == keep);
            Ok(())
        })
    }
}

/// An Argon2id hash of a random password nobody has (made once), so a sign-in for an unknown
/// email takes as long as one for a known one.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| figcraft_collab_net::token::secret().and_then(|p| hash_password(&p)).unwrap_or_default())
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = figcraft_collab_net::token::secret()?;
    let salt = SaltString::encode_b64(salt.as_bytes()).map_err(|e| e.to_string())?;
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| format!("couldn't keep the password: {e}"))
}

fn verify(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

fn check_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_PASSWORD {
        return Err(format!("choose a password of at least {MIN_PASSWORD} characters"));
    }
    if n > MAX_PASSWORD {
        return Err(format!("choose a password of at most {MAX_PASSWORD} characters"));
    }
    Ok(())
}

/// `email` in lower case, when it looks like one.
fn check_email(email: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    let ok = email.len() <= MAX_EMAIL
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.starts_with('.') && !b.ends_with('.'));
    if ok { Ok(email) } else { Err("give an email address such as name@example.com".into()) }
}

/// A person's name: one line, no control characters, at most [`MAX_NAME`] characters.
fn clean_name(name: &str) -> String {
    let one: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_NAME).collect()
}
