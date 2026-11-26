//! The Vectorize image options (#2497): Object ▸ Vectorize image… opens them for the selected
//! image layer (`image.vectorizePreview` asks for the window). Color or one color, the number of
//! colors or the threshold, detail, smoothness and what becomes of the original; a preview of the
//! shapes follows the options, a moment after they stop changing. Vectorize runs
//! `image.vectorize` with them (its progress window takes over); the options are kept for next time.

use egui::{Rect, RichText, Sense, vec2};
use figcraft_doc::NodeId;
use figcraft_engine::vectorize::{Original, Settings, preview};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, trn};
use crate::theme::{Tokens, regular};

/// How long the options stay still before the preview is traced again (seconds).
const SETTLE: f64 = 0.25;
/// The preview's box in the window.
const PREVIEW_W: f32 = 320.0;
const PREVIEW_H: f32 = 220.0;
/// The width of an option's name in front of its slider.
const NAME_W: f32 = 84.0;

/// The open window.
pub struct Options {
    /// The document and the layers it vectorizes (the first with an image fill is previewed).
    pub uid: u64,
    pub ids: Vec<NodeId>,
    pub settings: Settings,
    /// When the options last changed.
    pub changed_at: f64,
    /// The preview shown, and the options it was traced with.
    pub preview: Option<(egui::TextureHandle, usize, Settings)>,
    /// The layer couldn't be traced for a preview.
    pub failed: bool,
}

impl Options {
    pub fn inspect(&self) -> Value {
        json!({
            "ids": self.ids,
            "params": params(&self.settings),
            "previewShapes": self.preview.as_ref().filter(|p| p.2 == self.settings).map(|p| p.1),
        })
    }
}

/// `image.vectorize`'s parameters for `s`.
pub fn params(s: &Settings) -> Value {
    json!({
        "colors": s.colors,
        "detail": s.detail,
        "smoothness": s.smoothness,
        "mode": if s.mono.is_some() { "mono" } else { "color" },
        "threshold": s.mono.unwrap_or(0.5),
        "original": s.original.id(),
    })
}

/// `image.vectorizePreview` asked for the window for `data.ids`.
pub fn request(app: &mut FigApp, data: &Value) {
    let Ok(uid) = app.session.ds().map(|d| d.uid) else { return };
    let ids: Vec<NodeId> = data["ids"].as_array().into_iter().flatten().filter_map(figcraft_engine::params::id_of).collect();
    if ids.is_empty() {
        return;
    }
    app.ui.vectorize_options =
        Some(Options { uid, ids, settings: app.ui.vectorize_last, changed_at: f64::NEG_INFINITY, preview: None, failed: false });
}

pub fn show(app: &mut FigApp, ctx: &egui::Context) {
    let Some(mut w) = app.ui.vectorize_options.take() else { return };
    // The window belongs to its file: it closes when another one comes to the front.
    if app.session.ds().map(|d| d.uid).ok() != Some(w.uid) {
        return;
    }
    let now = ctx.input(|i| i.time);
    refresh_preview(app, ctx, &mut w, now);
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut action = None;
    crate::dialogs::window(tr("Vectorize image"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(PREVIEW_W);
        preview_box(ui, &t, &w);
        ui.add_space(8.0);
        let before = w.settings;
        options(ui, &t, &mut w.settings);
        if w.settings != before {
            w.changed_at = now;
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if crate::widgets::button(ui, tr("Vectorize"), true, true).clicked() {
                action = Some(true);
            }
            if crate::widgets::button(ui, tr("Cancel"), false, true).clicked() {
                action = Some(false);
            }
        });
    });
    match action {
        Some(true) => {
            app.ui.vectorize_last = w.settings;
            let mut p = params(&w.settings);
            p["ids"] = json!(w.ids);
            app.run("image.vectorize", p);
        }
        Some(false) => {}
        None if open => app.ui.vectorize_options = Some(w),
        None => {}
    }
}

/// Trace the preview again once the options have stayed still a moment.
fn refresh_preview(app: &mut FigApp, ctx: &egui::Context, w: &mut Options, now: f64) {
    if w.preview.as_ref().is_some_and(|p| p.2 == w.settings) || (w.failed && now - w.changed_at > SETTLE) {
        return;
    }
    if now - w.changed_at < SETTLE {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
        return;
    }
    let Ok(doc) = app.session.doc() else { return };
    let Some(id) = w.ids.iter().copied().find(|id| doc.node(*id).and_then(figcraft_engine::cmd::image::top_image_fill).is_some()) else {
        w.failed = true;
        return;
    };
    match preview(doc, id, &w.settings) {
        Some(p) => {
            let img = crate::render_worker::screen_image(&p.image, doc.color_profile);
            let tex = ctx.load_texture("vectorize-preview", img, egui::TextureOptions::LINEAR);
            w.preview = Some((tex, p.shapes, w.settings));
            w.failed = false;
        }
        None => w.failed = true,
    }
}

/// The preview over a checkerboard (what is transparent shows as such), and how many shapes it has.
fn preview_box(ui: &mut egui::Ui, t: &Tokens, w: &Options) {
    let (rect, resp) = ui.allocate_exact_size(vec2(PREVIEW_W, PREVIEW_H), Sense::hover());
    crate::widgets::checker(ui.painter(), rect, 8.0);
    let caption = match &w.preview {
        Some((tex, shapes, _)) => {
            let [iw, ih] = tex.size();
            let k = (rect.width() / iw.max(1) as f32).min(rect.height() / ih.max(1) as f32);
            let r = Rect::from_center_size(rect.center(), vec2(iw as f32 * k, ih as f32 * k));
            ui.painter().image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            trn("Preview: {n} shapes", *shapes, &[])
        }
        None if w.failed => tr("This layer can't be previewed").to_string(),
        None => tr("Preparing the preview…").to_string(),
    };
    let stale = w.preview.as_ref().is_some_and(|p| p.2 != w.settings);
    crate::a11y::image(&resp, &caption);
    ui.add_space(4.0);
    ui.label(RichText::new(if stale { format!("{caption} {}", tr("(updating)")) } else { caption }).color(t.text_secondary).font(regular(11.0)));
}

/// The options, changed in place.
fn options(ui: &mut egui::Ui, t: &Tokens, s: &mut Settings) {
    let mono = s.mono.is_some();
    if let Some(i) = crate::widgets::segmented_text(ui, &[tr("Color"), tr("One color")], usize::from(mono)) {
        s.mono = if i == 1 { Some(s.mono.unwrap_or(0.5)) } else { None };
    }
    ui.add_space(6.0);
    let d = Settings::default();
    match s.mono {
        None => {
            let colors = slider_row(ui, t, "vectorize_colors", tr("Colors"), 2.0..=64.0, s.colors as f64, d.colors as f64, &s.colors.to_string());
            s.colors = colors.round().clamp(2.0, 64.0) as usize;
        }
        Some(th) => {
            let shown = format!("{}%", (th * 100.0).round());
            s.mono = Some(slider_row(ui, t, "vectorize_threshold", tr("Threshold"), 0.0..=1.0, f64::from(th), 0.5, &shown) as f32);
        }
    }
    let pct = |v: f64| format!("{}%", (v * 100.0).round());
    s.detail = slider_row(ui, t, "vectorize_detail", tr("Detail"), 0.0..=1.0, s.detail, d.detail, &pct(s.detail));
    s.smoothness = slider_row(ui, t, "vectorize_smoothness", tr("Smoothness"), 0.0..=1.0, s.smoothness, d.smoothness, &pct(s.smoothness));
    ui.add_space(6.0);
    for (o, label) in
        [(Original::Hide, tr("Hide the original")), (Original::Remove, tr("Delete the original")), (Original::Keep, tr("Keep the original visible"))]
    {
        if crate::widgets::radio(ui, s.original == o, label) {
            s.original = o;
        }
    }
}

/// One option: its name, a slider (a double-click puts `default` back) and its value. Returns the
/// value, changed or not.
fn slider_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    id: &str,
    name: &str,
    range: std::ops::RangeInclusive<f64>,
    value: f64,
    default: f64,
    shown: &str,
) -> f64 {
    let mut out = value;
    ui.horizontal(|ui| {
        ui.add_sized(vec2(NAME_W, 20.0), egui::Label::new(RichText::new(name).color(t.text)));
        let width = PREVIEW_W - NAME_W - 52.0;
        if let Some((v, _)) = crate::widgets::slider(ui, egui::Id::unique(id), range, Some(default), Some(value), width, name) {
            out = v.unwrap_or(default);
        }
        ui.label(RichText::new(shown).color(t.text_secondary));
    });
    out
}
