//! Exporting several layers as PDF: one PDF holding them all (a page per frame, top to bottom then
//! left to right on the canvas) or a PDF file each. [`export_layers`] asks when every file it
//! would write is a PDF with the same setting.

use super::{Context, FigApp, RichText, Tokens, regular, tr, trn, window};
use crate::export_panel::{export_each, save_one};

/// The question, while [`crate::UiState::pdf_choice`] holds one.
pub(super) fn pdf_choice_dialog(app: &mut FigApp, ctx: &Context) {
    let Some(choice) = app.ui.pdf_choice.clone() else { return };
    let t = Tokens::get(ctx);
    let n = choice.ordered.len();
    let mut open = true;
    let mut picked = None;
    let mut cancel = false;
    window(tr("Export as PDF"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(
            RichText::new(trn("{n} layers export as PDF. Put them in one file, or a file each?", n, &[])).color(t.text_secondary).font(regular(11.0)),
        );
        ui.add_space(10.0);
        let (one, how) = if choice.frames {
            (trn("One PDF with {n} pages", n, &[]), tr("A page per frame, top to bottom and left to right as on the canvas."))
        } else {
            (tr("One PDF").to_string(), tr("They aren't all frames, so they share one page."))
        };
        if crate::widgets::button(ui, &one, true, true).clicked() {
            picked = Some(true);
        }
        ui.label(RichText::new(how).color(t.text_secondary).font(regular(11.0)));
        ui.add_space(8.0);
        if crate::widgets::button(ui, &trn("{n} separate PDF files", n, &[]), false, true).clicked() {
            picked = Some(false);
        }
        ui.add_space(8.0);
        if crate::widgets::button(ui, tr("Cancel"), false, true).clicked() {
            cancel = true;
        }
    });
    if picked.is_some() || cancel || !open {
        app.ui.pdf_choice = None;
    }
    match picked {
        Some(true) => save_one(app, &choice.ordered, &choice.setting, &choice.file),
        Some(false) => export_each(app, &choice.ids, choice.assets),
        None => {}
    }
}
