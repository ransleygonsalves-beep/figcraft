//! `file.exportVideo`, `video.exportStatus` and `video.cancelExport`: layers as an MP4 (see
//! [`crate::video_export`]). The browser has no video export yet: they say so.

use serde_json::{Value, json};

use super::{CommandSpec, Result, Session, bad, spec, string};
#[cfg(not(target_arch = "wasm32"))]
use super::{EngineError, num, targets};

#[cfg(not(target_arch = "wasm32"))]
fn export_video(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::video_export::{self as ve, Export, codec};
    const CMD: &str = "file.exportVideo";
    let path = string(p, "path").ok_or_else(|| bad(CMD, "give the file to write (`path`, .mp4)"))?;
    let path = crate::file_access::allow(&path, crate::file_access::Access::Write).map_err(|e| bad(CMD, e.to_string()))?;
    let ids = targets(s, p);
    if ids.is_empty() {
        return Err(bad(CMD, "select the layers to export (or pass `ids`)"));
    }
    if ve::running() {
        return Err(EngineError::Disabled(CMD.into(), "a video is being exported already: wait for it, or cancel it".into()));
    }
    let doc = s.doc()?;
    let setting = figcraft_doc::ExportSetting::default();
    let area = super::export_area(doc, &ids, &setting).ok_or_else(|| bad(CMD, "these layers draw nothing"))?;
    let drawn = super::drawn_layers(doc, &ids, &setting);
    let (longest, rate) = ve::timing(doc, &ids)
        .ok_or_else(|| EngineError::Disabled(CMD.into(), "nothing in these layers moves: add a video or an animated GIF to export an MP4".into()))?;
    let seconds = num(p, "seconds").filter(|v| v.is_finite() && *v > 0.0).unwrap_or(longest).min(ve::MAX_FRAMES as f64 / rate);
    let fps = num(p, "fps").filter(|v| v.is_finite()).map_or(rate, |v| v.clamp(1.0, 60.0));
    let scale = num(p, "scale").filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0).min(4.0);
    if !codec::ready() {
        return Err(EngineError::Disabled(
            CMD.into(),
            "video export needs Cisco's H.264 codec, about 0.5 MB, downloaded once: allow it in the app (Export as MP4)".into(),
        ));
    }
    let export =
        Export { doc_uid: s.ds()?.uid, doc: s.ds()?.doc.clone(), ids: drawn, area, scale, seconds, fps, path: path.to_string_lossy().to_string() };
    let frames = (seconds * fps).ceil() as usize;
    ve::start(export);
    if crate::params::boolean(p, "wait").unwrap_or(false) {
        // Scripts and agents: the outcome, once it's there.
        loop {
            match ve::take_finished() {
                Some(ve::Job::Done { path, frames, seconds, .. }) => return Ok(json!({"path": path, "frames": frames, "seconds": seconds})),
                Some(ve::Job::Failed { error, .. }) => return Err(EngineError::Other(error)),
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
    Ok(json!({"started": true, "frames": frames, "seconds": seconds, "fps": fps}))
}

#[cfg(not(target_arch = "wasm32"))]
fn export_status(_: &mut Session, _: &Value) -> Result<Value> {
    use crate::video_export::{self as ve, Job, Phase};
    Ok(match ve::status() {
        None => json!({"running": false}),
        Some(Job::Running { phase: Phase::Downloading { done, total }, .. }) => {
            json!({"running": true, "phase": "downloading", "done": done, "total": total})
        }
        Some(Job::Running { phase: Phase::Encoding { done, total }, .. }) => {
            json!({"running": true, "phase": "encoding", "done": done, "total": total})
        }
        Some(Job::Done { .. } | Job::Failed { .. }) => match ve::take_finished() {
            Some(Job::Done { path, frames, seconds, .. }) => {
                json!({"running": false, "done": true, "path": path, "frames": frames, "seconds": seconds})
            }
            Some(Job::Failed { error, .. }) => json!({"running": false, "error": error}),
            _ => json!({"running": false}),
        },
    })
}

#[cfg(target_arch = "wasm32")]
fn export_video(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("file.exportVideo", "video export works in the desktop app"))
}

#[cfg(target_arch = "wasm32")]
fn export_status(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"running": false}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    let _ = string;
    vec![
        spec!(
            "file.exportVideo",
            "Export as MP4",
            "{ids? (default the selection), path (.mp4), seconds? (default the longest video or animated GIF in them), fps? (default the fastest video's, at most 60; 30 for GIFs), scale? (default 1, at most 4), wait? (true: return when written)}  the layers drawn over time, their videos and GIFs playing, as an H.264 MP4 (no sound; see-through areas white). Needs Cisco's OpenH264 codec, downloaded once after the user allows it in the app. Without `wait` it runs in the background: video.exportStatus",
            false,
            export_video
        ),
        spec!(
            "video.exportStatus",
            "Video export status",
            "{}  {running, phase (downloading|encoding), done, total} while it runs; once it ends, once: {done: true, path, frames, seconds} or {error}",
            false,
            export_status
        ),
        spec!("video.cancelExport", "Cancel video export", "{}  stops the running export (nothing is written)", false, |_, _| {
            #[cfg(not(target_arch = "wasm32"))]
            crate::video_export::cancel();
            Ok(json!({"cancelled": true}))
        }),
    ]
}
