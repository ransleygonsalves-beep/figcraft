//! Reading what isn't a usable MP4 is refused with a reason. With Cisco's OpenH264 library
//! (`FIGCRAFT_OPENH264` names it; without it these skip), frames written come back: the file says
//! its size, length and rate, any frame decodes (forwards, backwards, by time) close to what was
//! written (on Windows through its own decoder), and odd sides are cropped even.

use crate::{Frame, is_mp4, probe};

#[test]
fn what_isnt_an_h264_mp4_is_refused_with_a_reason() {
    assert!(!is_mp4(b"GIF89a"));
    assert!(probe(b"not a video at all").unwrap_err().contains("isn't an MP4"));
    // An `ftyp` box with nothing after it: no video track.
    let empty = [0, 0, 0, 16, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 2, 0];
    assert!(is_mp4(&empty));
    assert!(probe(&empty).is_err());
}

#[cfg(not(target_arch = "wasm32"))]
mod with_codec {
    use super::*;
    use crate::{Clip, Player, Writer, set_codec};

    fn codec() -> bool {
        match std::env::var("FIGCRAFT_OPENH264") {
            Ok(p) => set_codec(p).map_err(|e| panic!("{e}")).is_ok(),
            Err(_) => {
                eprintln!("skipped: set FIGCRAFT_OPENH264 to Cisco's OpenH264 2.6.0 library");
                false
            }
        }
    }

    /// Frame `i`: a solid colour that changes with `i`.
    fn solid(i: u32, w: u32, h: u32) -> Frame {
        let px = [(i * 8 % 256) as u8, 128, (255 - i * 8 % 256) as u8, 255];
        Frame { width: w, height: h, rgba: px.repeat((w * h) as usize) }
    }

    fn centre(f: &Frame) -> [u8; 3] {
        let i = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
    }

    fn close(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 12)
    }

    #[test]
    fn frames_written_come_back() {
        if !codec() {
            return;
        }
        let mut w = Writer::new(65, 49, 30.0).unwrap();
        assert_eq!(w.size(), (64, 48), "cropped to even sides");
        for i in 0..30 {
            w.push(&solid(i, 65, 49)).unwrap();
        }
        let mp4 = w.finish().unwrap();
        assert!(is_mp4(&mp4));
        let info = probe(&mp4).unwrap();
        assert_eq!((info.width, info.height, info.frames), (64, 48, 30));
        assert!((info.duration - 1.0).abs() < 0.05 && (info.fps - 30.0).abs() < 1.0, "{info:?}");
        let mut p = Player::new(Clip::open(std::sync::Arc::new(mp4)).unwrap()).unwrap();
        let want = |i: u32| centre(&solid(i, 64, 48));
        for k in [0usize, 10, 29, 3, 20] {
            let f = p.frame(k).unwrap();
            assert_eq!((f.width, f.height), (64, 48));
            assert!(close(centre(&f), want(k as u32)), "frame {k}: {:?} vs {:?}", centre(&f), want(k as u32));
        }
        let half = p.frame_at(0.5).unwrap();
        assert!(close(centre(&half), want(15)), "half way in: {:?}", centre(&half));
        // Windows plays it through its own decoder (Media Foundation).
        assert_eq!(p.system(), cfg!(windows));
        let looped = p.frame_at(1.25).unwrap();
        assert!(close(centre(&looped), centre(&p.frame_at(0.25).unwrap())), "past the end it loops");
    }
}
