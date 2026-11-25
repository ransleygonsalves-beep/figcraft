//! `image.vectorize` (Object ▸ Vectorize image): an image layer traced into a group of filled
//! vectors over it, the layer hidden, as one undo step; layers without an image fill are refused.

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
    let r = ex(&mut s, "image.vectorize", json!({"ids": [img], "colors": 2}));
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
