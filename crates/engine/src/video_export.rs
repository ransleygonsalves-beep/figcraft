//! Exporting layers as an MP4 video: the layers drawn over time (the videos and animated GIFs in
//! them playing) for as long as the longest of those runs, encoded as H.264.
//!
//! Encoding goes through Cisco's OpenH264 library (Cisco's build carries the H.264 patent
//! licence). It is downloaded from Cisco once, after the user allowed it in the app (as with the
//! background removal model, no command turns downloading on), into the suite's shared model
//! folder, checked against its pinned SHA-256 and against the builds the bindings know. Playback
//! doesn't need it on Windows and macOS (their own decoders play videos).
//!
//! `file.exportVideo` starts the job on a thread of its own; `video.exportStatus` reports its
//! progress and, once, its outcome; `video.cancelExport` stops it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use figcraft_doc::{Document, NodeId};
use figcraft_geom::Rect;

/// Where the job is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Getting the codec: bytes so far and in all.
    Downloading { done: u64, total: u64 },
    /// Frames encoded so far and in all.
    Encoding { done: usize, total: usize },
}

/// The process's one export, for document `doc` (its uid).
#[derive(Clone, Debug)]
pub enum Job {
    Running { doc: u64, phase: Phase },
    Done { doc: u64, path: String, frames: usize, seconds: f64 },
    Failed { doc: u64, error: String },
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn set(j: Job) {
    *JOB.lock().unwrap_or_else(PoisonError::into_inner) = Some(j);
}

/// The job as it is now, without changing it.
pub fn status() -> Option<Job> {
    JOB.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Take a finished or failed job, leaving none.
pub fn take_finished() -> Option<Job> {
    let mut slot = JOB.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(slot.as_ref(), Some(Job::Done { .. } | Job::Failed { .. })) { slot.take() } else { None }
}

/// Stop the running job (its file isn't written).
pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

pub fn running() -> bool {
    matches!(status(), Some(Job::Running { .. }))
}

/// The most frames one export writes (ten minutes at 60 per second).
pub const MAX_FRAMES: usize = 36_000;

/// What exporting `ids` takes: how long (the longest video or animated GIF in them) and at what
/// rate (the fastest video's, at most 60; 30 for GIFs alone). `None`: nothing in them moves.
pub fn timing(doc: &Document, ids: &[NodeId]) -> Option<(f64, f64)> {
    let mut longest: Option<f64> = None;
    let mut fps: Option<f64> = None;
    for id in ids.iter().flat_map(|i| doc.subtree(*i)) {
        let Some(n) = doc.node(id).filter(|n| n.visible) else { continue };
        for p in n.fills.iter().chain(&n.strokes).filter(|p| p.visible) {
            let figcraft_color::PaintKind::Image(img) = &p.kind else { continue };
            let data = doc.images.get(&img.image);
            if let Some(i) = figcraft_render::video::info(&img.image, data) {
                longest = Some(longest.unwrap_or(0.0).max(i.duration));
                fps = Some(fps.unwrap_or(0.0).max(i.fps));
            } else if let Some(a) = figcraft_render::images::animation(&img.image, data, &img.filters, doc.color_profile) {
                longest = Some(longest.unwrap_or(0.0).max(a.duration()));
            }
        }
    }
    Some((longest?, fps.unwrap_or(30.0).clamp(1.0, 60.0)))
}

/// One export: document `doc_uid`'s `doc` (a snapshot), its layers `ids` over page `area` at
/// `scale`, for `seconds` at `fps`, into `path`.
pub struct Export {
    pub doc_uid: u64,
    pub doc: Arc<Document>,
    pub ids: Vec<NodeId>,
    pub area: Rect,
    pub scale: f64,
    pub seconds: f64,
    pub fps: f64,
    pub path: String,
}

/// Run `e` on a thread of its own (the codec downloads first if it isn't here).
pub fn start(e: Export) {
    CANCEL.store(false, Ordering::Relaxed);
    set(Job::Running { doc: e.doc_uid, phase: Phase::Encoding { done: 0, total: 0 } });
    // File access confined for the caller (an agent's roots) stays confined on the thread.
    let roots = crate::file_access::current();
    std::thread::spawn(move || {
        let work = std::panic::AssertUnwindSafe(|| crate::file_access::confined(roots.as_ref(), || run(&e)));
        set(match std::panic::catch_unwind(work) {
            Ok(Ok(frames)) => Job::Done { doc: e.doc_uid, path: e.path.clone(), frames, seconds: e.seconds },
            Ok(Err(error)) => Job::Failed { doc: e.doc_uid, error },
            Err(_) => Job::Failed { doc: e.doc_uid, error: "the video export stopped unexpectedly".into() },
        });
    });
}

fn run(e: &Export) -> Result<usize, String> {
    let Export { doc_uid, ref doc, ref ids, area, scale, seconds, fps, ref path } = *e;
    codec::ensure(&|done, total| set(Job::Running { doc: doc_uid, phase: Phase::Downloading { done, total } }))?;
    let total = ((seconds * fps).ceil() as usize).clamp(1, MAX_FRAMES);
    let srgb = doc.in_srgb();
    let mut writer: Option<figcraft_video::Writer> = None;
    for i in 0..total {
        if CANCEL.load(Ordering::Relaxed) {
            return Err("the export was cancelled".into());
        }
        set(Job::Running { doc: doc_uid, phase: Phase::Encoding { done: i, total } });
        // The middle of each frame's time, so a video's frame boundary never rounds the wrong way.
        let t = (i as f64 + 0.5) / fps;
        let pix = figcraft_render::render_area_at(&srgb, ids, area, scale, t).ok_or("a frame couldn't be drawn (empty or too large)")?;
        let w = match writer.as_mut() {
            Some(w) => w,
            None => writer.insert(figcraft_video::Writer::new(pix.width(), pix.height(), fps)?),
        };
        // Video has no transparency: what's see-through shows white.
        let mut rgba = figcraft_render::to_rgba8(&pix);
        for px in rgba.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
            }
            px[3] = 255;
        }
        w.push(&figcraft_video::Frame { width: pix.width(), height: pix.height(), rgba })?;
    }
    let bytes = writer.ok_or("no frames were drawn")?.finish()?;
    crate::file_access::write(std::path::Path::new(path), &bytes).map_err(|e| format!("couldn't write {path}: {e}"))?;
    Ok(total)
}

/// Cisco's OpenH264 library: where it goes, downloading and loading it.
pub mod codec {
    use std::path::PathBuf;

    use crate::file_access;

    /// Cisco's build for this system: its download (bzip2), the SHA-256 of that download, the
    /// library's file name and the download's.
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    const BUILD: Option<(&str, &str, &str, &str)> = Some((
        "http://ciscobinary.openh264.org/openh264-2.6.0-win64.dll.bz2",
        "dab5f2a872777f9a58b69bfa9fbcf20d9f82f2d6ec91383fd70bff49bd34ac9f",
        "openh264-2.6.0-win64.dll",
        "openh264-2.6.0-win64.dll.bz2",
    ));
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    const BUILD: Option<(&str, &str, &str, &str)> = Some((
        "http://ciscobinary.openh264.org/libopenh264-2.6.0-mac-arm64.dylib.bz2",
        "6db362ee5abdab572311aeadb96d3f44b0617d9a4a4b9f4db4cb5ac4d968da71",
        "libopenh264-2.6.0-mac-arm64.dylib",
        "libopenh264-2.6.0-mac-arm64.dylib.bz2",
    ));
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    const BUILD: Option<(&str, &str, &str, &str)> = Some((
        "http://ciscobinary.openh264.org/libopenh264-2.6.0-mac-x64.dylib.bz2",
        "38b2ed6d1d45b6a3e408c734173f2d67ab44a10d0e154ff3489b89877cd60e7e",
        "libopenh264-2.6.0-mac-x64.dylib",
        "libopenh264-2.6.0-mac-x64.dylib.bz2",
    ));
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const BUILD: Option<(&str, &str, &str, &str)> = Some((
        "http://ciscobinary.openh264.org/libopenh264-2.6.0-linux64.8.so.bz2",
        "27ab53323c110b76214c1c72222f459d17febbcd1e252136cadc292b0308d75b",
        "libopenh264-2.6.0-linux64.8.so",
        "libopenh264-2.6.0-linux64.8.so.bz2",
    ));
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", any(target_arch = "aarch64", target_arch = "x86_64")),
        all(target_os = "linux", target_arch = "x86_64")
    )))]
    const BUILD: Option<(&str, &str, &str, &str)> = None;

    /// What the window asking to download it says of it.
    pub const NOTICE: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";
    pub const SIZE_KB: u64 = 500;

    fn dir() -> Option<PathBuf> {
        if cfg!(test) {
            return Some(std::env::temp_dir().join(format!("figcraft-codec-tests-{}", std::process::id())));
        }
        craft_ai::probe::models_dir()
    }

    fn consent() -> Option<PathBuf> {
        Some(dir()?.join("figcraft-openh264.allowed"))
    }

    /// Whether the codec may be used: it is set up already, or the user allowed the download.
    pub fn ready() -> bool {
        let allowed = || consent().is_some_and(|f| file_access::app_owned(|| file_access::metadata(&f)).is_ok_and(|m| m.is_file()));
        figcraft_video::codec_ready() || allowed()
    }

    /// Allow downloading the codec. Only from the user's own action in the app's window.
    pub fn allow_download() -> Result<(), String> {
        let file = consent().ok_or_else(|| "this system has no folder for downloads like this".to_string())?;
        file_access::app_owned(|| {
            if let Some(d) = file.parent() {
                file_access::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            file_access::write(&file, b"The user allowed FigCraft to download Cisco's OpenH264 video codec.\n").map_err(|e| e.to_string())
        })
    }

    /// Set the codec up: loaded already, on disk, or downloaded now (`progress`: bytes so far, in
    /// all). Refused without the user's go-ahead.
    pub fn ensure(progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        if figcraft_video::codec_ready() {
            return Ok(());
        }
        let (url, sha, file, packed_file) =
            BUILD.ok_or("there's no H.264 codec for this system: video export works on Windows, macOS and Linux (x86-64)")?;
        let dir = dir().ok_or("this system has no folder for downloads like this")?;
        let lib = dir.join(file);
        let on_disk = file_access::app_owned(|| file_access::metadata(&lib)).is_ok_and(|m| m.is_file());
        if !on_disk {
            if !ready() {
                return Err("video export needs Cisco's H.264 codec: allow its download in the app first".into());
            }
            let entry = craft_ai::ModelEntry {
                id: "openh264",
                name: "OpenH264 (Cisco)",
                url,
                file: packed_file,
                tokenizer_url: "",
                sha256: Some(sha),
                size_mb: 1,
                min_ram_gb: 0.0,
                rec_ram_gb: 0.0,
                license: "BSD-2-Clause (Cisco's binary licence)",
                quality: craft_ai::Quality::Small,
                arch: craft_ai::Arch::Qwen2,
                langs: "",
            };
            let on = |e: &craft_ai::fetch::Event| {
                if let craft_ai::fetch::Event::Progress { done, total, .. } = e {
                    progress(*done, *total);
                }
            };
            let bz2 = craft_ai::fetch::download(&entry, &dir, on, None).map_err(|e| format!("the H.264 codec couldn't be downloaded: {e}"))?;
            let packed =
                file_access::app_owned(|| file_access::read_capped(&bz2, 16 << 20)).map_err(|e| format!("the codec couldn't be read: {e}"))?;
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut bzip2_rs::DecoderReader::new(packed.as_slice()), &mut out)
                .map_err(|e| format!("the codec couldn't be unpacked: {e}"))?;
            file_access::app_owned(|| file_access::write(&lib, &out)).map_err(|e| format!("the codec couldn't be saved: {e}"))?;
        }
        // The bindings check it is one of Cisco's builds they know before loading it.
        figcraft_video::set_codec(&lib)?;
        figcraft_render::video::forget_failures();
        Ok(())
    }
}
