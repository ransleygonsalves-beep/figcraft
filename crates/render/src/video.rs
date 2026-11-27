//! Videos in image fills: an image whose data is an MP4 shows the frame for the render's time
//! (its first frame on the canvas, playing in the presenter as animated GIFs do). Frames come
//! from [`figcraft_video::Player`] (the system's decoder, else Cisco's), with the fill's image
//! adjustments and the document's colour profile, cached a few at a time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use figcraft_color::{ColorProfile, ImageFilters};
use figcraft_doc::ImageData;
use tiny_skia::Pixmap;

/// Whether `data` is a video (an MP4) rather than a picture.
pub fn is_video(data: Option<&ImageData>) -> bool {
    data.is_some_and(|d| figcraft_video::is_mp4(&d.bytes))
}

/// What video `hash` is (size, length, frame rate), read from its boxes and kept.
pub fn info(hash: &str, data: Option<&ImageData>) -> Option<figcraft_video::Info> {
    static INFO: OnceLock<Mutex<HashMap<String, Option<figcraft_video::Info>>>> = OnceLock::new();
    let cache = INFO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(i) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(hash) {
        return *i;
    }
    let i = figcraft_video::probe(&data?.bytes).ok();
    cache.lock().unwrap_or_else(PoisonError::into_inner).insert(hash.to_string(), i);
    i
}

/// The frame of video `hash` to show `t` seconds in (looping), adjusted by `filters` and in
/// `profile`. `None` when it can't be played here (the reason: [`why_not`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn frame(hash: &str, data: Option<&ImageData>, filters: &ImageFilters, profile: ColorProfile, t: f64) -> Option<Arc<Pixmap>> {
    type Key = (String, usize, [i32; 7], ColorProfile);
    static FRAMES: OnceLock<Mutex<HashMap<Key, Arc<Pixmap>>>> = OnceLock::new();
    let k = clock(hash, data, t)?.0;
    let data = data?;
    let key = (hash.to_string(), k, crate::images::filters_key(filters), profile);
    let frames = FRAMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(p) = frames.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return Some(p.clone());
    }
    let f = with_player(hash, data, |p| p.frame(k))?;
    let mut pm = Pixmap::from_vec(f.rgba, tiny_skia::IntSize::from_wh(f.width, f.height)?)?;
    crate::images::finish_frame(&mut pm, filters, profile);
    let pm = Arc::new(pm);
    let mut c = frames.lock().unwrap_or_else(PoisonError::into_inner);
    // A few seconds' frames of a few videos: playback reads ahead one at a time.
    if c.len() >= 48 {
        c.clear();
    }
    c.insert(key, pm.clone());
    Some(pm)
}

/// The browser plays no video yet: nothing to draw.
#[cfg(target_arch = "wasm32")]
pub fn frame(_: &str, _: Option<&ImageData>, _: &ImageFilters, _: ColorProfile, _: f64) -> Option<Arc<Pixmap>> {
    None
}

/// Why video `hash` doesn't play here, when it doesn't (no codec, a codec the system lacks).
#[cfg(not(target_arch = "wasm32"))]
pub fn why_not(hash: &str, data: Option<&ImageData>) -> Option<String> {
    let data = data?;
    let mut why = None;
    let played = with_player_err(hash, data, &mut why);
    if played { None } else { why }
}

#[cfg(target_arch = "wasm32")]
pub fn why_not(_: &str, _: Option<&ImageData>) -> Option<String> {
    Some("videos play in the desktop app for now".into())
}

#[cfg(not(target_arch = "wasm32"))]
type Players = HashMap<String, Result<Arc<Mutex<figcraft_video::Player>>, String>>;

#[cfg(not(target_arch = "wasm32"))]
fn players() -> &'static Mutex<Players> {
    static P: OnceLock<Mutex<Players>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Run `f` on video `hash`'s player, opened on first use (one per video: playback reads on from
/// the last frame). A video that can't open is remembered, until [`forget_failures`].
#[cfg(not(target_arch = "wasm32"))]
fn with_player<T>(hash: &str, data: &ImageData, f: impl FnOnce(&mut figcraft_video::Player) -> Result<T, String>) -> Option<T> {
    let player = open(hash, data).ok()?;
    let mut p = player.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut p).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn with_player_err(hash: &str, data: &ImageData, why: &mut Option<String>) -> bool {
    match open(hash, data) {
        Ok(_) => true,
        Err(e) => {
            *why = Some(e);
            false
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open(hash: &str, data: &ImageData) -> Result<Arc<Mutex<figcraft_video::Player>>, String> {
    let mut all = players().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(r) = all.get(hash) {
        return r.clone();
    }
    let r = figcraft_video::Clip::open(data.bytes.clone()).and_then(figcraft_video::Player::new).map(|p| Arc::new(Mutex::new(p)));
    // Too many open at once: start over (each holds a decoder).
    if all.len() >= 32 {
        all.clear();
    }
    all.insert(hash.to_string(), r.clone());
    r
}

/// Try again the videos that couldn't open (the codec arrived since).
pub fn forget_failures() {
    #[cfg(not(target_arch = "wasm32"))]
    players().lock().unwrap_or_else(PoisonError::into_inner).retain(|_, r| r.is_ok());
}

/// For the presenter's clock: the frame video `hash` shows `t` seconds in (as a key) and the
/// seconds until the next one.
pub fn clock(hash: &str, data: Option<&ImageData>, t: f64) -> Option<(usize, f64)> {
    let i = info(hash, data)?;
    let t = if t.is_finite() && t > 0.0 { t % i.duration } else { 0.0 };
    let k = (t * i.fps).floor().clamp(0.0, f64::from(i.frames.saturating_sub(1)));
    let next = (k + 1.0) / i.fps - t;
    Some((k as usize, next.max(0.001)))
}
