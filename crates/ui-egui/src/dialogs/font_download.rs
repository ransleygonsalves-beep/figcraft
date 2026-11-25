//! Download font: a family picked from the font menu's Google Fonts and Noto section
//! (`font.catalog`). The first download asks the user to turn downloading on, with the prompt the
//! Missing fonts window shows; then `font.download` installs it on a thread of its own, and once
//! it's installed the family goes on the text it was picked for, as a pick from the menu does.
//! Closing the window doesn't stop a download under way; the family is then installed but not
//! applied.

use egui::RichText;
use figcraft_doc::NodeId;
use figcraft_engine::fonts::{Download, allow_downloads, downloads, downloads_allowed};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, regular};

/// The family being downloaded and the text it goes on.
#[derive(Clone, Debug)]
pub struct Pending {
    pub family: String,
    /// The document, layers and selected characters it was picked for.
    uid: u64,
    ids: Vec<NodeId>,
    range: Option<(NodeId, usize, usize)>,
    /// Waiting for the user to allow downloading fonts.
    pub consent: bool,
}

impl Pending {
    pub fn inspect(&self) -> Value {
        json!({"family": self.family, "consent": self.consent})
    }
}

/// The user picked `family` from the font menu for `ids` (or the `range` being edited): download
/// it now while downloading is on, else ask first.
pub fn request(app: &mut FigApp, family: String, ids: Vec<NodeId>, range: Option<(NodeId, usize, usize)>) {
    let Ok(uid) = app.session.ds().map(|d| d.uid) else { return };
    let consent = !downloads_allowed();
    if !consent {
        start(app, &family);
    }
    app.ui.font_download = Some(Pending { family, uid, ids, range, consent });
}

fn start(app: &mut FigApp, family: &str) {
    app.run("font.download", json!({"family": family}));
}

fn job(family: &str) -> Option<Download> {
    downloads().into_iter().find(|(f, _)| f.eq_ignore_ascii_case(family)).map(|(_, d)| d)
}

/// What the user did in the window this frame.
#[derive(Default)]
struct Clicks {
    consent: Option<bool>,
    retry: bool,
}

pub fn show(app: &mut FigApp, ctx: &egui::Context) {
    let Some(pending) = app.ui.font_download.clone() else { return };
    let job = job(&pending.family);
    if !pending.consent {
        // The Missing fonts window's upkeep runs `font.download` again for a finished download,
        // which fits the text and ends the job; so installed shows as the family being there.
        if job == Some(Download::Installed) || (job.is_none() && figcraft_text::has_family(&pending.family)) {
            app.ui.font_download = None;
            finish(app, ctx, &pending);
            return;
        }
        if job.is_none() {
            // It didn't start (the error is shown).
            app.ui.font_download = None;
            return;
        }
    }
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut clicks = Clicks::default();
    crate::dialogs::window(tr("Download font"), ctx).open(&mut open).show(ctx, |ui| clicks = body(ui, &t, &pending, job.as_ref()));
    if clicks.retry {
        start(app, &pending.family);
    }
    match clicks.consent {
        Some(true) => match allow_downloads() {
            Ok(()) => {
                start(app, &pending.family);
                if let Some(p) = app.ui.font_download.as_mut() {
                    p.consent = false;
                }
            }
            Err(e) => {
                app.ui.font_download = None;
                app.show_error(e);
            }
        },
        Some(false) => app.ui.font_download = None,
        None if !open => app.ui.font_download = None,
        None => {}
    }
}

fn body(ui: &mut egui::Ui, t: &Tokens, pending: &Pending, job: Option<&Download>) -> Clicks {
    let mut clicks = Clicks::default();
    ui.set_width(340.0);
    ui.label(RichText::new(&pending.family).strong());
    if pending.consent {
        clicks.consent = crate::missing_fonts::consent_prompt(ui, t);
        return clicks;
    }
    match job {
        Some(Download::Running { done, total }) => {
            let percent = if *total == 0 { 0 } else { done.saturating_mul(100) / total };
            ui.label(RichText::new(trf("Downloading… {percent}%", &[("percent", &percent)])).color(t.text_secondary));
        }
        Some(Download::Failed(error)) => {
            ui.add(egui::Label::new(RichText::new(error).color(t.danger).font(regular(11.0))).wrap());
            ui.add_space(6.0);
            clicks.retry = crate::widgets::button(ui, tr("Try again"), true, true).clicked();
        }
        Some(Download::Installed) | None => {}
    }
    clicks
}

/// Installed: the family goes on the text it was picked for, if that file is still in front.
fn finish(app: &mut FigApp, ctx: &egui::Context, pending: &Pending) {
    crate::typography::remember_family(ctx, &pending.family);
    if app.session.ds().is_ok_and(|d| d.uid == pending.uid) {
        crate::typography::commit_text(app, &pending.ids, pending.range, json!({"fontFamily": pending.family}));
    }
}
