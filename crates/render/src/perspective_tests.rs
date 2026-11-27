//! The perspective effect: a positive up/down tilt leans the top edge away (narrower than the
//! bottom), a positive left/right tilt turns the right edge away (shorter than the left), no tilt
//! draws the layer as it was, opacity applies once to the tilted whole, a hidden one does nothing,
//! and the layer's paint bounds cover what it draws tilted (culling and partial redraws use them).

use figcraft_color::Paint;
use figcraft_doc::{Document, Effect, EffectKind, NodeId, Perspective};

use super::paint_behaviour_tests::{hex, page, px, rect, render};

/// A 100 × 100 red square at (50, 50), with a perspective of `tilt` (up/down, left/right).
fn tilted(tilt: (f64, f64), visible: bool) -> (Document, NodeId) {
    let (mut d, p) = page();
    let r = rect(&mut d, p, (50.0, 50.0, 100.0, 100.0), vec![Paint::solid(hex("#FF0000"))]);
    let kind = EffectKind::Perspective(Perspective { tilt_x: tilt.0, tilt_y: tilt.1, distance: 300.0 });
    d.get_mut(r).unwrap().effects = vec![Effect { kind, visible }];
    (d, r)
}

fn red(p: &tiny_skia::Pixmap, x: u32, y: u32) -> bool {
    let [r, g, b] = px(p, x, y);
    r > 200 && g < 80 && b < 80
}

/// How many red pixels along row `y` (or, `column`, along column `y`).
fn run(p: &tiny_skia::Pixmap, y: u32, column: bool) -> usize {
    (0..200).filter(|&i| if column { red(p, y, i) } else { red(p, i, y) }).count()
}

#[test]
fn an_up_down_tilt_leans_the_top_edge_away() {
    let flat = render(&tilted((0.0, 0.0), true).0);
    let pix = render(&tilted((40.0, 0.0), true).0);
    let (top, bottom) = (run(&pix, 70, false), run(&pix, 130, false));
    assert!(top > 0 && top + 10 < bottom, "the top is narrower than the bottom: {top} vs {bottom}");
    // The square's top corner was red flat and isn't tilted.
    assert!(red(&flat, 52, 52) && !red(&pix, 52, 52));
}

#[test]
fn a_left_right_tilt_turns_the_right_edge_away() {
    let pix = render(&tilted((0.0, 40.0), true).0);
    let (left, right) = (run(&pix, 70, true), run(&pix, 130, true));
    assert!(right > 0 && right + 10 < left, "the right side is shorter than the left: {right} vs {left}");
}

#[test]
fn no_tilt_or_a_hidden_one_draws_the_layer_as_it_was() {
    let (mut plain, r) = tilted((0.0, 0.0), true);
    plain.get_mut(r).unwrap().effects.clear();
    let want = render(&plain);
    for got in [render(&tilted((0.0, 0.0), true).0), render(&tilted((40.0, 30.0), false).0)] {
        let differ = (0..200u32).flat_map(|y| (0..200u32).map(move |x| (x, y))).filter(|&(x, y)| px(&want, x, y) != px(&got, x, y)).count();
        assert!(differ < 40, "{differ} pixels differ (only the edges' resampling may)");
    }
}

#[test]
fn opacity_applies_once_to_the_tilted_layer() {
    let (mut d, r) = tilted((30.0, 0.0), true);
    d.get_mut(r).unwrap().opacity = 0.5;
    let [r_, g, b] = px(&render(&d), 100, 100);
    assert!(r_ > 240 && (118..=138).contains(&g) && (118..=138).contains(&b), "half red over white: {:?}", [r_, g, b]);
}

#[test]
fn the_paint_bounds_cover_what_the_tilt_draws() {
    let (d, r) = tilted((40.0, 25.0), true);
    let n = d.node(r).unwrap();
    let world = d.world_transform(r);
    let b = n.own_paint_bounds(world);
    let flat = n.local_rect();
    assert!(b.width() > flat.width() || b.height() > flat.height(), "the near side grows past the box: {b:?}");
    let pix = render(&d);
    for y in 0..200u32 {
        for x in 0..200u32 {
            if red(&pix, x, y) {
                assert!(b.inflate(1.0, 1.0).contains(figcraft_geom::Point::new(f64::from(x), f64::from(y))), "({x}, {y}) is outside {b:?}");
            }
        }
    }
}
