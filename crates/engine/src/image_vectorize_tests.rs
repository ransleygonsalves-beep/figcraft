//! `image.vectorize` (Object ▸ Vectorize image, #2497): an image layer traced into a group of
//! filled vectors over it, the layer hidden, as one undo step; the tracing runs on its own thread,
//! reports its progress and can be cancelled; layers without an image fill are refused.

use base64::Engine as _;
use figcraft_color::PaintKind;
use figcraft_doc::{NodeId, NodeKind};
use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// A 120×80 image layer at (300, 200): a red disc on white.
fn disc_layer(s: &mut Session) -> NodeId {
    let image = image::RgbaImage::from_fn(120, 80, |x, y| {
        let inside = (x as f64 - 60.0).hypot(y as f64 - 40.0) < 30.0;
        image::Rgba(if inside { [210, 30, 40, 255] } else { [255, 255, 255, 255] })
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    id_of(&ex(s, "file.import", json!({"data": data, "name": "disc.png", "x": 300, "y": 200}))["id"]).unwrap()
}

#[test]
fn an_image_becomes_a_group_of_coloured_vectors_over_it() {
    let mut s = Session::new();
    let img = disc_layer(&mut s);
    let r = ex(&mut s, "image.vectorize", json!({"ids": [img], "colors": 2, "wait": true}));
    let group = id_of(&r["ids"][0]).unwrap();
    let doc = s.doc().unwrap();
    assert!(!doc.node(img).unwrap().visible, "the image is hidden, not removed");
    let parent = doc.node(img).unwrap().parent;
    assert_eq!(doc.node(group).unwrap().parent, parent);
    assert_eq!(doc.index_in_parent(group), doc.index_in_parent(img).map(|i| i + 1), "just above the image");
    assert_eq!(s.selection(), vec![group]);
    // Covers where the image was.
    let (a, b) = (doc.world_bounds(img).unwrap(), doc.world_bounds(group).unwrap());
    assert!((a.x0 - b.x0).abs() < 1.0 && (a.y0 - b.y0).abs() < 1.0 && (a.x1 - b.x1).abs() < 1.0 && (a.y1 - b.y1).abs() < 1.0, "{a:?} {b:?}");
    // White below, red above, each a filled vector.
    let kids = &doc.node(group).unwrap().children;
    assert_eq!(r["shapes"], json!(kids.len()));
    assert_eq!(kids.len(), 2, "two colours");
    let colour = |id: NodeId| match &doc.node(id).unwrap().fills[0].kind {
        PaintKind::Solid { color } => color.to_rgba8(),
        k => panic!("{k:?}"),
    };
    assert!(kids.iter().all(|k| matches!(doc.node(*k).unwrap().kind, NodeKind::Vector { .. })));
    let (bottom, top) = (colour(kids[0]), colour(kids[1]));
    assert!(bottom[0] > 240 && bottom[1] > 240, "white at the bottom: {bottom:?}");
    assert!(top[0] > 180 && top[1] < 80, "red on top: {top:?}");
    // The red shape sits where the disc is (centre 60, 40 of the image; radius 30).
    let disc = doc.world_bounds(kids[1]).unwrap();
    assert!((disc.center().x - 360.0).abs() < 2.0 && (disc.center().y - 240.0).abs() < 2.0 && (disc.width() - 60.0).abs() < 3.0, "{disc:?}");
    // One undo step brings the image back as it was.
    s.execute("edit.undo", &json!({})).unwrap();
    let doc = s.doc().unwrap();
    assert!(doc.node(img).unwrap().visible && doc.node(group).is_none());
}

#[test]
fn a_layer_without_an_image_is_refused() {
    let mut s = Session::new();
    let rect = id_of(&ex(&mut s, "node.create", json!({"type": "rectangle", "width": 50, "height": 50}))["id"]).unwrap();
    let e = s.execute("image.vectorize", &json!({"ids": [rect]})).unwrap_err();
    assert!(e.to_string().contains("image fill"), "{e}");
}

/// One test, as the job is the process's: a real job runs to the end and goes in, then a running
/// job is cancelled.
#[test]
fn the_tracing_runs_on_its_own_thread_and_can_be_cancelled() {
    use crate::vectorize::{self, Job};
    let mut s = Session::new();
    let img = disc_layer(&mut s);
    let started = ex(&mut s, "image.vectorize", json!({"ids": [img], "colors": 2}));
    assert_eq!(started["state"], "working", "{started}");
    assert!(s.doc().unwrap().node(img).unwrap().visible, "nothing changes while it traces");
    // Called again it reports progress, then, once done, puts the shapes in.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let done = loop {
        let r = ex(&mut s, "image.vectorize", json!({}));
        if r["state"] == "vectorized" {
            break r;
        }
        assert_eq!(r["state"], "working", "{r}");
        assert!(std::time::Instant::now() < deadline, "the job never finished");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let group = id_of(&done["ids"][0]).unwrap();
    assert_eq!(done["shapes"], 2);
    assert!(!s.doc().unwrap().node(img).unwrap().visible);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().node(group).is_none() && s.doc().unwrap().node(img).unwrap().visible, "one undo step");

    // Cancel: a running job is asked to stop, and the call after it ends says so.
    assert!(s.execute("image.vectorize", &json!({"cancel": true})).is_err(), "nothing to cancel");
    let uid = s.ds().unwrap().uid;
    vectorize::set(Job::Running { doc: uid, done: 0.3, images: 1 });
    assert_eq!(ex(&mut s, "image.vectorize", json!({"cancel": true}))["state"], "cancelling");
    vectorize::set(Job::Cancelled { doc: uid });
    assert_eq!(ex(&mut s, "image.vectorize", json!({}))["state"], "cancelled");
    assert!(vectorize::status().is_none(), "the job is gone");
}

#[test]
fn one_colour_mode_and_what_becomes_of_the_original() {
    let mut s = Session::new();
    // One colour: the red disc is darker than 0.9 of white, the white around it isn't.
    let img = disc_layer(&mut s);
    let r = ex(&mut s, "image.vectorize", json!({"ids": [img], "mode": "mono", "threshold": 0.9, "wait": true}));
    assert_eq!(r["shapes"], 1, "one shape: {r}");
    // Remove: the image goes.
    s.execute("edit.undo", &json!({})).unwrap();
    ex(&mut s, "image.vectorize", json!({"ids": [img], "colors": 2, "original": "remove", "wait": true}));
    assert!(s.doc().unwrap().node(img).is_none(), "the image was deleted");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().node(img).is_some(), "undo brings it back");
    // Keep: the image stays visible under the shapes.
    ex(&mut s, "image.vectorize", json!({"ids": [img], "colors": 2, "original": "keep", "wait": true}));
    assert!(s.doc().unwrap().node(img).unwrap().visible);
    // Unknown choices are refused, naming the ones there are.
    let e = s.execute("image.vectorize", &json!({"ids": [img], "mode": "sepia", "wait": true})).unwrap_err();
    assert!(e.to_string().contains("color|mono"), "{e}");
    let e = s.execute("image.vectorize", &json!({"ids": [img], "original": "archive", "wait": true})).unwrap_err();
    assert!(e.to_string().contains("hide|remove|keep"), "{e}");
}

#[test]
fn the_preview_shows_the_shapes_and_changes_nothing() {
    use base64::Engine as _;
    let mut s = Session::new();
    let img = disc_layer(&mut s);
    let before = s.doc().unwrap().clone();
    let r = ex(&mut s, "image.vectorizePreview", json!({"ids": [img], "colors": 2}));
    assert_eq!(r["shapes"], 2);
    assert!(r["width"].as_u64().unwrap() <= 360 && r["height"].as_u64().unwrap() <= 360, "{r}");
    let png = base64::engine::general_purpose::STANDARD.decode(r["png"].as_str().unwrap()).unwrap();
    let pic = image::load_from_memory(&png).unwrap().to_rgba8();
    // The disc's centre is red and opaque in the picture.
    let c = pic.get_pixel(pic.width() / 2, pic.height() / 2);
    assert!(c[0] > 180 && c[1] < 80 && c[3] == 255, "{c:?}");
    assert_eq!(*s.doc().unwrap(), before, "nothing changed");
    // One colour previews as one shape.
    assert_eq!(ex(&mut s, "image.vectorizePreview", json!({"ids": [img], "mode": "mono", "threshold": 0.9}))["shapes"], 1);
}
