//! The Smooth tool: dragged along a path it irons out wobbles and drops points the curve can do
//! without; ends, junctions and what's outside the brush stay; one drag is one undo step.

use serde_json::json;

use super::ops::smooth;
use super::tests::{ev, ex, net, ve};
use super::*;
use crate::tools::PointerKind;

/// An open path along y = 0 from x = 0 to 200 that zigzags 3 units up and down every 5 (its ends
/// on the line).
fn zigzag() -> VectorNetwork {
    let y = |i: i32| {
        if i == 0 || i == 40 {
            0.0
        } else if i % 2 == 0 {
            3.0
        } else {
            -3.0
        }
    };
    let pts: Vec<Point> = (0..=40).map(|i| Point::new(f64::from(i) * 5.0, y(i))).collect();
    VectorNetwork::from_points(&pts, false)
}

/// How far the drawn curve strays from y = 0 between x = 15 and 185.
fn wobble(n: &VectorNetwork) -> f64 {
    use figcraft_geom::kurbo::ParamCurve;
    (0..n.segments.len())
        .filter_map(|s| n.cubic(s))
        .flat_map(|c| (0..=16).map(move |k| c.eval(f64::from(k) / 16.0)))
        .filter(|p| p.x > 15.0 && p.x < 185.0)
        .map(|p| p.y.abs())
        .fold(0.0, f64::max)
}

/// Dabs every 3 units along the path, `passes` times.
fn brush_along(n: &mut VectorNetwork, passes: usize) {
    for _ in 0..passes {
        for i in 0..=66 {
            smooth(n, Point::new(f64::from(i) * 3.0, 0.0), 12.0, 1.0);
        }
    }
}

#[test]
fn dragging_along_a_path_irons_it_out_and_drops_points() {
    let mut n = zigzag();
    let (count, wobbled) = (n.vertices.len(), wobble(&zigzag()));
    brush_along(&mut n, 3);
    assert!(n.vertices.len() < count / 2, "points it could do without are gone: {} of {count}", n.vertices.len());
    assert!(wobble(&n) < wobbled / 3.0, "the zigzag flattens: {} vs {wobbled}", wobble(&n));
    assert!(n.vertices.iter().any(|v| (v.point() - Point::new(0.0, 0.0)).hypot() < 1e-9), "the start stays");
    assert!(n.vertices.iter().any(|v| (v.point() - Point::new(200.0, 0.0)).hypot() < 1e-9), "the end stays");
    assert_eq!(super::ops::parts(&n).len(), 1, "still one path");
}

#[test]
fn points_outside_the_brush_and_junctions_stay() {
    let mut n = zigzag();
    smooth(&mut n, Point::new(100.0, 0.0), 12.0, 1.0);
    let before = zigzag();
    for v in before.vertices.iter().filter(|v| (v.x - 100.0).abs() > 12.0) {
        assert!(n.vertices.iter().any(|w| (w.point() - v.point()).hypot() < 1e-9), "{v:?} is outside the brush");
    }
    // A junction of three segments, under the brush: it stays where it is.
    let mut t = VectorNetwork::from_points(&[Point::new(-20.0, 0.0), Point::new(0.0, 0.0), Point::new(20.0, 0.0)], false);
    t.vertices.push(Vertex::new(0.0, 20.0));
    t.segments.push(figcraft_doc::Segment::line(1, 3));
    smooth(&mut t, Point::new(0.0, 0.0), 12.0, 1.0);
    assert!(t.vertices.iter().any(|v| v.point() == Point::new(0.0, 0.0)), "the junction stays");
    assert_eq!(t.segments.len(), 3);
}

#[test]
fn a_closed_shape_keeps_three_points_and_bad_input_changes_nothing() {
    let mut sq = VectorNetwork::from_points(&[Point::new(0.0, 0.0), Point::new(20.0, 0.0), Point::new(20.0, 20.0), Point::new(0.0, 20.0)], true);
    for _ in 0..50 {
        smooth(&mut sq, Point::new(10.0, 10.0), 40.0, 100.0);
    }
    assert!(sq.vertices.len() >= 3, "{sq:?}");
    assert!(!sq.regions.is_empty(), "still filled");
    let mut n = zigzag();
    assert!(!smooth(&mut n, Point::new(f64::NAN, 0.0), 12.0, 1.0));
    assert!(!smooth(&mut n, Point::new(0.0, 0.0), f64::INFINITY, 1.0));
    assert!(!smooth(&mut n, Point::new(0.0, 0.0), 0.0, 1.0));
    assert!(!smooth(&mut n, Point::new(500.0, 500.0), 12.0, 1.0), "nothing under the brush");
    assert_eq!(n, zigzag());
}

#[test]
fn the_tool_smooths_under_the_drag_in_one_undo_step() {
    let mut s = Session::new();
    let pts: Vec<[f64; 2]> = zigzag().vertices.iter().map(|v| [v.x, v.y]).collect();
    let r = ex(&mut s, "vector.create", json!({"points": pts, "closed": false}));
    let id = crate::params::id_of(&r["id"]).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    assert!(key(&mut s, "S", true, false, false).unwrap(), "Shift+S picks it");
    assert_eq!(ve(&s).tool, VTool::Smooth);
    s.pointer(&ev(PointerKind::Hover, 50.0, 0.0)).unwrap();
    assert!(s.tool.overlay.brush.is_some(), "the brush shows");
    let count = net(&s, id).vertices.len();
    s.pointer(&ev(PointerKind::Down, 0.0, 0.0)).unwrap();
    for i in 1..=40 {
        s.pointer(&ev(PointerKind::Move, f64::from(i) * 5.0, 0.0)).unwrap();
    }
    s.pointer(&ev(PointerKind::Up, 200.0, 0.0)).unwrap();
    assert!(net(&s, id).vertices.len() < count, "points dropped under the drag");
    s.undo().unwrap();
    assert_eq!(net(&s, id).vertices.len(), count, "one undo step brings the path back");
    // Agents brush the same way with a command.
    let r = ex(&mut s, "vector.edit.smoothAlong", json!({"points": [[0, 0], [200, 0]]}));
    assert_eq!(r["changed"], json!(true));
    assert!(r["vertices"].as_u64().unwrap() < count as u64, "{r}");
}
