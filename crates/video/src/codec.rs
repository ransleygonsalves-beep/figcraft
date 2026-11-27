//! H.264 through Cisco's OpenH264 library: decoding a clip's frames ([`Player`]) and encoding new
//! ones into an MP4 ([`Writer`]). The library is a file the app downloaded from Cisco; it is
//! checked against the builds the bindings know before it is loaded.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use bytes::Bytes;
use mp4::{AvcConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType};
use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{Encoder, EncoderConfig, FrameRate, FrameType, QpRange, RateControlMode};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};

use crate::container::Reader;
use crate::{Clip, Frame, MAX_SIDE};

/// The OpenH264 library to load, once one is set.
static CODEC: Mutex<Option<PathBuf>> = Mutex::new(None);

/// What asking for the codec before it is there says.
const NO_CODEC: &str = "video needs the H.264 codec, which hasn't been downloaded yet";

/// Use the OpenH264 library at `path` (Cisco's build for this system): it is loaded once here to
/// check it, then whenever a clip decodes or a video encodes.
pub fn set_codec(path: impl Into<PathBuf>) -> Result<(), String> {
    let path = path.into();
    OpenH264API::from_blob_path(&path).map_err(|e| format!("the H.264 codec can't be used: {e}"))?;
    *CODEC.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
    Ok(())
}

/// Whether a codec is set ([`set_codec`]).
pub fn codec_ready() -> bool {
    CODEC.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}

fn api() -> Result<OpenH264API, String> {
    let path = CODEC.lock().unwrap_or_else(PoisonError::into_inner).clone().ok_or(NO_CODEC)?;
    OpenH264API::from_blob_path(path).map_err(|e| format!("the H.264 codec can't be used: {e}"))
}

/// A clip's frames on demand: through the operating system's own decoder where there is one
/// (any video it plays), else Cisco's OpenH264 (H.264 without B-frames).
pub struct Player {
    clip: Clip,
    inner: Inner,
}

enum Inner {
    #[cfg(windows)]
    System(crate::system_windows::SystemVideo),
    Codec(Box<H264Player>),
}

impl Player {
    pub fn new(clip: Clip) -> Result<Player, String> {
        #[cfg(windows)]
        match crate::system_windows::SystemVideo::open(clip.bytes.clone()) {
            Ok(v) => return Ok(Player { clip, inner: Inner::System(v) }),
            Err(e) if clip.needs_system => return Err(e),
            Err(_) => {}
        }
        if clip.needs_system {
            return Err(crate::B_FRAMES.into());
        }
        let p = H264Player::new(clip.clone())?;
        Ok(Player { clip, inner: Inner::Codec(Box::new(p)) })
    }

    pub fn clip(&self) -> &Clip {
        &self.clip
    }

    /// Whether the operating system decodes it.
    pub fn system(&self) -> bool {
        !matches!(self.inner, Inner::Codec(_))
    }

    /// The frame to show `t` seconds in (looping).
    pub fn frame_at(&mut self, t: f64) -> Result<Frame, String> {
        let k = self.clip.frame_at(t);
        self.frame(k)
    }

    /// Frame `k` in showing order.
    pub fn frame(&mut self, k: usize) -> Result<Frame, String> {
        match &mut self.inner {
            #[cfg(windows)]
            // The middle of the frame's time, so rounding never lands on its neighbour.
            Inner::System(v) => v.frame_at((k as f64 + 0.5) / self.clip.info.fps),
            Inner::Codec(p) => p.frame(k),
        }
    }
}

/// Cisco's decoder over a clip. Decoding starts at the key frame at or before the frame asked
/// for, and carries on from where it is when the next frame asked for lies ahead (playback).
struct H264Player {
    clip: Clip,
    reader: Reader,
    /// Each sample's place in showing order.
    rank: Vec<usize>,
    decoder: Option<Decoder>,
    /// The next sample (decoding order) to feed the decoder.
    next: usize,
    /// The showing-order frame the decoder outputs next.
    shown: usize,
    last: Option<(usize, Frame)>,
}

impl H264Player {
    fn new(clip: Clip) -> Result<H264Player, String> {
        let order = clip.showing_order();
        let mut rank = vec![0; order.len()];
        for (k, s) in order.iter().enumerate() {
            rank[*s] = k;
        }
        let reader = clip.reader()?;
        Ok(H264Player { clip, reader, rank, decoder: None, next: 0, shown: 0, last: None })
    }

    /// Frame `k` in showing order.
    fn frame(&mut self, k: usize) -> Result<Frame, String> {
        if let Some((i, f)) = &self.last
            && *i == k
        {
            return Ok(f.clone());
        }
        // Where decoding has to start: the last key frame showing at or before `k`.
        let start = (0..self.clip.samples.len()).filter(|s| self.clip.samples[*s].1 && self.rank[*s] <= k).max_by_key(|s| self.rank[*s]).unwrap_or(0);
        let ahead = self.decoder.is_some() && self.shown <= k && self.rank[start] <= self.shown;
        if !ahead {
            let mut d = Decoder::with_api_config(api()?, DecoderConfig::new()).map_err(|e| format!("the video can't be decoded: {e}"))?;
            let _ = d.decode(&self.clip.parameter_sets());
            self.decoder = Some(d);
            self.next = start;
            self.shown = self.rank[start];
        }
        loop {
            let Some(d) = self.decoder.as_mut() else { return Err(NO_CODEC.into()) };
            let outputs: Vec<Frame> = if self.next < self.clip.samples.len() {
                let data = self.clip.annex_b(&mut self.reader, self.next)?;
                self.next += 1;
                match d.decode(&data) {
                    Ok(Some(yuv)) => vec![to_frame(&yuv)],
                    Ok(None) => Vec::new(),
                    // A damaged frame: the ones after it may still come out.
                    Err(_) => Vec::new(),
                }
            } else {
                let rest: Vec<Frame> = d.flush_remaining().map_err(|e| format!("the video can't be decoded: {e}"))?.iter().map(to_frame).collect();
                if rest.is_empty() {
                    self.decoder = None;
                    return self.last.as_ref().map(|(_, f)| f.clone()).ok_or_else(|| "no frame of the video could be decoded".into());
                }
                rest
            };
            for f in outputs {
                let i = self.shown;
                self.shown += 1;
                if i >= k {
                    self.last = Some((i, f.clone()));
                    return Ok(f);
                }
                self.last = Some((i, f));
            }
        }
    }
}

fn to_frame(yuv: &openh264::decoder::DecodedYUV<'_>) -> Frame {
    let (w, h) = yuv.dimensions();
    let mut rgba = vec![0; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    Frame { width: w as u32, height: h as u32, rgba }
}

/// Encodes frames into a new MP4 (H.264, no sound). Frames are cropped to even sides, which
/// H.264's colour layout needs.
pub struct Writer {
    encoder: Encoder,
    width: u32,
    height: u32,
    /// Frames per second, and the file's time units per second.
    fps: f64,
    timescale: u32,
    /// Each frame's data (length-prefixed units), whether it is a key frame, and how many frame
    /// times it lasts (a frame the encoder skipped lengthens the one before).
    samples: Vec<(Vec<u8>, bool, u32)>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl Writer {
    /// A video of `width` × `height` (each made even) at `fps` frames per second.
    pub fn new(width: u32, height: u32, fps: f64) -> Result<Writer, String> {
        let (width, height) = (width & !1, height & !1);
        if width < 2 || height < 2 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(format!("a video can be 2 to {MAX_SIDE} pixels a side, not {width} × {height}"));
        }
        let fps = if fps.is_finite() { fps.clamp(1.0, 120.0) } else { 30.0 };
        // Even quality, every frame kept: a narrow quantiser range, no bitrate target (which would
        // drop frames to meet it). Sharp enough for interface text.
        let config = EncoderConfig::new()
            .rate_control_mode(RateControlMode::Off)
            .qp(QpRange::new(18, 24))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .skip_frames(false);
        let encoder = Encoder::with_api_config(api()?, config).map_err(|e| format!("the video can't be encoded: {e}"))?;
        Ok(Writer { encoder, width, height, fps, timescale: (fps * 1000.0).round() as u32, samples: Vec::new(), sps: None, pps: None })
    }

    /// The size frames are written at.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Add `frame` (RGBA, at least the writer's size; anything past it is cropped).
    pub fn push(&mut self, frame: &Frame) -> Result<(), String> {
        if frame.width < self.width || frame.height < self.height || frame.rgba.len() < (frame.width * frame.height * 4) as usize {
            return Err("a frame is smaller than the video".into());
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let cropped: Vec<u8> = if frame.width == self.width {
            frame.rgba[..w * h * 4].to_vec()
        } else {
            (0..h).flat_map(|y| frame.rgba[y * frame.width as usize * 4..][..w * 4].iter().copied()).collect()
        };
        let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&cropped, (w, h)));
        let bs = self.encoder.encode(&yuv).map_err(|e| format!("the video can't be encoded: {e}"))?;
        let key = matches!(bs.frame_type(), FrameType::IDR);
        let mut data = Vec::new();
        for l in 0..bs.num_layers() {
            let Some(layer) = bs.layer(l) else { continue };
            for n in 0..layer.nal_count() {
                let Some(unit) = layer.nal_unit(n).map(strip_start_code) else { continue };
                match unit.first().map(|b| b & 0x1f) {
                    Some(7) => {
                        self.sps.get_or_insert_with(|| unit.to_vec());
                    }
                    Some(8) => {
                        self.pps.get_or_insert_with(|| unit.to_vec());
                    }
                    Some(_) => {
                        data.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                        data.extend_from_slice(unit);
                    }
                    None => {}
                }
            }
        }
        match self.samples.last_mut() {
            Some(last) if data.is_empty() => last.2 += 1,
            _ if data.is_empty() => {}
            _ => self.samples.push((data, key, 1)),
        }
        Ok(())
    }

    /// The MP4 file.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        let (Some(sps), Some(pps)) = (self.sps, self.pps) else { return Err("no frames were encoded".into()) };
        let brand = |s: &str| s.parse::<FourCC>().map_err(|e| e.to_string());
        let config = Mp4Config {
            major_brand: brand("isom")?,
            minor_version: 512,
            compatible_brands: vec![brand("isom")?, brand("iso2")?, brand("avc1")?, brand("mp41")?],
            timescale: 1000,
        };
        let failed = |e: mp4::Error| format!("the video can't be written: {e}");
        let mut w = Mp4Writer::write_start(Cursor::new(Vec::new()), &config).map_err(failed)?;
        let avc = AvcConfig { width: self.width as u16, height: self.height as u16, seq_param_set: sps, pic_param_set: pps };
        w.add_track(&TrackConfig {
            track_type: TrackType::Video,
            timescale: self.timescale,
            language: "und".into(),
            media_conf: MediaConfig::AvcConfig(avc),
        })
        .map_err(failed)?;
        // Each frame lasts 1000 units of the track's `fps × 1000` per second.
        let mut start = 0u64;
        for (data, key, frames) in self.samples {
            let duration = 1000 * frames;
            w.write_sample(1, &Mp4Sample { start_time: start, duration, rendering_offset: 0, is_sync: key, bytes: Bytes::from(data) })
                .map_err(failed)?;
            start += u64::from(duration);
        }
        w.write_end().map_err(failed)?;
        let _ = self.fps;
        Ok(w.into_writer().into_inner())
    }
}

/// A unit without the `00 00 01` or `00 00 00 01` before it.
fn strip_start_code(unit: &[u8]) -> &[u8] {
    if unit.starts_with(&[0, 0, 0, 1]) {
        &unit[4..]
    } else if unit.starts_with(&[0, 0, 1]) {
        &unit[3..]
    } else {
        unit
    }
}
