//! Home ▸ Online ▸ Your team's server: signing in to the server in Preferences with an account
//! (or making one with an admin's invite), the files shared with you (a row opens one under your
//! account's name), sharing a file you own with people by email, tokens for agents (an AI
//! assistant acting for you with limited access), and, for admins, the accounts on the server,
//! invites and password resets. Every action runs an `account.*` command in the background; the
//! server's refusals show as they do for other commands.

use egui::{Id, RichText, Ui};
use figcraft_engine::online::{same_server, server_host};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, trf, trn};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets;

/// A field's width in the forms.
const FIELD_W: f32 = 280.0;
/// The access levels a member can have, as `account.share` names them.
const LEVELS: [&str; 3] = ["edit", "comment", "view"];

/// The sign-in form: with an invite code (making an account), with a reset code (choosing a new
/// password), or neither.
#[derive(Clone, Debug, Default)]
pub struct Form {
    pub invite: bool,
    pub reset: bool,
    pub server: String,
    pub email: String,
    pub password: String,
    pub code: String,
    pub name: String,
}

/// Who may open a file, as its owner or an admin manages them.
#[derive(Clone, Debug, Default)]
pub struct ShareDialog {
    pub file: String,
    pub name: String,
    pub members: Vec<Value>,
    pub email: String,
    pub access: String,
}

/// UI state of the account part of Home ▸ Online.
#[derive(Default)]
pub struct AccountUi {
    /// The files shared with you, as the server last said (`None`: not asked yet).
    pub files: Option<Vec<Value>>,
    pub form: Option<Form>,
    pub share: Option<ShareDialog>,
    /// The admin's People window: the accounts (`None` while asked).
    pub people: Option<Option<Vec<Value>>>,
    /// The last invite made: its code, and whether it makes an admin.
    pub invite: Option<(String, bool)>,
    /// The next invite makes an admin.
    pub invite_admin: bool,
    /// The last password reset code made: whose (name) and the code.
    pub reset: Option<(String, String)>,
    /// The Tokens for agents window.
    pub agents: Option<AgentsUi>,
    /// The admin window's tab (People, Invites, Files, Activity).
    pub admin_tab: usize,
    /// The People tab's search.
    pub search: String,
    /// The open invites (`None` while asked).
    pub invites: Option<Vec<Value>>,
    /// The server's latest audit events (`None` while asked).
    pub activity: Option<Vec<Value>>,
    /// A file being renamed in the Files tab: its id and the name typed.
    pub renaming: Option<(String, String)>,
    /// A file waiting for its deletion to be confirmed.
    pub deleting: Option<String>,
    /// The Change password window.
    pub password: Option<PasswordForm>,
    /// The frame the section last drew in (it asks for the files again after a gap).
    drawn: Option<u64>,
    /// The server (and account) the files were last asked of: another one asks again.
    asked: Option<String>,
}

/// What the Change password window holds while it is open.
#[derive(Clone, Debug, Default)]
pub struct PasswordForm {
    pub old: String,
    pub new: String,
    pub again: String,
}

/// The Tokens for agents window: your tokens (`None` while asked), the next one's label and
/// access, and the token just made (shown once).
#[derive(Clone, Debug, Default)]
pub struct AgentsUi {
    pub list: Option<Vec<Value>>,
    pub label: String,
    pub access: String,
    pub made: Option<String>,
}

/// The server the account part is about: the one in Preferences.
fn server(app: &FigApp) -> String {
    app.session.prefs.publish_server.trim().to_string()
}

/// Signed in to `server`: (name, email, admin).
fn signed_in(app: &FigApp, server: &str) -> Option<(String, String, bool)> {
    app.session.prefs.accounts.iter().find(|a| same_server(&a.server, server)).map(|a| (a.name.clone(), a.email.clone(), a.admin))
}

fn refresh(app: &mut FigApp, ctx: &egui::Context) {
    let server = server(app);
    if server.is_empty() || app.session.waiting_for("account.files") {
        return;
    }
    let Some((_, email, _)) = signed_in(app, &server) else { return };
    app.ui.account.asked = Some(format!("{server} {email}"));
    app.run_background(
        ctx,
        "account.files",
        json!({"server": server}),
        Box::new(|app, _, v| app.ui.account.files = Some(v["files"].as_array().cloned().unwrap_or_default())),
    );
}

/// The account part of the Online section.
pub fn section(app: &mut FigApp, ui: &mut Ui, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let frame = ctx.cumulative_frame_nr();
    let server = server(app);
    let now_for = signed_in(app, &server).map(|(_, email, _)| format!("{server} {email}"));
    if app.ui.account.drawn.replace(frame).is_none_or(|d| d.saturating_add(1) < frame) || (now_for.is_some() && now_for != app.ui.account.asked) {
        if now_for != app.ui.account.asked {
            app.ui.account.files = None;
        }
        refresh(app, &ctx);
    }
    ui.label(RichText::new(tr("Your team's server")).font(semibold(14.0)).color(t.text));
    if server.is_empty() {
        widgets::label(ui, tr("Choose your team's server in Preferences, under Server for online sharing, then sign in here."), t.text_secondary);
        return;
    }
    match signed_in(app, &server) {
        None => {
            widgets::label(ui, &trf("Sign in to {server} to see the files shared with you.", &[("server", &server_host(&server))]), t.text_secondary);
            ui.horizontal(|ui| {
                if widgets::button(ui, tr("Sign in"), true, true).clicked() {
                    app.ui.account.form = Some(Form { server: server.clone(), ..Form::default() });
                }
                if widgets::button(ui, tr("I have an invite"), false, true).clicked() {
                    app.ui.account.form = Some(Form { invite: true, server: server.clone(), ..Form::default() });
                }
                if widgets::button(ui, tr("I have a reset code"), false, true).clicked() {
                    app.ui.account.form = Some(Form { reset: true, server: server.clone(), ..Form::default() });
                }
            });
        }
        Some((name, email, admin)) => signed_in_ui(app, ui, t, &server, (&name, &email, admin)),
    }
    form(app, &ctx, t);
    share_dialog(app, &ctx, t);
    people(app, &ctx, t);
    agents(app, &ctx, t);
    password(app, &ctx, t);
}

fn signed_in_ui(app: &mut FigApp, ui: &mut Ui, t: &Tokens, server: &str, (name, email, admin): (&str, &str, bool)) {
    let ctx = ui.ctx().clone();
    let shown = trf("Signed in to {server} as {name} ({email})", &[("server", &server_host(server)), ("name", &name), ("email", &email)]);
    widgets::label(ui, &shown, t.text_secondary);
    ui.horizontal(|ui| {
        if widgets::button(ui, tr("Refresh"), false, true).clicked() {
            refresh(app, &ctx);
        }
        if admin && widgets::button(ui, tr("Admin"), false, true).clicked() {
            app.ui.account.people = Some(None);
            app.ui.account.admin_tab = 0;
            ask_people(app, &ctx);
        }
        if widgets::button(ui, tr("Change password"), false, true).clicked() {
            app.ui.account.password = Some(PasswordForm::default());
        }
        if widgets::button(ui, tr("Tokens for agents"), false, true).clicked() {
            app.ui.account.agents = Some(AgentsUi { access: "edit".into(), ..AgentsUi::default() });
            ask_agents(app, &ctx);
        }
        if widgets::button(ui, tr("Sign out"), false, true).clicked() {
            app.run_background(&ctx, "account.signOut", json!({"server": server}), Box::new(|app, _, _| app.ui.account.files = None));
        }
    });
    ui.add_space(4.0);
    let files = app.ui.account.files.clone();
    match files {
        None => widgets::label(ui, tr("Asking the server…"), t.text_secondary),
        Some(files) if files.is_empty() => {
            widgets::label(ui, tr("Nothing is shared with you yet. Ask a file's owner to share it with your email."), t.text_secondary)
        }
        Some(files) => {
            for f in &files {
                file_row(app, ui, t, server, f);
            }
        }
    }
}

/// What a level of access is called.
fn level(access: &str) -> String {
    match access {
        "edit" => tr("Can edit").to_string(),
        "comment" => tr("Can comment").to_string(),
        _ => tr("Can view").to_string(),
    }
}

/// One file shared with you: its name and what you may do; Open, and Share for its owner or an admin.
fn file_row(app: &mut FigApp, ui: &mut Ui, t: &Tokens, server: &str, f: &Value) {
    let (file, name) = (f["file"].as_str().unwrap_or_default().to_string(), f["name"].as_str().unwrap_or_default().to_string());
    let owner = f["owner"] == true;
    let admin = signed_in(app, server).is_some_and(|(_, _, a)| a);
    let what = if owner { tr("Yours").to_string() } else { level(f["access"].as_str().unwrap_or_default()) };
    ui.horizontal(|ui| {
        ui.label(RichText::new(&name).color(t.text));
        ui.label(RichText::new(what).font(regular(11.0)).color(t.text_secondary));
        if widgets::button(ui, &trf("Open {file}", &[("file", &name)]), false, true).clicked()
            && let Some(r) = app.run("account.open", json!({"server": server, "file": file}))
        {
            app.ui.live.home_join = r["session"].as_u64();
        }
        if (owner || admin) && widgets::button(ui, &trf("Share {file}", &[("file", &name)]), false, true).clicked() {
            app.ui.account.share = Some(ShareDialog { file: file.clone(), name: name.clone(), access: "edit".into(), ..ShareDialog::default() });
            ask_members(app, &ui.ctx().clone(), &file);
        }
    });
}

fn ask_members(app: &mut FigApp, ctx: &egui::Context, file: &str) {
    let params = json!({"server": server(app), "file": file});
    app.run_background(
        ctx,
        "account.members",
        params,
        Box::new(|app, _, v| {
            if let Some(d) = app.ui.account.share.as_mut() {
                d.members = v["members"].as_array().cloned().unwrap_or_default();
            }
        }),
    );
}

fn ask_agents(app: &mut FigApp, ctx: &egui::Context) {
    let params = json!({"server": server(app)});
    app.run_background(
        ctx,
        "account.agentTokens",
        params,
        Box::new(|app, _, v| {
            if let Some(a) = app.ui.account.agents.as_mut() {
                a.list = Some(v["tokens"].as_array().cloned().unwrap_or_default());
            }
        }),
    );
}

/// Days from now until `expires` (Unix seconds), at least 0.
fn days_left(expires: i64) -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    (expires - now).max(0) / 86_400
}

/// The Tokens for agents window: what a token lets an agent do, yours with End, and making one.
fn agents(app: &mut FigApp, ctx: &egui::Context, t: &Tokens) {
    let Some(mut a) = app.ui.account.agents.take() else { return };
    let mut open = true;
    let (mut end, mut make) = (None, false);
    crate::dialogs::window(tr("Tokens for agents"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(FIELD_W + 160.0);
        widgets::label(
            ui,
            tr("A token lets an AI agent, such as Claude, work in your files for you: it opens them with at most the access you choose, shows as acting for you, can't share, manage people or delete, and ends after 30 days."),
            t.text_secondary,
        );
        ui.add_space(6.0);
        match &a.list {
            None => widgets::label(ui, tr("Asking the server…"), t.text_secondary),
            Some(list) if list.is_empty() => widgets::label(ui, tr("You have no tokens for agents."), t.text_secondary),
            Some(list) => {
                for tok in list {
                    let (id, label) = (tok["id"].as_str().unwrap_or_default(), tok["label"].as_str().unwrap_or_default());
                    let ends = trf("{access} · ends in {days} days", &[
                        ("access", &level(tok["access"].as_str().unwrap_or_default())),
                        ("days", &days_left(tok["expires"].as_i64().unwrap_or(0)).to_string()),
                    ]);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).color(t.text));
                        ui.label(RichText::new(ends).font(regular(11.0)).color(t.text_secondary));
                        if widgets::button(ui, &trf("End {name}", &[("name", &label)]), false, true).clicked() {
                            end = Some(id.to_string());
                        }
                    });
                }
            }
        }
        ui.add_space(8.0);
        widgets::separator(ui);
        ui.add_space(6.0);
        widgets::label(ui, tr("Make a token"), t.text);
        ui.horizontal(|ui| {
            if let Some(v) = widgets::text_field_named(ui, Id::unique("agent_label"), &a.label, FIELD_W - 60.0, tr("Name (Claude)")) {
                a.label = v;
            }
            let options: Vec<String> = LEVELS.iter().map(|l| level(l)).collect();
            if let Some(picked) = widgets::combo_field(ui, Id::unique("agent_access"), &level(&a.access), 130.0, &options)
                && let Some(i) = options.iter().position(|o| *o == picked)
            {
                a.access = LEVELS[i].to_string();
            }
        });
        make = widgets::button(ui, tr("Make a token"), true, true).clicked();
        if let Some(token) = &a.made {
            ui.add_space(6.0);
            widgets::label(ui, tr("Copy it now: it isn't shown again. Give it to your agent as FIGCRAFT_TOKEN, with this server as FIGCRAFT_SERVER."), t.text_secondary);
            ui.horizontal(|ui| {
                if widgets::button(ui, tr("Copy token"), true, true).clicked() {
                    ui.ctx().copy_text(token.clone());
                }
            });
        }
        ui.add_space(6.0);
        widgets::label(
            ui,
            tr("On the computer the agent runs on, figcraft-cli account login makes one and keeps it for figcraft-cli mcp, without showing it."),
            t.text_secondary,
        );
    });
    if let Some(id) = end {
        app.run_background(ctx, "account.revokeAgentToken", json!({"server": server(app), "id": id}), Box::new(|app, ctx, _| ask_agents(app, ctx)));
    }
    if make {
        let label = if a.label.trim().is_empty() { "Claude".to_string() } else { a.label.trim().to_string() };
        let params = json!({"server": server(app), "access": a.access, "label": label});
        app.run_background(
            ctx,
            "account.agentToken",
            params,
            Box::new(|app, ctx, v| {
                if let Some(a) = app.ui.account.agents.as_mut() {
                    a.made = v["token"].as_str().map(str::to_string);
                }
                ask_agents(app, ctx);
            }),
        );
    }
    if open {
        app.ui.account.agents = Some(a);
    }
}

fn ask_people(app: &mut FigApp, ctx: &egui::Context) {
    let params = json!({"server": server(app)});
    app.run_background(ctx, "account.users", params, Box::new(|app, _, v| app.ui.account.people = Some(v["users"].as_array().cloned())));
}

/// The sign-in (or make an account) window.
fn form(app: &mut FigApp, ctx: &egui::Context, t: &Tokens) {
    let Some(mut f) = app.ui.account.form.take() else { return };
    let mut open = true;
    let mut submit = false;
    let title = if f.reset {
        tr("Choose a new password")
    } else if f.invite {
        tr("Make your account")
    } else {
        tr("Sign in")
    };
    crate::dialogs::window(title, ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(FIELD_W + 20.0);
        let field = |ui: &mut Ui, label: &str, value: &mut String, secret: bool| {
            widgets::label(ui, label, t.text_secondary);
            let id = Id::unique(("account_form", label));
            let changed = if secret {
                widgets::secret_field_named(ui, id, value, FIELD_W, label)
            } else {
                widgets::text_field_named(ui, id, value, FIELD_W, label)
            };
            if let Some(v) = changed {
                *value = v;
            }
        };
        field(ui, tr("Server"), &mut f.server, false);
        if f.reset {
            field(ui, tr("Reset code"), &mut f.code, false);
        } else if f.invite {
            field(ui, tr("Invite code"), &mut f.code, false);
            field(ui, tr("Your name"), &mut f.name, false);
        }
        if !f.reset {
            field(ui, tr("Email"), &mut f.email, false);
        }
        let new_password = f.invite || f.reset;
        field(ui, if new_password { tr("Choose a password (at least 8 characters)") } else { tr("Password") }, &mut f.password, true);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = !f.server.trim().is_empty()
                && !f.password.is_empty()
                && (f.reset || !f.email.trim().is_empty())
                && (!(f.invite || f.reset) || !f.code.trim().is_empty());
            let go = if f.reset {
                tr("Set password")
            } else if f.invite {
                tr("Make account")
            } else {
                tr("Sign in")
            };
            submit = widgets::button(ui, go, true, ready).clicked();
            let other = if f.invite || f.reset { tr("I already have an account") } else { tr("I have an invite") };
            if widgets::button(ui, other, false, true).clicked() {
                f.invite = !(f.invite || f.reset);
                f.reset = false;
            }
        });
        ui.label(
            RichText::new(tr("Your password goes only to this server; this computer keeps a sign-in for it, not the password."))
                .font(regular(11.0))
                .color(t.text_secondary),
        );
    });
    if submit {
        let (cmd, params) = if f.reset {
            ("account.reset", json!({"server": f.server.trim(), "code": f.code.trim(), "password": f.password}))
        } else if f.invite {
            (
                "account.join",
                json!({"server": f.server.trim(), "code": f.code.trim(), "email": f.email.trim(), "name": f.name.trim(), "password": f.password}),
            )
        } else {
            ("account.signIn", json!({"server": f.server.trim(), "email": f.email.trim(), "password": f.password}))
        };
        // The server in Preferences becomes this one, so the section shows what you signed in to.
        let server = f.server.trim().to_string();
        app.run("pref.publishServer", json!({"url": server}));
        app.run_background(
            ctx,
            cmd,
            params,
            Box::new(|app, ctx, _| {
                app.ui.account.form = None;
                app.ui.account.files = None;
                refresh(app, ctx);
            }),
        );
        // The form stays (its password cleared) until the server says yes; a refusal shows why.
        f.password.clear();
    }
    if open {
        app.ui.account.form = Some(f);
    }
}

/// Who may open a file, with what; add people by email.
fn share_dialog(app: &mut FigApp, ctx: &egui::Context, t: &Tokens) {
    let Some(mut d) = app.ui.account.share.take() else { return };
    let mut open = true;
    let mut change: Option<(String, String)> = None;
    crate::dialogs::window(&trf("Share {file}", &[("file", &d.name)]), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(FIELD_W + 160.0);
        widgets::label(ui, tr("People with access"), t.text);
        let options: Vec<String> = LEVELS.iter().map(|l| level(l)).collect();
        for m in &d.members {
            let (email, name) = (m["email"].as_str().unwrap_or_default(), m["name"].as_str().unwrap_or_default());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{name} ({email})")).color(t.text));
                if m["owner"] == true {
                    ui.label(RichText::new(tr("Owner")).color(t.text_secondary));
                    return;
                }
                let current = level(m["access"].as_str().unwrap_or_default());
                if let Some(picked) = widgets::combo_field(ui, Id::unique(("member_access", email)), &current, 130.0, &options)
                    && let Some(i) = options.iter().position(|o| *o == picked)
                {
                    change = Some((email.to_string(), LEVELS[i].to_string()));
                }
                if widgets::button(ui, &trf("Remove {name}", &[("name", &name)]), false, true).clicked() {
                    change = Some((email.to_string(), "none".into()));
                }
            });
        }
        ui.add_space(8.0);
        widgets::label(ui, tr("Add someone by the email of their account"), t.text_secondary);
        ui.horizontal(|ui| {
            if let Some(v) = widgets::text_field_named(ui, Id::unique("share_email"), &d.email, FIELD_W - 60.0, tr("Email")) {
                d.email = v;
            }
            let current = level(&d.access);
            if let Some(picked) = widgets::combo_field(ui, Id::unique("share_access"), &current, 130.0, &options)
                && let Some(i) = options.iter().position(|o| *o == picked)
            {
                d.access = LEVELS[i].to_string();
            }
            if widgets::button(ui, tr("Add"), true, !d.email.trim().is_empty()).clicked() {
                change = Some((d.email.trim().to_string(), d.access.clone()));
            }
        });
        ui.label(
            RichText::new(tr("They need an account on this server first: an admin invites them from the Admin window's Invites tab."))
                .font(regular(11.0))
                .color(t.text_secondary),
        );
    });
    if let Some((email, access)) = change {
        let params = json!({"server": server(app), "file": d.file, "email": email, "access": access});
        app.run_background(
            ctx,
            "account.share",
            params,
            Box::new(|app, _, v| {
                if let Some(d) = app.ui.account.share.as_mut() {
                    d.members = v["members"].as_array().cloned().unwrap_or_default();
                    d.email.clear();
                }
            }),
        );
    }
    if open {
        app.ui.account.share = Some(d);
    }
}

/// The admin panel's tabs.
const ADMIN_TABS: usize = 4;

/// The admin's window: People (find someone; make them an admin, disable them, reset their
/// password), Invites (make one; the open ones, to withdraw), Files (every file on the server:
/// open, share, rename, delete) and Activity (the server's audit log).
fn people(app: &mut FigApp, ctx: &egui::Context, t: &Tokens) {
    let Some(list) = app.ui.account.people.clone() else { return };
    let mut open = true;
    let mut act: Option<AdminAct> = None;
    let me = signed_in(app, &server(app)).map(|(_, email, _)| email).unwrap_or_default();
    let tabs = [tr("People"), tr("Invites"), tr("Files"), tr("Activity")];
    crate::dialogs::window(tr("Admin"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(FIELD_W + 240.0);
        if let Some(i) = widgets::segmented_text(ui, &tabs, app.ui.account.admin_tab.min(ADMIN_TABS - 1)) {
            act = Some(AdminAct::Tab(i));
        }
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height(440.0).auto_shrink([false, true]).show(ui, |ui| match app.ui.account.admin_tab {
            0 => people_tab(app, ui, t, list.as_deref(), &me, &mut act),
            1 => invites_tab(app, ui, t, &mut act),
            2 => files_tab(app, ui, t, &mut act),
            _ => activity_tab(app, ui, t, &mut act),
        });
    });
    if let Some(a) = act {
        admin_act(app, ctx, a);
    }
    if !open {
        let a = &mut app.ui.account;
        (a.people, a.invite, a.reset, a.renaming, a.deleting) = (None, None, None, None, None);
    }
}

/// What the admin window asks for.
enum AdminAct {
    Tab(usize),
    SetUser(Value),
    Reset(u64, String),
    Invite,
    Withdraw(String),
    Rename(String, String),
    Delete(String),
    Refresh,
}

fn admin_act(app: &mut FigApp, ctx: &egui::Context, a: AdminAct) {
    let server = server(app);
    match a {
        AdminAct::Tab(i) => {
            app.ui.account.admin_tab = i;
            match i {
                0 => ask_people(app, ctx),
                1 => ask_invites(app, ctx),
                2 => refresh(app, ctx),
                _ => ask_activity(app, ctx),
            }
        }
        AdminAct::Refresh => admin_act(app, ctx, AdminAct::Tab(app.ui.account.admin_tab)),
        AdminAct::SetUser(mut p) => {
            p["server"] = json!(server);
            app.run_background(ctx, "account.setUser", p, Box::new(|app, ctx, _| ask_people(app, ctx)));
        }
        AdminAct::Reset(id, name) => app.run_background(
            ctx,
            "account.resetPassword",
            json!({"server": server, "id": id}),
            Box::new(move |app, _, v| app.ui.account.reset = v["code"].as_str().map(|c| (name, c.to_string()))),
        ),
        AdminAct::Invite => {
            let params = json!({"server": server, "admin": app.ui.account.invite_admin});
            app.run_background(
                ctx,
                "account.invite",
                params,
                Box::new(|app, ctx, v| {
                    app.ui.account.invite = v["code"].as_str().map(|c| (c.to_string(), v["admin"] == true));
                    ask_invites(app, ctx);
                }),
            );
        }
        AdminAct::Withdraw(id) => {
            app.run_background(ctx, "account.revokeInvite", json!({"server": server, "id": id}), Box::new(|app, ctx, _| ask_invites(app, ctx)))
        }
        AdminAct::Rename(file, name) => {
            app.ui.account.renaming = None;
            app.run_background(
                ctx,
                "account.renameFile",
                json!({"server": server, "file": file, "name": name}),
                Box::new(|app, ctx, _| refresh(app, ctx)),
            );
        }
        AdminAct::Delete(file) => {
            app.ui.account.deleting = None;
            app.run_background(
                ctx,
                "account.removeFile",
                json!({"server": server, "file": file}),
                Box::new(|app, ctx, _| {
                    app.toast(tr("File deleted from the server"));
                    refresh(app, ctx);
                }),
            );
        }
    }
}

fn ask_invites(app: &mut FigApp, ctx: &egui::Context) {
    let params = json!({"server": server(app)});
    app.run_background(ctx, "account.invites", params, Box::new(|app, _, v| app.ui.account.invites = v["invites"].as_array().cloned()));
}

fn ask_activity(app: &mut FigApp, ctx: &egui::Context) {
    let params = json!({"server": server(app), "limit": 200});
    app.run_background(ctx, "account.activity", params, Box::new(|app, _, v| app.ui.account.activity = v["events"].as_array().cloned()));
}

/// A small, secondary line of text.
fn note(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(regular(11.0)).color(t.text_secondary));
}

fn people_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens, list: Option<&[Value]>, me: &str, act: &mut Option<AdminAct>) {
    let Some(users) = list else { return widgets::label(ui, tr("Asking the server…"), t.text_secondary) };
    if let Some(v) = widgets::text_field(ui, Id::unique("admin_search"), &app.ui.account.search, FIELD_W, tr("Search people")) {
        app.ui.account.search = v;
    }
    let q = app.ui.account.search.trim().to_lowercase();
    let found: Vec<&Value> = users
        .iter()
        .filter(|u| q.is_empty() || [&u["name"], &u["email"]].iter().any(|f| f.as_str().is_some_and(|s| s.to_lowercase().contains(&q))))
        .collect();
    note(ui, t, &trf("{shown} of {total} people", &[("shown", &found.len()), ("total", &users.len())]));
    ui.add_space(4.0);
    for u in found {
        let (id, name, email) = (u["id"].as_u64().unwrap_or(0), u["name"].as_str().unwrap_or_default(), u["email"].as_str().unwrap_or_default());
        let mut tags = Vec::new();
        if email == me {
            tags.push(tr("You"));
        }
        if u["admin"] == true {
            tags.push(tr("Admin"));
        }
        if u["disabled"] == true {
            tags.push(tr("Disabled"));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{name} ({email})")).color(t.text));
            if !tags.is_empty() {
                note(ui, t, &tags.join(" · "));
            }
        });
        if email == me {
            note(ui, t, tr("Your own account: another admin changes it."));
        } else {
            ui.horizontal(|ui| {
                let narrow = egui::vec2(120.0, 20.0);
                ui.allocate_ui(narrow, |ui| {
                    if widgets::checkbox(ui, u["admin"] == true, tr("Admin")) {
                        *act = Some(AdminAct::SetUser(json!({"id": id, "admin": u["admin"] != true})));
                    }
                });
                ui.allocate_ui(narrow, |ui| {
                    if widgets::checkbox(ui, u["disabled"] == true, tr("Disabled")) {
                        *act = Some(AdminAct::SetUser(json!({"id": id, "disabled": u["disabled"] != true})));
                    }
                });
                if widgets::button(ui, &trf("Reset password for {name}", &[("name", &name)]), false, true).clicked() {
                    *act = Some(AdminAct::Reset(id, name.to_string()));
                }
            });
        }
        ui.add_space(6.0);
    }
    if let Some((name, code)) = &app.ui.account.reset {
        widgets::separator(ui);
        ui.add_space(6.0);
        widgets::label(
            ui,
            &trf(
                "{name} was signed out everywhere. This code lets them choose a new password once, within a day. Send it to them:",
                &[("name", name)],
            ),
            t.text_secondary,
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new(code).monospace().color(t.text));
            if widgets::button(ui, tr("Copy"), false, true).clicked() {
                ui.ctx().copy_text(code.clone());
            }
        });
        widgets::label(ui, tr("In FigCraft they open Home, choose Online, then I have a reset code."), t.text_secondary);
    }
}

fn invites_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens, act: &mut Option<AdminAct>) {
    widgets::label(ui, tr("Invite someone"), t.text);
    if widgets::checkbox(ui, app.ui.account.invite_admin, tr("Make them an admin")) {
        app.ui.account.invite_admin = !app.ui.account.invite_admin;
    }
    if widgets::button(ui, tr("Make an invite code"), true, true).clicked() {
        *act = Some(AdminAct::Invite);
    }
    if let Some((code, admin)) = &app.ui.account.invite {
        ui.add_space(6.0);
        let kind = if *admin { tr("an admin account") } else { tr("an account") };
        widgets::label(ui, &trf("This code makes {kind} once, within 7 days. Send it to them:", &[("kind", &kind)]), t.text_secondary);
        ui.horizontal(|ui| {
            ui.label(RichText::new(code).monospace().color(t.text));
            if widgets::button(ui, tr("Copy"), false, true).clicked() {
                ui.ctx().copy_text(code.clone());
            }
        });
        let address = server(app);
        let host = server_host(&address);
        widgets::label(
            ui,
            &trf("In FigCraft they open Home, choose Online, then I have an invite, and enter the server {server}.", &[("server", &host)]),
            t.text_secondary,
        );
    }
    ui.add_space(10.0);
    widgets::separator(ui);
    ui.add_space(6.0);
    widgets::label(ui, tr("Open invites"), t.text);
    match &app.ui.account.invites {
        None => widgets::label(ui, tr("Asking the server…"), t.text_secondary),
        Some(list) if list.is_empty() => note(ui, t, tr("Every invite was used or has ended.")),
        Some(list) => {
            for i in list {
                let kind = if i["admin"] == true { tr("Admin invite") } else { tr("Invite") };
                let made = crate::comments::ago_long(i["created"].as_u64().unwrap_or(0));
                let ends = trf("ends in {days} days", &[("days", &days_left(i["expires"].as_i64().unwrap_or(0)))]);
                let id = i["id"].as_str().unwrap_or_default().to_string();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(kind).color(t.text));
                    note(ui, t, &format!("{made} · {ends}"));
                    if widgets::button(ui, tr("Withdraw"), false, true).clicked() {
                        *act = Some(AdminAct::Withdraw(id));
                    }
                });
            }
        }
    }
}

fn files_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens, act: &mut Option<AdminAct>) {
    let server = server(app);
    let files = app.ui.account.files.clone();
    let Some(files) = files else { return widgets::label(ui, tr("Asking the server…"), t.text_secondary) };
    note(ui, t, &trn("{n} files on the server", files.len(), &[]));
    ui.add_space(4.0);
    for f in &files {
        let (file, name) = (f["file"].as_str().unwrap_or_default().to_string(), f["name"].as_str().unwrap_or_default().to_string());
        if let Some(mut typed) = app.ui.account.renaming.as_ref().filter(|(id, _)| *id == file).map(|(_, n)| n.clone()) {
            let mut cancel = false;
            ui.horizontal(|ui| {
                if let Some(v) = widgets::text_field_named(ui, Id::unique(("admin_rename", &file)), &typed, FIELD_W, tr("File name")) {
                    typed = v;
                }
                if widgets::button(ui, tr("Save"), true, !typed.trim().is_empty()).clicked() {
                    *act = Some(AdminAct::Rename(file.clone(), typed.trim().to_string()));
                }
                cancel = widgets::button(ui, tr("Cancel"), false, true).clicked();
            });
            app.ui.account.renaming = (!cancel).then(|| (file.clone(), typed));
            continue;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(&name).color(t.text));
            note(ui, t, &trf("changed {when}", &[("when", &crate::comments::ago_long(f["updated"].as_u64().unwrap_or(0)))]));
        });
        if app.ui.account.deleting.as_deref() == Some(file.as_str()) {
            note(ui, t, &trf("Delete {file} for everyone? Its history goes too, and it can't be undone.", &[("file", &name)]));
            ui.horizontal(|ui| {
                if widgets::button(ui, &trf("Delete {file}", &[("file", &name)]), true, true).clicked() {
                    *act = Some(AdminAct::Delete(file.clone()));
                }
                if widgets::button(ui, tr("Cancel"), false, true).clicked() {
                    app.ui.account.deleting = None;
                }
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                if widgets::button(ui, &trf("Open {file}", &[("file", &name)]), false, true).clicked()
                    && let Some(r) = app.run("account.open", json!({"server": server, "file": file}))
                {
                    app.ui.live.home_join = r["session"].as_u64();
                }
                if widgets::button(ui, &trf("Share {file}", &[("file", &name)]), false, true).clicked() {
                    app.ui.account.share =
                        Some(ShareDialog { file: file.clone(), name: name.clone(), access: "edit".into(), ..ShareDialog::default() });
                    ask_members(app, &ui.ctx().clone(), &file);
                }
                if widgets::button(ui, &trf("Rename {file}", &[("file", &name)]), false, true).clicked() {
                    app.ui.account.renaming = Some((file.clone(), name.clone()));
                }
                if widgets::button(ui, &trf("Delete {file}", &[("file", &name)]), false, true).clicked() {
                    app.ui.account.deleting = Some(file.clone());
                }
            });
        }
        ui.add_space(6.0);
    }
}

fn activity_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens, act: &mut Option<AdminAct>) {
    ui.horizontal(|ui| {
        note(ui, t, tr("Sign-ins, sharing, deletions and resets on this server, newest first."));
        if widgets::button(ui, tr("Refresh"), false, true).clicked() {
            *act = Some(AdminAct::Refresh);
        }
    });
    ui.add_space(4.0);
    match &app.ui.account.activity {
        None => widgets::label(ui, tr("Asking the server…"), t.text_secondary),
        Some(list) if list.is_empty() => note(ui, t, tr("Nothing has happened on this server yet.")),
        Some(list) => {
            for e in list {
                let event = e["event"].as_str().unwrap_or_default().replace('_', " ");
                let who = e["who"].as_str().filter(|w| !w.is_empty()).map_or_else(|| tr("not signed in").to_string(), str::to_string);
                let when = crate::comments::ago_long(e["time"].as_u64().unwrap_or(0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(event).font(semibold(11.0)).color(t.text));
                    note(ui, t, &format!("{when} · {who} · {}", e["ip"].as_str().unwrap_or_default()));
                });
                if let Some(d) = e["detail"].as_str().filter(|d| !d.is_empty()) {
                    note(ui, t, d);
                }
                ui.add_space(4.0);
            }
        }
    }
}

/// Changing your own password: the current one, then the new one twice. Your other sign-ins end.
fn password(app: &mut FigApp, ctx: &egui::Context, t: &Tokens) {
    let Some(mut f) = app.ui.account.password.take() else { return };
    let mut open = true;
    let mut submit = false;
    crate::dialogs::window(tr("Change your password"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(FIELD_W + 20.0);
        for (label, value) in
            [(tr("Current password"), &mut f.old), (tr("New password (at least 8 characters)"), &mut f.new), (tr("New password again"), &mut f.again)]
        {
            widgets::label(ui, label, t.text_secondary);
            if let Some(v) = widgets::secret_field_named(ui, Id::unique(("change_password", label)), value, FIELD_W, label) {
                *value = v;
            }
        }
        let matches = f.new == f.again;
        if !f.again.is_empty() && !matches {
            note(ui, t, tr("The new passwords don't match."));
        }
        ui.add_space(8.0);
        submit = widgets::button(ui, tr("Change password"), true, !f.old.is_empty() && f.new.chars().count() >= 8 && matches).clicked();
        note(ui, t, tr("Your other sign-ins on this server end, including tokens for agents."));
    });
    if submit {
        let params = json!({"server": server(app), "old": f.old, "new": f.new});
        app.run_background(
            ctx,
            "account.changePassword",
            params,
            Box::new(|app, _, _| {
                app.ui.account.password = None;
                app.toast(tr("Password changed"));
            }),
        );
        f.old.clear();
    }
    if open {
        app.ui.account.password = Some(f);
    }
}
