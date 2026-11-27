//! Export as MP4: the first time, a window asks to download Cisco's H.264 codec (with Cisco's
//! notice); then a save dialog; then a window following the export (downloading the codec,
//! encoding frame N of M) with Cancel, and a toast when it's written.

use serde_json::json;

use super::{Context, FigApp, RichText, Tokens, regular, tr, trf, window};

/// What the MP4 export shows.
#[derive(Clone, Debug)]
pub enum Mp4Ui {
    /// Asking to download the codec, for exporting `ids`.
    Consent { ids: Vec<figcraft_doc::NodeId> },
    /// Following the export of the file named `name`.
    Running { name: String },
}

/// Export `ids` as an MP4 (asks for the codec first if it isn't allowed yet).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn export_mp4(app: &mut FigApp, ids: &[figcraft_doc::NodeId]) {
    if !figcraft_engine::video_export::codec::ready() {
        app.ui.mp4 = Some(Mp4Ui::Consent { ids: ids.to_vec() });
        return;
    }
    let name = ids.first().and_then(|i| app.session.doc().ok()?.node(*i).map(|n| format!("{}.mp4", n.name))).unwrap_or_else(|| "Video.mp4".into());
    let ids = ids.to_vec();
    app.pick_save(tr("Export as MP4"), &name, &["mp4"], move |app, p| {
        let file = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if app.run("file.exportVideo", json!({"ids": ids, "path": p.to_string_lossy()})).is_some() {
            app.ui.mp4 = Some(Mp4Ui::Running { name: file });
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn export_mp4(app: &mut FigApp, _: &[figcraft_doc::NodeId]) {
    app.toast(tr("Video export works in the desktop app"));
}

/// The consent and progress windows, while one is up.
pub(super) fn mp4_dialog(app: &mut FigApp, ctx: &Context) {
    let Some(state) = app.ui.mp4.clone() else { return };
    let t = Tokens::get(ctx);
    let mut open = true;
    match state {
        Mp4Ui::Consent { ids } => {
            let (mut go, mut cancel) = (false, false);
            window(tr("Export as MP4"), ctx).open(&mut open).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(
                    RichText::new(tr("Video export uses Cisco's H.264 codec (OpenH264), about 0.5 MB, downloaded once from Cisco. Cisco's build carries the H.264 licence."))
                        .color(t.text_secondary)
                        .font(regular(11.0)),
                );
                ui.add_space(4.0);
                ui.label(RichText::new("OpenH264 Video Codec provided by Cisco Systems, Inc.").color(t.text_secondary).font(regular(11.0)));
                ui.add_space(10.0);
                go = crate::widgets::button(ui, tr("Download and continue"), true, true).clicked();
                cancel = crate::widgets::button(ui, tr("Cancel"), false, true).clicked();
            });
            open &= !cancel;
            if go {
                app.ui.mp4 = None;
                #[cfg(not(target_arch = "wasm32"))]
                match figcraft_engine::video_export::codec::allow_download() {
                    Ok(()) => export_mp4(app, &ids),
                    Err(e) => app.toast(&e),
                }
                #[cfg(target_arch = "wasm32")]
                let _ = ids;
                return;
            }
        }
        Mp4Ui::Running { name } => {
            let status = app.session.execute("video.exportStatus", &json!({})).unwrap_or_default();
            if status["running"] != true {
                app.ui.mp4 = None;
                match status["error"].as_str() {
                    Some(e) => app.toast(trf("Couldn't export {name}: {error}", &[("name", &name), ("error", &e)])),
                    None if status["done"] == true => app.toast(trf("Exported {name}", &[("name", &name)])),
                    None => {}
                }
                return;
            }
            let (done, total) = (status["done"].as_u64().unwrap_or(0), status["total"].as_u64().unwrap_or(0));
            let line = if status["phase"] == "downloading" {
                trf("Downloading the codec… {done} of {total} KB", &[("done", &(done / 1024)), ("total", &(total / 1024))])
            } else {
                trf("Encoding frame {done} of {total}", &[("done", &done), ("total", &total)])
            };
            let mut cancel = false;
            window(tr("Exporting video"), ctx).open(&mut open).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new(&name).color(t.text));
                ui.label(RichText::new(line).color(t.text_secondary).font(regular(11.0)));
                ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }));
                ui.add_space(6.0);
                cancel = crate::widgets::button(ui, tr("Cancel"), false, true).clicked();
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            if cancel || !open {
                app.session.execute("video.cancelExport", &json!({})).ok();
            }
            return;
        }
    }
    if !open {
        app.ui.mp4 = None;
    }
}
