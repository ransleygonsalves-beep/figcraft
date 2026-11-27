//! Video for FigCraft: MP4 files with an H.264 video track (what phones, screen recorders and
//! editors write by default).
//!
//! - [`probe`] reads a file's size, length and frame rate from its boxes alone, anywhere (the
//!   browser too).
//! - Decoding and encoding H.264 go through Cisco's OpenH264 library, which the app downloads from
//!   Cisco the first time it is needed (Cisco's build carries its H.264 patent licence; nothing of
//!   H.264 is compiled into FigCraft). [`set_codec`] names that library; until then
//!   [`codec_ready`] is false and [`Player`] and [`Writer`] say why they can't start.
//! - [`Player`] gives a clip's frame at any time, decoding forward from the key frame before it.
//! - [`Writer`] encodes frames into a new MP4.
//!
//! Sound is left out: frames only, in and out.

#[cfg(not(target_arch = "wasm32"))]
mod codec;
mod container;

#[cfg(not(target_arch = "wasm32"))]
pub use codec::{Player, Writer, codec_ready, set_codec};
pub use container::{B_FRAMES, Clip, Info, is_mp4, probe};

/// The largest frame side taken (pixels): H.264's level 5.2 tops out about here.
pub const MAX_SIDE: u32 = 8192;
/// The most frames one clip may have (an hour at 60 per second).
pub const MAX_FRAMES: u32 = 216_000;

/// One frame: `width` × `height` pixels, RGBA, unpremultiplied (video has no transparency: alpha
/// is always 255).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the browser build says where a codec is needed.
#[cfg(target_arch = "wasm32")]
pub fn codec_ready() -> bool {
    false
}

#[cfg(test)]
mod tests;
