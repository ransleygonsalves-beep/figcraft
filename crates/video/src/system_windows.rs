//! Windows' own video decoding (Media Foundation's source reader): what Edge and the Films & TV
//! app play, H.264 with B-frames and HEVC (with Microsoft's HEVC extension) included, through the
//! machine's hardware decoder where it has one.
//!
//! COM objects belong to the thread that made them, and the renderer asks for frames from
//! several, so one thread of its own holds every open video and answers requests in turn.
//!
//! The fork's one exception to `unsafe_code = "forbid"` on Windows (see CLAUDE.md, This fork):
//! the calls into Media Foundation are `unsafe` by nature; each says why it holds.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION, MFCreateAttributes,
    MFCreateMFByteStreamOnStream, MFCreateMediaType, MFCreateSourceReaderFromByteStream, MFMediaType_Video, MFSTARTUP_NOSOCKET, MFStartup,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::GUID;

use crate::Frame;

/// What one open video is asked.
enum Req {
    Open { id: u64, bytes: Arc<Vec<u8>>, reply: Sender<Result<(), String>> },
    Frame { id: u64, t: f64, reply: Sender<Result<Frame, String>> },
    Close { id: u64 },
}

/// The decoding thread's inbox (started on first use).
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
    /// Open `bytes` (any video Windows plays); `Err` says why it can't.
    pub(crate) fn open(bytes: Arc<Vec<u8>>) -> Result<SystemVideo, String> {
        let tx = worker().ok_or("the video thread couldn't start")?;
        let (reply, rx) = channel();
        let id = next_id();
        tx.send(Req::Open { id, bytes, reply }).map_err(|_| "the video thread stopped")?;
        rx.recv().map_err(|_| "the video thread stopped".to_string())??;
        Ok(SystemVideo { id, tx })
    }

    /// The frame showing `t` seconds in.
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

/// A source reader and where it is.
struct Open {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    /// Bytes from one row to the next (negative: the rows run bottom up).
    stride: i32,
    /// The first frame's time (100 ns units): the file's times count from it.
    start: i64,
    /// The last two frames read, each with its time: the first is showing until the second's.
    frames: std::collections::VecDeque<(i64, Frame)>,
    /// The stream ended after the last frame read.
    ended: bool,
}

fn run(rx: &Receiver<Req>) {
    // SAFETY: called once, first thing on this thread, which no one else initialises.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: Media Foundation's start-up, with the version these bindings were made for.
    let mf = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) };
    let ready = com.is_ok() && mf.is_ok();
    let mut open: HashMap<u64, Open> = HashMap::new();
    for req in rx {
        match req {
            Req::Open { id, bytes, reply } => {
                let r =
                    if ready { open_reader(&bytes).map(|o| drop(open.insert(id, o))) } else { Err("Windows' video decoding isn't available".into()) };
                let _ = reply.send(r);
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

fn failed(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {}", e.message())
}

/// A source reader for `bytes`, giving its first video stream as 32-bit RGB.
fn open_reader(bytes: &[u8]) -> Result<Open, String> {
    // SAFETY: each call gets live COM objects made just before it on this thread (which joined
    // the multithreaded apartment), and plain values; `bytes` is copied by SHCreateMemStream.
    unsafe {
        let stream = SHCreateMemStream(Some(bytes)).ok_or("the video couldn't be read into memory")?;
        let bs = MFCreateMFByteStreamOnStream(&stream).map_err(failed("the video couldn't be opened"))?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(failed("the video couldn't be opened"))?;
        let attrs = attrs.ok_or("the video couldn't be opened")?;
        // Lets the reader convert whatever it decodes into RGB.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(failed("the video couldn't be opened"))?;
        let reader = MFCreateSourceReaderFromByteStream(&bs, &attrs).map_err(failed("Windows can't play this video"))?;
        let all = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
        let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader.SetStreamSelection(all, false).map_err(failed("the video couldn't be opened"))?;
        reader.SetStreamSelection(video, true).map_err(failed("the video has no picture"))?;
        let want = MFCreateMediaType().map_err(failed("the video couldn't be opened"))?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(failed("the video couldn't be opened"))?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(failed("the video couldn't be opened"))?;
        reader.SetCurrentMediaType(video, None, &want).map_err(failed("Windows can't decode this video (its codec may be missing)"))?;
        let got = reader.GetCurrentMediaType(video).map_err(failed("the video couldn't be opened"))?;
        let size = got.GetUINT64(&MF_MT_FRAME_SIZE).map_err(failed("the video has no size"))?;
        let (width, height) = ((size >> 32) as u32, (size & 0xffff_ffff) as u32);
        let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or((width * 4) as i32, |s| s as i32);
        if width == 0 || height == 0 || width > crate::MAX_SIDE || height > crate::MAX_SIDE {
            return Err(format!("the video is {width} × {height}: at most {} pixels a side", crate::MAX_SIDE));
        }
        let mut o = Open { reader, width, height, stride, start: 0, frames: std::collections::VecDeque::new(), ended: false };
        // The first frame says when the file's time starts.
        read_one(&mut o)?;
        o.start = o.frames.front().map_or(0, |f| f.0);
        Ok(o)
    }
}

/// Read the next frame onto `o.frames` (keeping the last two); false at the end of the stream.
fn read_one(o: &mut Open) -> Result<bool, String> {
    let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    loop {
        let (mut flags, mut stamp, mut sample) = (0u32, 0i64, None);
        // SAFETY: the out-pointers point at locals that outlive the call.
        unsafe { o.reader.ReadSample(video, 0, None, Some(&mut flags), Some(&mut stamp), Some(&mut sample)) }
            .map_err(failed("the video couldn't be decoded"))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            o.ended = true;
            return Ok(false);
        }
        let Some(sample) = sample else { continue };
        let frame = copy_frame(o, &sample)?;
        o.frames.push_back((stamp, frame));
        while o.frames.len() > 2 {
            o.frames.pop_front();
        }
        return Ok(true);
    }
}

/// The frame showing at `t` seconds (from the first frame): the last one whose time is at or
/// before it. Read on from where the reader is when that's just ahead, else seek first.
fn frame_at(o: &mut Open, t: f64) -> Result<Frame, String> {
    let target = o.start + (t.max(0.0) * 10_000_000.0) as i64;
    let showing = |o: &Open| -> Option<Frame> {
        match (o.frames.front(), o.frames.get(1)) {
            (Some(a), Some(b)) if a.0 <= target && target < b.0 => Some(a.1.clone()),
            (_, Some(b)) if o.ended && b.0 <= target => Some(b.1.clone()),
            (Some(a), None) if o.ended && a.0 <= target => Some(a.1.clone()),
            _ => None,
        }
    };
    if let Some(f) = showing(o) {
        return Ok(f);
    }
    // Ahead by under a second of what was read: decode on (playback). Else seek, which lands on
    // the key frame at or before the target, and decode on from there.
    let near = o.frames.back().is_some_and(|b| target >= b.0 && target - b.0 < 10_000_000);
    if !near {
        // SAFETY: a live reader of this thread and a PROPVARIANT that lives across the call.
        unsafe {
            let pos = PROPVARIANT::from(target.max(o.start));
            o.reader.SetCurrentPosition(&GUID::zeroed(), &pos).map_err(failed("the video couldn't seek"))?;
        }
        o.frames.clear();
        o.ended = false;
    }
    // A long run of frames before the target (a far key frame) is bounded by the file's length.
    for _ in 0..crate::MAX_FRAMES {
        if let Some(f) = showing(o) {
            return Ok(f);
        }
        if !read_one(o)? {
            return o.frames.back().map(|f| f.1.clone()).ok_or_else(|| "no frame of the video could be decoded".into());
        }
        // The seek landed after the target (its first frame): that one shows.
        if let (Some(a), None) = (o.frames.front(), o.frames.get(1))
            && a.0 > target
        {
            return Ok(a.1.clone());
        }
    }
    Err("the video couldn't be decoded".into())
}

/// A sample's pixels (BGRX rows, maybe bottom up) as RGBA rows top down.
fn copy_frame(o: &Open, sample: &windows::Win32::Media::MediaFoundation::IMFSample) -> Result<Frame, String> {
    let (w, h) = (o.width as usize, o.height as usize);
    let mut rgba = vec![255u8; w * h * 4];
    // SAFETY: the buffer is locked for the copy and unlocked after it; every read stays within
    // the `len` bytes it reports, row by row at its stride.
    unsafe {
        let buf = sample.ConvertToContiguousBuffer().map_err(failed("the video couldn't be decoded"))?;
        let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
        buf.Lock(&mut ptr, None, Some(&mut len)).map_err(failed("the video couldn't be decoded"))?;
        let data = std::slice::from_raw_parts(ptr, len as usize);
        let stride = o.stride.unsigned_abs() as usize;
        for y in 0..h {
            let row = if o.stride < 0 { h - 1 - y } else { y };
            let Some(src) = data.get(row * stride..row * stride + w * 4) else { break };
            for (d, s) in rgba[y * w * 4..(y + 1) * w * 4].chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
            }
        }
        let _ = buf.Unlock();
    }
    Ok(Frame { width: o.width, height: o.height, rgba })
}
