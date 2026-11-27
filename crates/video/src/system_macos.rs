//! macOS's own video decoding (AVFoundation): what QuickTime Player and Safari play, H.264 with
//! B-frames and HEVC included, through the machine's hardware decoder.
//!
//! AVFoundation reads from a file, so each open video's bytes are written to a temporary file
//! (removed when it closes). Its objects stay on one thread of its own, as on Windows; frames are
//! asked for exactly (no tolerance), so the one showing at the time comes back.
//!
//! The fork's one exception to `unsafe_code = "forbid"` on macOS (see CLAUDE.md, This fork):
//! the calls into AVFoundation and CoreGraphics are `unsafe` by nature; each says why it holds.
//! Not run in this repository's tests (no Mac runs them): checked on a Mac by hand.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use objc2::rc::Retained;
use objc2_av_foundation::{AVAssetImageGenerator, AVURLAsset};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_core_media::{CMTime, kCMTimeZero};
use objc2_foundation::{NSError, NSString, NSURL};

use crate::Frame;

enum Req {
    Open { id: u64, bytes: Arc<Vec<u8>>, reply: Sender<Result<(), String>> },
    Frame { id: u64, t: f64, reply: Sender<Result<Frame, String>> },
    Close { id: u64 },
}

fn worker() -> Option<Sender<Req>> {
    static TX: OnceLock<Option<Mutex<Sender<Req>>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new().name("figcraft-video".into()).spawn(move || run(&rx));
        started.ok().map(|_| Mutex::new(tx))
    })
    .as_ref()
    .map(|m| m.lock().unwrap_or_else(PoisonError::into_inner).clone())
}

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A video open on the decoding thread.
pub(crate) struct SystemVideo {
    id: u64,
    tx: Sender<Req>,
}

impl SystemVideo {
    pub(crate) fn open(bytes: Arc<Vec<u8>>) -> Result<SystemVideo, String> {
        let tx = worker().ok_or("the video thread couldn't start")?;
        let (reply, rx) = channel();
        let id = next_id();
        tx.send(Req::Open { id, bytes, reply }).map_err(|_| "the video thread stopped")?;
        rx.recv().map_err(|_| "the video thread stopped".to_string())??;
        Ok(SystemVideo { id, tx })
    }

    pub(crate) fn frame_at(&self, t: f64) -> Result<Frame, String> {
        let (reply, rx) = channel();
        self.tx.send(Req::Frame { id: self.id, t, reply }).map_err(|_| "the video thread stopped")?;
        rx.recv().map_err(|_| "the video thread stopped".to_string())?
    }
}

impl Drop for SystemVideo {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Close { id: self.id });
    }
}

struct Open {
    generator: Retained<AVAssetImageGenerator>,
    file: PathBuf,
    last: Option<(f64, Frame)>,
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

fn run(rx: &Receiver<Req>) {
    let mut open: HashMap<u64, Open> = HashMap::new();
    for req in rx {
        match req {
            Req::Open { id, bytes, reply } => {
                let _ = reply.send(open_video(id, &bytes).map(|o| drop(open.insert(id, o))));
            }
            Req::Frame { id, t, reply } => {
                let r = open.get_mut(&id).ok_or_else(|| "the video was closed".to_string()).and_then(|o| frame_at(o, t));
                let _ = reply.send(r);
            }
            Req::Close { id } => {
                open.remove(&id);
            }
        }
    }
}

fn open_video(id: u64, bytes: &[u8]) -> Result<Open, String> {
    let file = std::env::temp_dir().join(format!("figcraft-video-{}-{id}.mp4", std::process::id()));
    std::fs::write(&file, bytes).map_err(|e| format!("the video couldn't be opened: {e}"))?;
    let path = file.to_string_lossy().to_string();
    // SAFETY: plain AVFoundation calls on objects made here, on this thread; kCMTimeZero is a
    // constant the framework defines.
    let generator = unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let g = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        g.setAppliesPreferredTrackTransform(true);
        g.setRequestedTimeToleranceBefore(kCMTimeZero);
        g.setRequestedTimeToleranceAfter(kCMTimeZero);
        g
    };
    let mut o = Open { generator, file, last: None };
    // The first frame, now: a video macOS can't play says so when it opens.
    frame_at(&mut o, 0.0).map_err(|e| format!("macOS can't play this video: {e}"))?;
    Ok(o)
}

fn frame_at(o: &mut Open, t: f64) -> Result<Frame, String> {
    if let Some((at, f)) = &o.last
        && (*at - t).abs() < 1e-6
    {
        return Ok(f.clone());
    }
    // The frame comes to a block on AVFoundation's own queue, which reads its pixels there (the
    // image is only lent to the block) and hands them back.
    let (tx, rx) = channel::<Result<Frame, String>>();
    let tx = Mutex::new(Some(tx));
    let handler = block2::RcBlock::new(move |image: *mut CGImage, _: CMTime, error: *mut NSError| {
        // SAFETY: AVFoundation passes a live image (or null) and a live error (or null) for the
        // block's duration.
        let r = match unsafe { image.as_ref() } {
            Some(img) => pixels(img),
            None => {
                Err(unsafe { error.as_ref() }.map_or_else(|| "the frame couldn't be decoded".to_string(), |e| e.localizedDescription().to_string()))
            }
        };
        if let Some(tx) = tx.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = tx.send(r);
        }
    });
    // SAFETY: the generator is this thread's, and the block only sends on a channel (sendable).
    unsafe { o.generator.generateCGImageAsynchronouslyForTime_completionHandler(CMTime::with_seconds(t.max(0.0), 600), &handler) };
    let frame = rx.recv_timeout(std::time::Duration::from_secs(10)).map_err(|_| "the frame took too long to decode".to_string())??;
    o.last = Some((t, frame.clone()));
    Ok(frame)
}

/// A CGImage's pixels as RGBA rows, top down.
fn pixels(image: &CGImage) -> Result<Frame, String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 || w > crate::MAX_SIDE as usize || h > crate::MAX_SIDE as usize {
        return Err(format!("the video is {w} × {h}: at most {} pixels a side", crate::MAX_SIDE));
    }
    let mut rgba = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb().ok_or("no colour space")?;
    // SAFETY: the context draws into `rgba`, which is w × h × 4 bytes as described, and outlives it.
    unsafe {
        let ctx = CGBitmapContextCreate(rgba.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), CGImageAlphaInfo::PremultipliedLast.0)
            .ok_or("the frame couldn't be read")?;
        CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    }
    // Video is opaque: premultiplied and straight are the same.
    Ok(Frame { width: w as u32, height: h as u32, rgba })
}
