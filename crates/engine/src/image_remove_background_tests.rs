//! `image.removeBackground`: refused without an image or before the user allowed the model; a
//! finished job's cut-outs go into the paints that still show the images they came from, as one
//! undo step; the mask keeps the subject's pixels and clears the rest. The model itself isn't run
//! here (it is a 214 MiB download). Desktop only, as the model is.
#![cfg(not(target_arch = "wasm32"))]

use base64::Engine as _;
use figcraft_color::PaintKind;
use figcraft_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;
use crate::cutout::{self, Cut, Job, Target};
use crate::params::id_of;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn png(w: u32, h: u32, at: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(at(x, y)));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn image_layer(s: &mut Session) -> NodeId {
    let data = base64::engine::general_purpose::STANDARD.encode(png(40, 30, |_, _| [200, 120, 40, 255]));
    id_of(&ex(s, "file.import", json!({"data": data, "name": "photo.png", "x": 0, "y": 0}))["id"]).unwrap()
}

fn image_key(s: &Session, id: NodeId) -> String {
    match &s.doc().unwrap().node(id).unwrap().fills[0].kind {
        PaintKind::Image(img) => img.image.clone(),
        k => panic!("{k:?}"),
    }
}

/// One test, as the job is the process's: refusals, then a finished job put in.
#[test]
fn refused_until_allowed_and_a_finished_job_goes_in_as_one_undo_step() {
    let mut s = Session::new();
    let rect = id_of(&ex(&mut s, "node.create", json!({"type": "rectangle", "width": 50, "height": 50}))["id"]).unwrap();
    let e = s.execute("image.removeBackground", &json!({"ids": [rect]})).unwrap_err();
    assert!(e.to_string().contains("image fill"), "{e}");

    let img = image_layer(&mut s);
    let e = s.execute("image.removeBackground", &json!({"ids": [img]})).unwrap_err();
    assert!(e.to_string().contains("allow"), "the model needs the user's go-ahead first: {e}");
    assert!(
        std::mem::take(&mut s.ui_requests).iter().any(|r| r.kind == "image.removeBackground" && r.data["state"] == "needsModel"),
        "the app is asked to show the window"
    );

    // A job that finished for this file: its cut-out replaces the image, keeping the paint.
    let before = image_key(&s, img);
    let uid = s.ds().unwrap().uid;
    let cut = png(40, 30, |x, _| if x < 20 { [200, 120, 40, 255] } else { [0, 0, 0, 0] });
    cutout::set(Job::Done { doc: uid, cuts: vec![Cut { target: Target { node: img, fill: 0, image: before.clone() }, png: cut }] });
    let r = ex(&mut s, "image.removeBackground", json!({}));
    assert_eq!(r, json!({"state": "removed", "changed": 1}));
    let after = image_key(&s, img);
    assert_ne!(after, before);
    assert!(s.doc().unwrap().images.contains_key(&after));
    assert!(cutout::status().is_none(), "the job is used up");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(image_key(&s, img), before, "one undo step brings the image back");

    // A cut-out whose paint shows another image by now changes nothing.
    cutout::set(Job::Done {
        doc: uid,
        cuts: vec![Cut { target: Target { node: img, fill: 0, image: "other".into() }, png: png(2, 2, |_, _| [0; 4]) }],
    });
    assert_eq!(ex(&mut s, "image.removeBackground", json!({}))["changed"], 0);
    // A failed job reports why.
    cutout::set(Job::Failed { doc: uid, error: "no network".into() });
    assert!(s.execute("image.removeBackground", &json!({})).unwrap_err().to_string().contains("no network"));
}

#[test]
fn the_mask_keeps_the_subject_and_clears_the_rest() {
    let side = cutout::SIDE_FOR_TESTS;
    let mut pix = figcraft_render::Pixmap::new(64, 32).unwrap();
    for px in pix.data_mut().chunks_exact_mut(4) {
        px.copy_from_slice(&[200, 120, 40, 255]);
    }
    // The subject is the left half.
    let mask: Vec<f32> = (0..side * side).map(|i| if i % side < side / 2 { 1.0 } else { 0.0 }).collect();
    let out = cutout::apply_mask_for_tests(&pix, &mask);
    let alpha = |x: usize, y: usize| out.data()[(y * 64 + x) * 4 + 3];
    assert_eq!(alpha(5, 10), 255, "kept");
    assert_eq!(alpha(60, 10), 0, "cleared");
    assert!(alpha(31, 10) > alpha(33, 10), "fades across the edge, left to right");
}
