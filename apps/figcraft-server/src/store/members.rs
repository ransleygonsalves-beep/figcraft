//! Who may open a file with an account: its owner (the account that published it), the members
//! its owner or an admin added (each with edit, comment or view), and every admin. Kept in the
//! file's `meta.json`; the accounts themselves are in [`crate::accounts`].

use figcraft_collab::Change;
use figcraft_collab::message::{AccountFile, AccountUser, Member};

use super::{Access, META, Store, write_json};

/// What asking about a file one doesn't manage says.
const NOT_YOURS: &str = "only the file's owner or an admin can do that";

impl Store {
    /// What account `user` may do in `file` now, or `None` when it may not open it.
    pub fn access_of(&self, user: &AccountUser, file: &str) -> Option<Access> {
        let e = self.files.get(file)?;
        if user.disabled {
            return None;
        }
        if user.admin || e.meta.account_owner == Some(user.id) {
            return Some(Access::Edit);
        }
        e.meta.members.get(&user.id).copied()
    }

    /// Whether `user` manages `file` (its owner, or an admin).
    fn manages(&self, user: &AccountUser, file: &str) -> Result<(), String> {
        let e = self.files.get(file).ok_or("this file isn't on the server (it may have been removed)")?;
        if user.admin || e.meta.account_owner == Some(user.id) { Ok(()) } else { Err(NOT_YOURS.into()) }
    }

    /// The files `user` may open, most recently changed first.
    pub fn account_files(&self, user: &AccountUser) -> Vec<AccountFile> {
        let mut out: Vec<AccountFile> = self
            .files
            .iter()
            .filter_map(|(id, e)| {
                let access = self.access_of(user, id)?;
                let owner = e.meta.account_owner == Some(user.id);
                Some(AccountFile {
                    file: id.clone(),
                    name: e.meta.name.clone(),
                    access,
                    owner,
                    updated: i64::try_from(e.meta.updated).unwrap_or(i64::MAX),
                })
            })
            .collect();
        out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Who may open `file` (its owner first), as `user` asks.
    pub fn members(&self, user: &AccountUser, file: &str) -> Result<Vec<Member>, String> {
        self.manages(user, file)?;
        let e = self.files.get(file).ok_or("the file vanished")?;
        let shown =
            |id: u64, access: Access, owner: bool| self.accounts.user(id).map(|u| Member { user: u.id, email: u.email, name: u.name, access, owner });
        let owner = e.meta.account_owner.and_then(|id| shown(id, Access::Edit, true));
        Ok(owner.into_iter().chain(e.meta.members.iter().filter_map(|(id, a)| shown(*id, *a, false))).collect())
    }

    /// Let the account with `email` open `file` with `access`, or (`None`) no longer, as `user`
    /// asks.
    pub fn set_member(&mut self, user: &AccountUser, file: &str, email: &str, access: Option<Access>) -> Result<(), String> {
        self.manages(user, file)?;
        let target = self.accounts.by_email(email).ok_or("there is no account with that email: invite them to the server first")?;
        let path = self.dir(file).join(META);
        let entry = self.entry(file)?;
        if entry.meta.account_owner == Some(target.id) {
            return Err("that's the file's owner: they always have edit access".into());
        }
        let before = entry.meta.members.clone();
        match access {
            Some(a) => {
                entry.meta.members.insert(target.id, a);
            }
            None => {
                entry.meta.members.remove(&target.id);
            }
        }
        if let Err(e) = write_json(&path, &entry.meta) {
            entry.meta.members = before;
            return Err(format!("couldn't change who may open the file: {e}"));
        }
        log::info!("event=member file={file} user={} access={}", target.id, access.map_or("none", Access::id));
        Ok(())
    }

    /// Keep a new file, owned by account `user`: its id.
    pub fn publish_account(&mut self, user: &AccountUser, name: &str, snapshot: &[Change]) -> Result<String, String> {
        let (info, ..) = self.publish(name, snapshot)?;
        let path = self.dir(&info.id).join(META);
        let entry = self.entry(&info.id)?;
        entry.meta.account_owner = Some(user.id);
        if let Err(e) = write_json(&path, &entry.meta) {
            let _ = self.remove(&info.id);
            return Err(format!("couldn't keep the file: {e}"));
        }
        log::info!("event=published file={} by=account user={}", info.id, user.id);
        Ok(info.id)
    }

    /// The account signed in with session token `token`: the session's id and who.
    pub fn account_session(&mut self, token: &str) -> Option<(String, AccountUser)> {
        self.accounts.session(token)
    }

    pub fn accounts(&mut self) -> &mut crate::accounts::Accounts {
        &mut self.accounts
    }

    /// Whether `user` may rename or remove `file` (its owner, or an admin).
    pub fn account_manages(&self, user: &AccountUser, file: &str) -> Result<(), String> {
        self.manages(user, file)
    }

    /// Run a password or invite check for `ip`, counted like a join code: too many wrong ones
    /// from an address lock it out for a while, and too many from everyone pause them for all.
    pub fn guessed<T>(&mut self, ip: std::net::IpAddr, check: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
        let now = std::time::Instant::now();
        if let Some(left) = self.abuse.code_locked(ip, now) {
            return Err(format!("too many wrong tries from your address: try again in {} minutes", left.as_secs() / 60 + 1));
        }
        if !self.abuse.allow(ip, crate::abuse::Action::Code, now) {
            return Err("too many tries from your address: wait a minute and try again".into());
        }
        if !self.abuse.codes_open(now) {
            return Err("sign-in is paused on this server for a while (too many wrong tries): try again later".into());
        }
        let out = check(self);
        self.abuse.code_tried(ip, out.is_ok(), now);
        out
    }

    /// Whether the server lets people in with their accounts only (share links are refused).
    pub fn accounts_only(&self) -> bool {
        self.cfg.hosting.accounts_only
    }
}
