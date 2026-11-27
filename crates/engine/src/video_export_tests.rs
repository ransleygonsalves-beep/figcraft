//! Videos in documents and MP4 export, through the commands: a placed MP4 becomes a "Video"
//! layer at its own size; exporting it (with `wait`) writes an MP4 as long as the video, at its
//! rate and the layer's size; layers where nothing moves are refused with a reason. The codec
//! comes from `FIGCRAFT_OPENH264` (Cisco's library); without it these skip.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::json;

use crate::Session;

fn codec() -> bool {
    match std::env::var("FIGCRAFT_OPENH264") {
        Ok(p) => figcraft_video::set_codec(p).is_ok(),
        Err(_) => {
            eprintln!("skipped: set FIGCRAFT_OPENH264 to Cisco's OpenH264 2.6.0 library");
            false
        }
    }
}

/// A one-second, 24-frame MP4 of 96 × 64 whose colour changes every frame.
fn clip() -> Vec<u8> {
    let mut w = figcraft_video::Writer::new(96, 64, 24.0).unwrap();
    for i in 0..24u32 {
        let px = [(i * 10) as u8, 90, 200, 255];
        w.push(&figcraft_video::Frame { width: 96, height: 64, rgba: px.repeat(96 * 64) }).unwrap();
    }
    w.finish().unwrap()
}

#[test]
fn a_placed_video_exports_as_an_mp4_of_its_length() {
    if !codec() {
        return;
    }
    let mut s = Session::new();
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, clip());
    let placed = s.execute("file.placeImage", &json!({"data": data, "x": 0, "y": 0})).unwrap();
    let id = placed["id"].as_u64().unwrap();
    assert_eq!((placed["width"].as_f64(), placed["height"].as_f64()), (Some(96.0), Some(64.0)));
    assert_eq!(s.doc().unwrap().node(figcraft_doc::NodeId(id)).unwrap().name, "Video");
    let out = std::env::temp_dir().join(format!("figcraft-export-{}.mp4", std::process::id()));
    let r = s.execute("file.exportVideo", &json!({"ids": [id], "path": out.to_string_lossy(), "wait": true})).unwrap();
    assert_eq!(r["frames"], 24, "{r}");
    let bytes = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let info = figcraft_video::probe(&bytes).unwrap();
    assert_eq!((info.width, info.height, info.frames), (96, 64, 24));
    assert!((info.duration - 1.0).abs() < 0.05 && (info.fps - 24.0).abs() < 0.5, "{info:?}");
}

#[test]
fn layers_where_nothing_moves_are_refused() {
    let mut s = Session::new();
    let r = s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 40, "height": 40})).unwrap();
    let out = std::env::temp_dir().join("figcraft-still.mp4");
    let e = s.execute("file.exportVideo", &json!({"ids": [r["id"]], "path": out.to_string_lossy()})).unwrap_err();
    assert!(e.to_string().contains("nothing in these layers moves"), "{e}");
}
