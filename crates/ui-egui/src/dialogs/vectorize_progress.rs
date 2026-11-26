//! The Vectorize image window (#2497): while `image.vectorize` traces on its own thread it shows
//! how far it got and a Cancel button, the canvas staying free; when the job ends it runs the
//! command again, which puts the shapes in as one undo step (or reports the cancel or the error).
//! Closing the window leaves the job running; it still goes in when it ends.

use figcraft_engine::vectorize::{self, Job};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, trn};
use crate::theme::{Tokens, regular};

/// How often the window looks at the job while it runs.
const POLL: std::time::Duration = std::time::Duration::from_millis(150);

#[derive(Clone, Debug)]
pub struct Shown {
    /// The document the job is for.
    pub uid: u64,
    /// Closed by the user: the job goes on and its shapes still go in, without the window.
    pub hidden: bool,
}

impl Shown {
    pub fn inspect(&self) -> Value {
        let job = vectorize::status().map(|j| match j {
            Job::Running { done, images, .. } => json!({"state": "working", "done": done, "images": images}),
            Job::Done { .. } => json!({"state": "done"}),
            Job::Cancelled { .. } => json!({"state": "cancelled"}),
            Job::Failed { error, .. } => json!({"state": "failed", "error": error}),
        });
        json!({"hidden": self.hidden, "job": job})
    }
}

/// The command asked for the window (`state` is what it reported): open it while the job works,
/// close it once the shapes are in or the job was cancelled.
pub fn request(app: &mut FigApp, data: &Value) {
    let Ok(uid) = app.session.ds().map(|d| d.uid) else { return };
    let was_open = app.ui.vectorizing.is_some();
    match data["state"].as_str() {
        Some("working") => {
            let hidden = app.ui.vectorizing.as_ref().is_some_and(|s| s.uid == uid && s.hidden);
            app.ui.vectorizing = Some(Shown { uid, hidden });
        }
        Some("vectorized") => {
            app.ui.vectorizing = None;
            if was_open {
                let shapes = usize::try_from(data["shapes"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
                app.toast(trn("Vectorized into {n} shapes", shapes, &[]));
            }
        }
        Some("cancelled") => {
            app.ui.vectorizing = None;
            app.toast(tr("Vectorize image was cancelled"));
        }
        _ => {}
    }
}

pub fn show(app: &mut FigApp, ctx: &egui::Context) {
    let Some(shown) = app.ui.vectorizing.clone() else { return };
    let job = vectorize::status();
    // A job that ended for the file in front: the command puts it in (or says why not).
    if matches!(job, Some(Job::Done { doc, .. } | Job::Cancelled { doc } | Job::Failed { doc, .. }) if doc == shown.uid)
        && app.session.ds().is_ok_and(|d| d.uid == shown.uid)
    {
        app.ui.vectorizing = None;
        app.run("image.vectorize", json!({}));
        return;
    }
    let Some(Job::Running { done, images, .. }) = job else {
        if job.is_none() {
            app.ui.vectorizing = None;
        }
        return;
    };
    ctx.request_repaint_after(POLL);
    if shown.hidden {
        return;
    }
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut cancel = false;
    crate::dialogs::window(tr("Vectorize image"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(340.0);
        let text = if images > 1 { trn("Tracing {n} images…", images, &[]) } else { tr("Tracing the image…").to_string() };
        ui.label(text);
        ui.add_space(6.0);
        ui.add(egui::ProgressBar::new(done.clamp(0.0, 1.0)).show_percentage());
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr("You can keep working; the shapes appear when it's done.")).color(t.text_secondary).font(regular(11.0)),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        cancel = crate::widgets::button(ui, tr("Cancel"), false, true).clicked();
    });
    if cancel {
        app.run("image.vectorize", json!({"cancel": true}));
    } else if !open && let Some(s) = app.ui.vectorizing.as_mut() {
        s.hidden = true;
    }
}
