//! The Vectorize image window (#2497): Object ▸ Vectorize image opens it while the tracing runs on
//! its own thread, the app keeps drawing frames, and the shapes go in by themselves when it ends.

use base64::Engine as _;
use serde_json::json;

use crate::present_tests::{Ui, id};

#[test]
fn the_window_follows_the_job_and_the_shapes_go_in_by_themselves() {
    let mut s = figcraft_engine::Session::new();
    let image = image::RgbaImage::from_fn(120, 80, |x, y| {
        let inside = (f64::from(x) - 60.0).hypot(f64::from(y) - 40.0) < 30.0;
        image::Rgba(if inside { [210, 30, 40, 255] } else { [255, 255, 255, 255] })
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    let img = id(&s.execute("file.import", &json!({"data": data, "name": "disc.png", "x": 0, "y": 0})).unwrap());
    let mut ui = Ui::new(s);
    let started = ui.app.run("image.vectorize", json!({"ids": [img], "colors": 2})).unwrap();
    assert_eq!(started["state"], "working");
    assert!(ui.app.ui.vectorizing.is_some(), "the window opens");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while ui.app.ui.vectorizing.is_some() {
        assert!(std::time::Instant::now() < deadline, "the job never finished");
        ui.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let doc = ui.app.session.doc().unwrap();
    assert!(!doc.node(img).unwrap().visible, "the shapes went in and the image is hidden");
    let group = ui.app.session.selection()[0];
    assert_eq!(doc.node(group).unwrap().children.len(), 2);
}

/// Object ▸ Vectorize image… (`image.vectorizePreview`) opens the options for the selection; the
/// preview follows them, and the window closes when its file leaves the front.
#[test]
fn the_options_window_previews_the_options() {
    let mut s = figcraft_engine::Session::new();
    let image = image::RgbaImage::from_fn(120, 80, |x, y| {
        let inside = (f64::from(x) - 60.0).hypot(f64::from(y) - 40.0) < 30.0;
        image::Rgba(if inside { [210, 30, 40, 255] } else { [255, 255, 255, 255] })
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    let img = id(&s.execute("file.import", &json!({"data": data, "name": "disc.png", "x": 0, "y": 0})).unwrap());
    let mut ui = Ui::new(s);
    ui.app.run("image.vectorizePreview", json!({"ids": [img]}));
    assert!(ui.app.ui.vectorize_options.is_some(), "the options open");
    ui.frames(2, Vec::new);
    let shapes = |ui: &Ui| ui.app.inspect()["vectorizeOptions"]["previewShapes"].as_u64();
    assert_eq!(shapes(&ui), Some(2), "{}", ui.app.inspect()["vectorizeOptions"]);
    // One color: the preview follows.
    ui.app.ui.vectorize_options.as_mut().unwrap().settings.mono = Some(0.9);
    ui.frames(2, Vec::new);
    assert_eq!(shapes(&ui), Some(1));
    assert_eq!(ui.app.inspect()["vectorizeOptions"]["params"]["mode"], "mono");
    // Nothing changed in the file, and the options aren't remembered until they run.
    assert!(ui.app.session.doc().unwrap().node(img).unwrap().visible);
    assert_eq!(ui.app.ui.vectorize_last, figcraft_engine::vectorize::Settings::default());
    // Another file in front: the window goes.
    ui.run("file.new", json!({}));
    ui.frames(1, Vec::new);
    assert!(ui.app.ui.vectorize_options.is_none());
}
