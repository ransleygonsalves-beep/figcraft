//! The MP4 container: what a file holds (its first H.264 video track) and where each frame's
//! data lies, without decoding anything.

use std::io::Cursor;
use std::sync::Arc;

use mp4::{MediaType, Mp4Reader, TrackType};

use crate::{MAX_FRAMES, MAX_SIDE};

/// What an MP4's video track is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    /// Seconds.
    pub duration: f64,
    /// Frames per second (on average).
    pub fps: f64,
    pub frames: u32,
}

/// What a video with B-frames says: the decoder (Cisco's OpenH264, made for video calls) reads
/// H.264 without them only.
pub const B_FRAMES: &str = "this video uses B-frames, which FigCraft can't play yet: export it again as H.264 without B-frames (in HandBrake: Video, then Encoder options, Profile Main and \"bframes=0\" in Advanced; in ffmpeg: -bf 0)";

/// Whether `bytes` look like an MP4 (or QuickTime) file: an `ftyp` box first.
pub fn is_mp4(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp")
}

/// What `bytes` (an MP4) hold, read from its boxes; an error says why it can't be used.
pub fn probe(bytes: &[u8]) -> Result<Info, String> {
    Clip::open(Arc::new(bytes.to_vec())).map(|c| c.info)
}

/// A clip's bytes, shared, as a reader reads them.
#[derive(Clone, Debug)]
pub(crate) struct Shared(pub(crate) Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A parsed MP4 kept open (reading a sample is then cheap: parsing the file isn't).
pub(crate) type Reader = Mp4Reader<Cursor<Shared>>;

/// An MP4's H.264 video track: its settings and each frame's place in the file.
#[derive(Clone, Debug)]
pub struct Clip {
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) track: u32,
    pub info: Info,
    /// Each sample (frame, in decoding order): when it shows (seconds) and whether decoding may
    /// start there (a key frame).
    pub(crate) samples: Vec<(f64, bool)>,
    /// The track's parameter sets (without start codes).
    pub(crate) sps: Vec<u8>,
    pub(crate) pps: Vec<u8>,
}

impl Clip {
    /// The first H.264 video track of `bytes`.
    pub fn open(bytes: Arc<Vec<u8>>) -> Result<Clip, String> {
        if !is_mp4(&bytes) {
            return Err("this isn't an MP4 video".into());
        }
        let size = bytes.len() as u64;
        let mut reader = Mp4Reader::read_header(Cursor::new(bytes.as_slice()), size).map_err(|e| format!("the video can't be read: {e}"))?;
        let found = reader.tracks().values().find(|t| t.track_type().ok() == Some(TrackType::Video)).map(|t| {
            let avc = t.media_type().ok() == Some(MediaType::H264);
            let ps = (t.sequence_parameter_set().map(<[u8]>::to_vec), t.picture_parameter_set().map(<[u8]>::to_vec));
            (t.track_id(), avc, u32::from(t.width()), u32::from(t.height()), t.timescale(), t.sample_count(), ps)
        });
        let Some((track, avc, width, height, timescale, count, (sps, pps))) = found else {
            return Err("the file has no video track".into());
        };
        if !avc {
            return Err("only H.264 videos can be used (the most common kind: re-export it as H.264 MP4)".into());
        }
        let (Ok(sps), Ok(pps)) = (sps, pps) else { return Err("the video's H.264 settings are missing".into()) };
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(format!("the video is {width} × {height}: at most {MAX_SIDE} pixels a side"));
        }
        if count == 0 || count > MAX_FRAMES {
            return Err(format!("the video has {count} frames: 1 to {MAX_FRAMES} can be used"));
        }
        let scale = f64::from(timescale.max(1));
        let mut samples = Vec::with_capacity(count as usize);
        let mut end = 0.0_f64;
        let mut reordered = false;
        for id in 1..=count {
            // Only the timing is read here (the sample's data is read again when it decodes).
            let s = reader.read_sample(track, id).map_err(|e| format!("the video can't be read: {e}"))?.ok_or("the video is cut short")?;
            // Frames shown in another order than they decode: B-frames.
            reordered |= s.rendering_offset != 0;
            let shows = (s.start_time as f64 + f64::from(s.rendering_offset)) / scale;
            end = end.max((s.start_time as f64 + f64::from(s.duration)) / scale);
            samples.push((shows, s.is_sync));
        }
        if reordered {
            return Err(B_FRAMES.into());
        }
        if !samples.first().is_some_and(|s| s.1) {
            return Err("the video doesn't start with a key frame".into());
        }
        let duration = end.max(1.0 / 1000.0);
        let info = Info { width, height, duration, fps: f64::from(count) / duration, frames: count };
        Ok(Clip { bytes, track, info, samples, sps, pps })
    }

    /// The frame (in showing order) to show `t` seconds in (looping past the end).
    pub fn frame_at(&self, t: f64) -> usize {
        let t = if t.is_finite() && t > 0.0 { t % self.info.duration } else { 0.0 };
        let i = (t * self.info.fps).floor();
        if i.is_finite() { (i as usize).min(self.samples.len() - 1) } else { 0 }
    }

    /// The file parsed once, to read samples from ([`Clip::annex_b`]).
    pub(crate) fn reader(&self) -> Result<Reader, String> {
        let size = self.bytes.len() as u64;
        Mp4Reader::read_header(Cursor::new(Shared(self.bytes.clone())), size).map_err(|e| format!("the video can't be read: {e}"))
    }

    /// The H.264 data of sample `i` (decoding order, from 0) in Annex B form (start codes), what
    /// the decoder reads: the file keeps each unit behind its 4-byte length instead.
    pub(crate) fn annex_b(&self, reader: &mut Reader, i: usize) -> Result<Vec<u8>, String> {
        let id = u32::try_from(i + 1).map_err(|_| "no such frame")?;
        let s = reader.read_sample(self.track, id).map_err(|e| e.to_string())?.ok_or("the video is cut short")?;
        let mut out = Vec::with_capacity(s.bytes.len() + 16);
        let mut rest: &[u8] = &s.bytes;
        while rest.len() >= 4 {
            let n = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            let unit = rest.get(4..4 + n).ok_or("a frame's data is cut short")?;
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(unit);
            rest = &rest[4 + n..];
        }
        Ok(out)
    }

    /// The parameter sets as Annex B (sent before decoding from a key frame).
    pub(crate) fn parameter_sets(&self) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1];
        out.extend_from_slice(&self.sps);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&self.pps);
        out
    }

    /// Samples in showing order: for showing-order frame `k`, its sample (decoding order).
    pub(crate) fn showing_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.samples.len()).collect();
        order.sort_by(|a, b| self.samples[*a].0.total_cmp(&self.samples[*b].0));
        order
    }
}
