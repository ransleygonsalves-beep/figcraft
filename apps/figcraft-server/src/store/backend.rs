use super::journal::flush_log;
use super::team::team_admits;
use super::{
    ACCOUNT_TOKEN, Access, AccountAuth, AccountHandled, AccountReply, AccountReq, Action, Admin, AdminReply, Admission, BUSY_ADMIN, BUSY_DOWNLOADS,
    Backend, ClientMsg, Closing, Handled, HostInfo, InviteInfo, IpAddr, LibraryReply, LibraryReq, OpenFile, OpenedAccount, PluginReply, PluginReq,
    Server, Store, disk, hash, now, same_secret,
};

/// What a used, withdrawn or made-up invite code says.
const INVITE_GONE: &str = "this invite doesn't work: it was used, withdrawn or is too old; ask for a new one";

/// Whether an account request changes something (counted against the address's budget).
fn account_changes(req: &AccountReq) -> bool {
    !matches!(req, AccountReq::Me | AccountReq::Files | AccountReq::Members { .. } | AccountReq::Invites | AccountReq::Users)
}

/// Whether `req` changes the server's files (a write, a removal or a join code's stretching):
/// such requests count against the address's budget ([`Action::Admin`]); looking doesn't.
fn changes_files(req: &Admin) -> bool {
    !matches!(
        req,
        Admin::Files
            | Admin::Links { .. }
            | Admin::Listing
            | Admin::Reports
            | Admin::Blocked
            | Admin::Owned { .. }
            | Admin::RemovePeer { .. }
            | Admin::Team
            | Admin::CreateInvite { .. }
    )
}

impl Store {
    /// Revoke link `link` of `file`, and the file's team links once it isn't shared any more:
    /// whoever joined with any of them is disconnected.
    fn revoke_all(&mut self, file: String, link: String) -> Handled {
        match self.revoke(&file, &link) {
            Ok(()) => {
                let mut revoked = vec![(file.clone(), link)];
                revoked.extend(self.team_unshared(&file).into_iter().map(|l| (file.clone(), l)));
                Handled { reply: AdminReply::Done, revoked, removed: None, blocked: None }
            }
            Err(reason) => AdminReply::Failed { reason }.into(),
        }
    }
}

impl Backend for Store {
    fn admit(&mut self, file: &str, token: &str) -> Result<Admission, String> {
        if let Some(session) = token.strip_prefix(ACCOUNT_TOKEN) {
            let (_, user) = self.account_session(session).ok_or("you were signed out of this server: sign in again")?;
            let access = self.access_of(&user, file).ok_or("this file isn't shared with your account: ask its owner")?;
            return Ok(Admission { access, link: None, account: Some((user.id, user.name)) });
        }
        if self.accounts_only() {
            return Err("this server lets people in with their accounts only: sign in to open its files".into());
        }
        let team = self.cfg.hosting.team;
        let entry = self.entry(file)?;
        let link = Self::link_of(entry, token).filter(|&i| team_admits(team, entry, i)).and_then(|i| entry.links.get(i));
        link.map(|l| Admission { access: l.access, link: Some(l.id.clone()), account: None })
            .ok_or_else(|| "this link isn't valid (it may have been revoked)".to_string())
    }

    fn open(&mut self, file: &str) -> Result<Server, String> {
        self.load(file).map_err(|e| {
            let why = format!("file {file} can't be opened: {e}");
            log::error!("{why}");
            self.note(why);
            "this file can't be opened on the server (its data is damaged); tell the server's owner".to_string()
        })
    }

    fn changed(&mut self, file: &str, server: &mut Server) {
        let journal = server.take_journal();
        if journal.is_empty() {
            return;
        }
        let (after, max_bytes) = (self.cfg.compact_after, self.cfg.compact_bytes);
        let Some(e) = self.files.get_mut(file) else { return };
        let Some(log) = e.log.as_mut() else { return };
        for j in &journal {
            match disk::record(j) {
                Ok(r) => log.pending.extend(r),
                Err(err) => log::error!("file {file}: a change couldn't be logged: {err}"),
            }
        }
        // A refusal changes nothing in the document: it counts toward the log's size (bytes), but
        // not toward the entries that make a snapshot rewrite worthwhile.
        log.entries += journal.iter().filter(|j| !matches!(j, figcraft_collab::Journal::Refused { .. })).count();
        log.dirty = true;
        e.meta.updated = now();
        if log.entries >= after || log.bytes + log.pending.len() as u64 >= max_bytes {
            // The snapshot holds what's pending too.
            self.compact(file, server);
        }
    }

    fn flush(&mut self) -> Vec<String> {
        self.flush_all()
    }

    fn room(&mut self, file: &str, server: &Server, bytes: usize) -> Result<(), String> {
        self.room_for(file, server, bytes as u64)
    }

    fn idle(&mut self, file: &str, server: &mut Server) -> bool {
        let mut synced = false;
        let compact = self.files.get_mut(file).is_some_and(|entry| {
            let snapshot_seq = entry.meta.seq;
            entry.log.as_mut().is_some_and(|log| {
                // Issued client ids and refusals are durable in the log; only edits need a
                // new image-bearing snapshot. Still try a snapshot if syncing the log failed.
                synced = flush_log(file, log).is_ok();
                !synced || (log.dirty && server.seq() > snapshot_seq)
            })
        });
        if compact {
            self.compact(file, server);
        }
        // It closes anyway, with its state in the snapshot and durable log.
        self.cut_short.retain(|f| f != file);
        let log = self.files.get_mut(file).and_then(|e| e.log.take());
        // Kept loaded a while, when its log was sound: a join soon after reads and rewrites
        // nothing (#1074). The hub drops its room as this returns: the session moves here.
        if let Some(log) = log.filter(|l| synced && l.pending.is_empty()) {
            let left = std::mem::replace(server, Server::new(figcraft_collab::Document::new(""), "", self.cfg.core));
            self.park(file, left, log);
        }
        // Everyone just left: idle time counts from now.
        self.touch(file);
        true
    }

    fn stop(&mut self, open: Vec<(&str, &mut Server)>) {
        self.write_usage(true);
        self.flush_all();
        // The open times not written yet (they are written at most hourly while files are used).
        let unwritten: Vec<String> = self.files.iter().filter(|(_, e)| e.meta.opened > e.opened_written).map(|(id, _)| id.clone()).collect();
        for file in unwritten {
            self.write_opened(&file);
        }
        for (file, server) in open {
            if self.files.get(file).and_then(|e| e.log.as_ref()).is_some_and(|l| l.dirty) {
                self.compact(file, server);
            }
        }
    }

    fn manage(&mut self, key: &str) -> Result<(), String> {
        // Both sides are SHA-256 digests, so the comparison sees equal lengths whatever was sent.
        match same_secret(&hash(key), &hash(&self.key)) {
            true => Ok(()),
            false if self.cfg.hosting.open => {
                Err("wrong server key (this server is open to the public: publish without a key, with an up-to-date FigCraft)".into())
            }
            false => Err("wrong server key".into()),
        }
    }

    fn connect(&mut self, ip: IpAddr) -> Result<(), String> {
        if now() >= self.next_sweep {
            self.sweep();
            self.write_usage(false);
        }
        if self.blocked.contains(&ip) {
            return Err("this server doesn't accept connections from your address".into());
        }
        if !self.abuse.allow(ip, Action::Connect, self.clock()) {
            return Err("too many connections from your address lately: wait a minute and try again".into());
        }
        Ok(())
    }

    fn message(&mut self, ip: IpAddr, msg: &ClientMsg) -> bool {
        // Cursors and cursor chat (many people behind one address each send ~30 a second while
        // they move) have a budget of their own, beside everything else.
        let what = if matches!(msg, ClientMsg::Presence(_) | ClientMsg::Signal(_) | ClientMsg::Ping) { Action::Presence } else { Action::Message };
        self.abuse.allow(ip, what, self.clock())
    }

    fn hosting(&mut self, file: &str) -> Option<HostInfo> {
        Some(self.host_info(Some(file)))
    }

    fn open_hosting(&mut self, _ip: IpAddr, file: &str, owner: &str) -> Result<HostInfo, String> {
        if file.is_empty() && owner.is_empty() {
            return Ok(self.host_info(None));
        }
        if !self.owns(file, owner) {
            return Err("that owner token doesn't manage a file of that id here (it may have been removed or expired)".into());
        }
        Ok(self.host_info(Some(file)))
    }

    fn hosted(&mut self, ip: IpAddr, owns: Option<&str>, req: Admin, open: &dyn Fn(&str) -> Option<OpenFile>) -> Handled {
        let failed = |reason: &str| Handled::from(AdminReply::Failed { reason: reason.to_string() });
        let file = match &req {
            Admin::Publish { name, snapshot } => {
                return match self.publish_open(ip, name, snapshot) {
                    Ok((file, edit, view, owner)) => {
                        let expires = self.files.get(&file.id).and_then(|e| self.expires(&e.meta));
                        AdminReply::Hosted { file, edit, view, owner, expires }.into()
                    }
                    Err(e) => {
                        log::info!("event=publish_refused ip={ip} reason={e:?}");
                        Handled::from(AdminReply::Failed { reason: e })
                    }
                };
            }
            Admin::Owned { files } => return self.owned(files).into(),
            // Anyone who reaches the server, when its operator turned the team list on.
            Admin::Team => return self.team(open).into(),
            Admin::Links { file }
            | Admin::CreateLink { file, .. }
            | Admin::Revoke { file, .. }
            | Admin::Rename { file, .. }
            | Admin::Code { file, .. }
            | Admin::Remove { file } => file,
            _ => return failed("that needs the server key"),
        };
        if owns != Some(file.as_str()) {
            return failed("an owner token manages only its own file");
        }
        self.admin(ip, req, open)
    }

    fn report(&mut self, file: &str, link: Option<&str>, ip: IpAddr, reason: &str) -> Result<(), String> {
        self.keep_report(file, link, ip, reason)
    }

    fn admin(&mut self, ip: IpAddr, req: Admin, open: &dyn Fn(&str) -> Option<OpenFile>) -> Handled {
        let failed = |reason: String| Handled::from(AdminReply::Failed { reason });
        if changes_files(&req) && !self.abuse.allow(ip, Action::Admin, self.clock()) {
            log::info!("event=refused ip={ip} reason=admin_per_address");
            return failed(BUSY_ADMIN.into());
        }
        match req {
            Admin::Publish { name, snapshot } => match self.publish(&name, &snapshot) {
                Ok((file, edit, view)) => AdminReply::Published { file, edit, view }.into(),
                Err(e) => failed(e),
            },
            Admin::Files => AdminReply::Files(self.list(open)).into(),
            Admin::Links { file } => self.links(&file).map_or_else(failed, |l| AdminReply::Links(l).into()),
            Admin::CreateLink { file, access, label } => self.create_link(&file, access, &label).map_or_else(failed, |l| AdminReply::Link(l).into()),
            Admin::Revoke { file, link } => self.revoke_all(file, link),
            Admin::Rename { file, name } => self.rename(&file, &name).map_or_else(failed, |()| AdminReply::Done.into()),
            Admin::Listing => AdminReply::Listing(self.listing(open)).into(),
            Admin::Reports => AdminReply::Reports(self.reports.clone()).into(),
            Admin::Block { ip } => match self.block(&ip, true) {
                Ok(ip) => Handled { reply: AdminReply::Done, revoked: Vec::new(), removed: None, blocked: Some(ip) },
                Err(e) => failed(e),
            },
            Admin::Unblock { ip } => self.block(&ip, false).map_or_else(failed, |_| AdminReply::Done.into()),
            Admin::Blocked => AdminReply::Blocked(self.blocked()).into(),
            Admin::Code { file, token } => self.set_code(&file, &token).map_or_else(failed, |code| AdminReply::Code { code }.into()),
            Admin::Owned { files } => self.owned(&files).into(),
            Admin::Team => self.team(open).into(),
            Admin::CreateInvite { admin } => {
                self.accounts.create_invite(admin).map_or_else(failed, |(id, code)| AdminReply::Invite { id, code }.into())
            }
            // The hub removes people itself (it knows who is connected); it never asks the store.
            Admin::RemovePeer { .. } => failed("removing someone from a file is done by the server's hub".into()),
            Admin::Remove { file } => {
                if !self.files.contains_key(&file) {
                    return failed("this file isn't on the server (it may have been removed)".into());
                }
                let reply = match self.remove(&file) {
                    Ok(()) => AdminReply::Done,
                    Err(reason) => AdminReply::Failed { reason },
                };
                Handled { reply, revoked: Vec::new(), removed: Some(file), blocked: None }
            }
        }
    }

    fn library(&mut self, ip: IpAddr, file: &str, access: Access, req: LibraryReq) -> LibraryReply {
        let failed = |reason: String| LibraryReply::Failed { reason };
        match req {
            LibraryReq::Fetch { have, usage } => {
                if let Some(u) = usage
                    && let Err(e) = self.report_usage(file, u)
                {
                    log::warn!("file {file}: a library usage report couldn't be kept: {e}");
                }
                self.fetch_library(ip, file, have).unwrap_or_else(failed)
            }
            LibraryReq::Publish { .. } if access != Access::Edit => {
                failed("you can only view this file: publishing its library needs an edit link".into())
            }
            LibraryReq::Publish { archive } => {
                self.publish_library(file, &archive).map_or_else(failed, |version| LibraryReply::Published { version })
            }
            LibraryReq::Usage => self.library_usage(file).map_or_else(failed, LibraryReply::Usage),
            LibraryReq::ReportDetached(d) => self.report_detached(file, d).map_or_else(failed, |()| LibraryReply::Noted),
            LibraryReq::Detached => self.library_detached(file).map_or_else(failed, LibraryReply::Detached),
            LibraryReq::ReportInserted(d) => self.report_inserted(file, d).map_or_else(failed, |()| LibraryReply::Noted),
            LibraryReq::Actions => self.library_actions(file).map_or_else(failed, LibraryReply::Actions),
            LibraryReq::ReportAssets(a) => self.report_assets(file, a).map_or_else(failed, |()| LibraryReply::Noted),
            LibraryReq::Assets => self.library_assets(file).map_or_else(failed, LibraryReply::Assets),
            LibraryReq::ReportAssetActions(a) => self.report_asset_actions(file, a).map_or_else(failed, |()| LibraryReply::Noted),
            LibraryReq::AssetActions => self.library_asset_actions(file).map_or_else(failed, LibraryReply::AssetActions),
        }
    }

    fn plugins(&mut self, ip: IpAddr, publisher: bool, req: PluginReq) -> PluginReply {
        // One address's share first (before everyone's budget is spent or anything is read).
        if let PluginReq::Fetch { id, version } = &req
            && let Some(size) = self.directory.size_of(id, version.as_deref())
            && !self.abuse.allow(ip, Action::Download(usize::try_from(size).unwrap_or(usize::MAX)), self.clock())
        {
            return PluginReply::Failed { reason: BUSY_DOWNLOADS.into() };
        }
        self.directory.handle(publisher, req)
    }

    fn resolve(&mut self, ip: IpAddr, code: &str) -> Result<(String, String), String> {
        self.resolve_code(ip, code)
    }

    fn resolve_team(&mut self, _ip: IpAddr, file: &str) -> Result<String, String> {
        self.team_link(file).map(|l| l.token)
    }

    fn open_account(&mut self, ip: IpAddr, auth: AccountAuth) -> Result<OpenedAccount, String> {
        let signed = match auth {
            AccountAuth::Session { token } => {
                let (session, user) = self.account_session(&token).ok_or("you were signed out of this server: sign in again")?;
                return Ok(OpenedAccount { user: user.id, session, reply: AccountReply::SignedIn { token, user } });
            }
            AccountAuth::Password { email, password } => self.guessed(ip, |s| s.accounts.sign_in(&email, &password))?,
            AccountAuth::Invite { code, email, name, password } => {
                // A wrong code is counted as a wrong guess; a right one with details to fix isn't.
                let open = self.guessed(ip, |s| if s.accounts.invite_open(&code) { Ok(()) } else { Err(INVITE_GONE.into()) });
                open?;
                self.accounts.redeem(&code, &email, &name, &password)?
            }
        };
        log::info!("event=signed_in user={} ip={ip}", signed.user.id);
        Ok(OpenedAccount { user: signed.user.id, session: signed.session, reply: AccountReply::SignedIn { token: signed.token, user: signed.user } })
    }

    fn account(&mut self, ip: IpAddr, user: u64, session: &str, req: AccountReq, _open: &dyn Fn(&str) -> Option<OpenFile>) -> AccountHandled {
        let Some(me) = self.accounts.user(user).filter(|u| !u.disabled) else {
            return AccountHandled { signed_out: vec![user], ..AccountReply::Failed { reason: "you were signed out of this server".into() }.into() };
        };
        let failed = |reason: String| AccountHandled::from(AccountReply::Failed { reason });
        let admin_only = || failed("only an admin can do that".into());
        if account_changes(&req) && !self.abuse.allow(ip, Action::Admin, self.clock()) {
            return failed(BUSY_ADMIN.into());
        }
        match req {
            AccountReq::Me => AccountReply::Me(me).into(),
            AccountReq::SignOut => {
                self.accounts.sign_out(session).map_or_else(failed, |()| AccountHandled { signed_out: Vec::new(), ..AccountReply::Done.into() })
            }
            AccountReq::Files => AccountReply::Files(self.account_files(&me)).into(),
            AccountReq::Members { file } => self.members(&me, &file).map_or_else(failed, |m| AccountReply::Members(m).into()),
            AccountReq::SetMember { file, email, access } => match self.set_member(&me, &file, &email, access) {
                Ok(()) => AccountHandled { members_changed: vec![file], ..AccountReply::Done.into() },
                Err(e) => failed(e),
            },
            AccountReq::Publish { name, snapshot } => {
                self.publish_account(&me, &name, &snapshot).map_or_else(failed, |file| AccountReply::Published { file }.into())
            }
            AccountReq::RenameFile { file, name } => match self.account_manages(&me, &file) {
                Ok(()) => self.rename(&file, &name).map_or_else(failed, |()| AccountReply::Done.into()),
                Err(e) => failed(e),
            },
            AccountReq::RemoveFile { file } => match self.account_manages(&me, &file).and_then(|()| self.remove(&file)) {
                Ok(()) => AccountHandled { removed: Some(file), ..AccountReply::Done.into() },
                Err(e) => failed(e),
            },
            AccountReq::ChangePassword { old, new } => {
                let others = self.accounts.change_password(user, session, &old, &new);
                others.map_or_else(failed, |()| AccountHandled { signed_out: Vec::new(), ..AccountReply::Done.into() })
            }
            _ if !me.admin => admin_only(),
            AccountReq::CreateInvite { admin } => {
                self.accounts.create_invite(admin).map_or_else(failed, |(id, code)| AccountReply::Invite { id, code }.into())
            }
            AccountReq::Invites => {
                let shown = |(id, admin, created, expires): (String, bool, u64, u64)| InviteInfo {
                    id,
                    admin,
                    created: i64::try_from(created).unwrap_or(i64::MAX),
                    expires: i64::try_from(expires).unwrap_or(i64::MAX),
                };
                AccountReply::Invites(self.accounts.invites().into_iter().map(shown).collect()).into()
            }
            AccountReq::RevokeInvite { id } => self.accounts.revoke_invite(&id).map_or_else(failed, |()| AccountReply::Done.into()),
            AccountReq::Users => AccountReply::Users(self.accounts.users()).into(),
            AccountReq::SetUser { id, admin, disabled } => match self.accounts.set_user(user, id, admin, disabled) {
                // Whoever was disabled is disconnected; a changed role is checked again in every file.
                Ok(()) => {
                    let signed_out = if disabled == Some(true) { vec![id] } else { Vec::new() };
                    let members_changed = if admin.is_some() { self.files.keys().cloned().collect() } else { Vec::new() };
                    AccountHandled { signed_out, members_changed, ..AccountReply::Done.into() }
                }
                Err(e) => failed(e),
            },
        }
    }

    fn account_access(&mut self, user: u64, file: &str) -> Option<Access> {
        let user = self.accounts.user(user)?;
        self.access_of(&user, file)
    }

    fn closing(&self) -> Closing {
        Closing::Restarting
    }
}
