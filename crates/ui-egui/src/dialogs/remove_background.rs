//! The Remove background window (`image.removeBackground`): the first time, it asks the user to
//! allow the model's one-time download (the only place that allows it); then it shows the job's
//! progress while the user keeps working, and when the job is done it runs the command again,
//! which puts the cut-out images in as one undo step.

use egui::RichText;
use figcraft_doc::NodeId;
use figcraft_engine::cutout::{self, Job, Phase};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, regular};

/// How often the window looks at the job while it runs.
const POLL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct Shown {
    /// The document the job is for.
    pub uid: u64,
    /// The layers to cut out once the user allows the download.
    pub ids: Vec<NodeId>,
    /// The model isn't downloaded and the user hasn't allowed it yet.
    pub needs_model: bool,
    /// Closed by the user: the job goes on and still finishes, without the window.
    pub hidden: bool,
}

impl Shown {
    pub fn inspect(&self) -> Value {
        json!({"needsModel": self.needs_model, "hidden": self.hidden, "job": cutout::status().map(|j| job_json(&j))})
    }
}

fn job_json(job: &Job) -> Value {
    match job {
        Job::Running { phase: Phase::Downloading { done, total }, .. } => json!({"state": "downloading", "done": done, "total": total}),
        Job::Running { phase: Phase::Loading, .. } => json!({"state": "loading"}),
        Job::Running { phase: Phase::Working { done, total }, .. } => json!({"state": "working", "done": done, "total": total}),
        Job::Done { .. } => json!({"state": "done"}),
        Job::Failed { error, .. } => json!({"state": "failed", "error": error}),
    }
}

/// The command asked for the window (`state` is what it reported): open it, or close it once the
/// images are in.
pub fn request(app: &mut FigApp, data: &Value) {
    let Ok(uid) = app.session.ds().map(|d| d.uid) else { return };
    match data["state"].as_str() {
        Some("removed") => {
            app.ui.remove_background = None;
            if data["changed"].as_u64().unwrap_or(0) > 0 {
                app.toast(tr("Background removed"));
            } else {
                app.toast(tr("The image changed while its background was being removed, so it was left as it is"));
            }
        }
        Some("needsModel") => {
            let ids = app.session.selection();
            app.ui.remove_background = Some(Shown { uid, ids, needs_model: true, hidden: false });
        }
        _ => {
            let keep = app.ui.remove_background.as_ref().is_some_and(|s| s.uid == uid && s.hidden);
            app.ui.remove_background = Some(Shown { uid, ids: Vec::new(), needs_model: false, hidden: keep });
        }
    }
}

pub fn show(app: &mut FigApp, ctx: &egui::Context) {
    let Some(shown) = app.ui.remove_background.clone() else { return };
    let job = cutout::status();
    // A job that finished for the file in front: run the command again to put its images in (or
    // to say why it failed).
    if !shown.needs_model
        && matches!(job, Some(Job::Done { doc, .. } | Job::Failed { doc, .. }) if doc == shown.uid)
        && app.session.ds().is_ok_and(|d| d.uid == shown.uid)
    {
        app.ui.remove_background = None;
        app.run("image.removeBackground", json!({}));
        return;
    }
    if !shown.needs_model && job.is_none() {
        app.ui.remove_background = None;
        return;
    }
    ctx.request_repaint_after(POLL);
    if shown.hidden {
        return;
    }
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut answer = None;
    crate::dialogs::window(tr("Remove background"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(380.0);
        if shown.needs_model {
            answer = consent(ui, &t);
        } else if let Some(job) = &job {
            progress(ui, &t, job);
        }
    });
    match answer {
        Some(true) => match cutout::allow_download() {
            Ok(()) => {
                app.ui.remove_background = None;
                app.run("image.removeBackground", json!({"ids": shown.ids}));
            }
            Err(e) => app.show_error(e),
        },
        Some(false) => app.ui.remove_background = None,
        None if !open => {
            if let Some(s) = app.ui.remove_background.as_mut() {
                if s.needs_model {
                    app.ui.remove_background = None;
                } else {
                    s.hidden = true;
                }
            }
        }
        None => {}
    }
}

/// Asks to download the model: `Some(true)` to allow it and go on, `Some(false)` to cancel.
fn consent(ui: &mut egui::Ui, t: &Tokens) -> Option<bool> {
    let size = cutout::MODEL_SIZE_MB.to_string();
    ui.add(
        egui::Label::new(RichText::new(trf(
            "Remove background uses a model that runs on this computer, so your images stay here. It is downloaded once: {size} MB from the BiRefNet project on GitHub ({licence} licence).",
            &[("size", &size), ("licence", &cutout::MODEL_LICENSE)],
        )))
        .wrap(),
    );
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new(tr("Each image takes about 15 seconds. You can keep working while it runs.")).color(t.text_secondary).font(regular(11.0)),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if crate::widgets::button(ui, tr("Download and remove"), true, true).clicked() {
            Some(true)
        } else if crate::widgets::button(ui, tr("Cancel"), false, true).clicked() {
            Some(false)
        } else {
            None
        }
    })
    .inner
}

/// What the job is doing, with a bar for how far it got.
fn progress(ui: &mut egui::Ui, t: &Tokens, job: &Job) {
    let mb = |b: u64| (b / (1 << 20)).to_string();
    let (text, done) = match job {
        Job::Running { phase: Phase::Downloading { done, total }, .. } => (
            trf("Downloading the model… {done} of {total} MB", &[("done", &mb(*done)), ("total", &mb((*total).max(cutout::MODEL_SIZE_MB << 20)))]),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Job::Running { phase: Phase::Loading, .. } => (tr("Loading the model…").to_string(), None),
        Job::Running { phase: Phase::Working { done, total }, .. } => (
            trf("Removing the background… image {n} of {total}", &[("n", &(done + 1)), ("total", total)]),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Job::Done { .. } => (tr("Background removed").to_string(), Some(1.0)),
        Job::Failed { error, .. } => (error.clone(), None),
    };
    ui.label(&text);
    ui.add_space(6.0);
    let bar = egui::ProgressBar::new(done.unwrap_or(0.0));
    ui.add(if done.is_some() { bar } else { bar.animate(true) });
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(RichText::new(tr("You can keep working; the image changes when it's done.")).color(t.text_secondary).font(regular(11.0)))
            .wrap(),
    );
}
