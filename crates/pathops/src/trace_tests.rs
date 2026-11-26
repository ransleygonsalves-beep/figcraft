//! Image tracing: colours stack without seams, holes stay empty, speckles go, and what is
//! transparent is left out.

use figcraft_geom::kurbo::{Point, Shape};

use crate::trace::{MAX_PIXELS, TraceOptions, TraceStop, trace, trace_with};

const OPT: TraceOptions = TraceOptions { colors: 8, speckle: 4, tolerance: 1.0, mono: None };

/// A `w` × `h` image painted by `at(x, y)` (RGBA).
fn image(w: usize, h: usize, at: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| at(x, y)).collect()
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [220, 20, 30, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn inside(x: usize, y: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> bool {
    (x0..x1).contains(&x) && (y0..y1).contains(&y)
}

#[test]
fn colours_stack_largest_first_and_the_lower_one_runs_under_the_upper() {
    let img = image(40, 30, |x, y| if inside(x, y, 10, 8, 25, 20) { RED } else { WHITE });
    let layers = trace(&img, 40, 30, &OPT).unwrap();
    assert_eq!(layers.len(), 2);
    let (bottom, top) = (&layers[0], &layers[1]);
    assert_eq!(bottom.color, [255, 255, 255], "the bigger colour is at the bottom");
    assert_eq!(top.color, [220, 20, 30]);
    // No seam: the white shape covers the red square's area too.
    assert!(bottom.path.winding(Point::new(17.5, 14.0)) != 0, "the bottom colour runs under the top one");
    let b = bottom.path.bounding_box();
    assert!(b.x0 <= 0.5 && b.y0 <= 0.5 && b.x1 >= 39.5 && b.y1 >= 29.5, "{b:?}");
    let t = top.path.bounding_box();
    assert!((t.x0 - 10.0).abs() < 1.0 && (t.y0 - 8.0).abs() < 1.0 && (t.x1 - 25.0).abs() < 1.0 && (t.y1 - 20.0).abs() < 1.0, "{t:?}");
}

#[test]
fn a_hole_stays_empty() {
    // A red ring on a clear background.
    let img = image(40, 40, |x, y| if inside(x, y, 5, 5, 35, 35) && !inside(x, y, 15, 15, 25, 25) { RED } else { CLEAR });
    let layers = trace(&img, 40, 40, &OPT).unwrap();
    assert_eq!(layers.len(), 1);
    let path = &layers[0].path;
    assert!(path.winding(Point::new(8.0, 20.0)) != 0, "the ring is filled");
    assert_eq!(path.winding(Point::new(20.0, 20.0)), 0, "its hole is not");
    assert_eq!(path.winding(Point::new(2.0, 2.0)), 0, "the clear background is not");
}

#[test]
fn speckles_take_the_colour_around_them() {
    let img = image(30, 30, |x, y| {
        if (x, y) == (12, 7) {
            [0, 0, 255, 255]
        } else if inside(x, y, 0, 0, 30, 15) {
            RED
        } else {
            WHITE
        }
    });
    let colours: Vec<[u8; 3]> = trace(&img, 30, 30, &OPT).unwrap().iter().map(|l| l.color).collect();
    assert_eq!(colours.len(), 2, "the lone blue pixel is gone: {colours:?}");
    assert!(!colours.contains(&[0, 0, 255]));
    let kept = trace(&img, 30, 30, &TraceOptions { speckle: 1, ..OPT }).unwrap();
    assert_eq!(kept.len(), 3, "with speckles kept it stays");
}

#[test]
fn smooth_outlines_curve_and_square_corners_stay_sharp() {
    // A disc: its outline is made of curves, not a pixel staircase.
    let disc = image(64, 64, |x, y| if (x as f64 - 31.5).hypot(y as f64 - 31.5) < 24.0 { RED } else { CLEAR });
    let layers = trace(&disc, 64, 64, &OPT).unwrap();
    let curves = layers[0].path.elements().iter().filter(|e| matches!(e, figcraft_geom::kurbo::PathEl::CurveTo(..))).count();
    assert!(curves >= 4, "a disc comes out curved");
    assert!(layers[0].path.elements().len() < 60, "far fewer points than its pixel steps: {}", layers[0].path.elements().len());
    // A square: four straight sides.
    let square = image(30, 30, |x, y| if inside(x, y, 5, 5, 25, 25) { RED } else { CLEAR });
    let sq = &trace(&square, 30, 30, &OPT).unwrap()[0].path;
    assert!(sq.elements().iter().all(|e| !matches!(e, figcraft_geom::kurbo::PathEl::CurveTo(..))), "{sq:?}");
}

#[test]
fn nothing_opaque_traces_to_nothing_and_bad_input_is_refused() {
    assert!(trace(&image(8, 8, |_, _| CLEAR), 8, 8, &OPT).unwrap().is_empty());
    assert!(trace(&[0; 15], 2, 2, &OPT).is_none(), "the buffer doesn't match the size");
    assert!(trace(&[], 0, 0, &OPT).is_none());
    assert!(trace(&[], usize::MAX, 2, &OPT).is_none(), "an overflowing size");
    let side = (MAX_PIXELS as f64).sqrt() as usize + 1;
    assert!(trace(&vec![255; side * side * 4], side, side, &OPT).is_none(), "past the pixel cap");
}

#[test]
fn progress_rises_to_the_end_and_cancelling_stops_it() {
    let img = image(40, 30, |x, y| if inside(x, y, 10, 8, 25, 20) { RED } else { WHITE });
    let mut seen = Vec::new();
    let layers = trace_with(&img, 40, 30, &OPT, &mut |f| {
        seen.push(f);
        true
    })
    .unwrap();
    assert_eq!(layers.len(), 2);
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "never goes back: {seen:?}");
    assert_eq!(seen.last(), Some(&1.0));
    let mut calls = 0;
    let stopped = trace_with(&img, 40, 30, &OPT, &mut |_| {
        calls += 1;
        calls < 3
    });
    assert_eq!(stopped.err(), Some(TraceStop::Cancelled));
    assert_eq!(calls, 3, "stops at the first no");
}

#[test]
fn one_colour_keeps_the_dark_pixels_as_one_shape() {
    // A dark ring and a light-grey square on white: at half the lightness only the ring is dark.
    let img = image(60, 40, |x, y| {
        if inside(x, y, 5, 5, 25, 25) && !inside(x, y, 10, 10, 20, 20) {
            [20, 20, 30, 255]
        } else if inside(x, y, 35, 10, 50, 25) {
            [200, 200, 200, 255]
        } else {
            WHITE
        }
    });
    let layers = trace(&img, 60, 40, &TraceOptions { mono: Some(0.5), ..OPT }).unwrap();
    assert_eq!(layers.len(), 1, "one shape");
    assert_eq!(layers[0].color, [20, 20, 30], "in the dark pixels' colour");
    let path = &layers[0].path;
    assert!(path.winding(Point::new(7.0, 15.0)) != 0, "the ring is in it");
    assert_eq!(path.winding(Point::new(15.0, 15.0)), 0, "its hole isn't");
    assert_eq!(path.winding(Point::new(42.0, 17.0)), 0, "the light square isn't");
    // Lighter than the threshold everywhere: nothing.
    assert!(trace(&img, 60, 40, &TraceOptions { mono: Some(0.05), ..OPT }).unwrap().is_empty());
    // A higher threshold takes the grey square too.
    let both = trace(&img, 60, 40, &TraceOptions { mono: Some(0.9), ..OPT }).unwrap();
    assert!(both[0].path.winding(Point::new(42.0, 17.0)) != 0);
}
