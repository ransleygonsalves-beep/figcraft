//! Exporting several layers as PDF asks first: one PDF (frames in reading order, a page each) or a
//! file each. One layer, other formats or differing settings export as before without asking.

use figcraft_engine::Session;
use serde_json::json;

use crate::{FigApp, NoServices};

/// A session with frames at `places` (x, y), each set to export as `format`.
fn frames(places: &[(f64, f64)], format: &str) -> (FigApp, Vec<figcraft_doc::NodeId>) {
    let mut s = Session::new();
    let ids: Vec<figcraft_doc::NodeId> = places
        .iter()
        .map(|(x, y)| {
            let r = s.execute("node.create", &json!({"type": "frame", "x": x, "y": y, "width": 100, "height": 80})).unwrap();
            figcraft_engine::params::id_of(&r["id"]).unwrap()
        })
        .collect();
    s.execute("export.add", &json!({"ids": ids, "setting": {"format": format}})).unwrap();
    (FigApp::new(s, Box::new(NoServices)), ids)
}

#[test]
fn several_pdfs_ask_and_order_the_frames_as_read() {
    // Made out of order: the lower right one first.
    let (mut app, ids) = frames(&[(300.0, 200.0), (0.0, 0.0), (300.0, 0.0), (0.0, 200.0)], "pdf");
    crate::export_panel::export_layers(&mut app, &ids, false);
    let choice = app.ui.pdf_choice.clone().expect("asked");
    assert!(choice.frames && choice.ids == ids);
    assert_eq!(choice.ordered, vec![ids[1], ids[2], ids[3], ids[0]], "top row left to right, then the next");
    // One PDF holds them all, a page each.
    let r = app.session.execute("file.export", &json!({"ids": choice.ordered, "format": "pdf"})).unwrap();
    let pdf = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, r["data"].as_str().unwrap()).unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/Count 4"), "four pages");
}

#[test]
fn one_layer_or_other_formats_dont_ask() {
    let (mut app, ids) = frames(&[(0.0, 0.0), (200.0, 0.0)], "png");
    crate::export_panel::export_layers(&mut app, &ids, false);
    assert!(app.ui.pdf_choice.is_none(), "PNGs export as before");
    let (mut app, ids) = frames(&[(0.0, 0.0)], "pdf");
    crate::export_panel::export_layers(&mut app, &ids, false);
    assert!(app.ui.pdf_choice.is_none(), "one PDF needs no question");
}
