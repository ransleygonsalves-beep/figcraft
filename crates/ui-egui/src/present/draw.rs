use std::collections::BTreeSet;
use std::sync::Arc;

use figcraft_doc::{Document, Node};

use super::actions::texture_scale;
use super::layout::{Layout, Place, hotspots};
use super::transition::{smart_interpolate, transition_frames};
use super::{ACCENT, Color32, CornerRadius, FLASH_SECS, NodeId, OverlayState, Presenter, Rect, Stroke, StrokeKind, Transition, pos2, vec2};

pub(super) fn draw(p: &mut Presenter, painter: &egui::Painter, ctx: &egui::Context, l: &Layout, now: f64) {
    let ppp = f64::from(ctx.pixels_per_point());
    let px = l.scale * ppp;
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let to_tex = p.texture(ctx, &l.disp, p.current, px, now);
    // A Smart Animate inside the presented frame (Change to): which part of it changes, once.
    if let Some(a) = p.anim.as_mut()
        && a.region.is_none()
        && matches!(a.transition, Transition::SmartAnimate)
    {
        a.region = Some(if a.from_frame == p.current { changed_area(&a.from_doc, &l.disp, p.current) } else { None });
    }
    // Transition progress.
    let mut drawn = false;
    if let Some(a) = &p.anim {
        let raw = ((now - a.start) / a.dur.max(1e-3)).clamp(0.0, 1.0);
        if raw >= 1.0 {
            p.anim = None;
        } else {
            ctx.request_repaint();
            let e = a.easing.eval(raw);
            let from_place = a.from_doc.world_bounds(a.from_frame).map(|fb| Place::centered(fb, l.screen.center(), l.scale));
            if matches!(a.transition, Transition::SmartAnimate) {
                #[cfg(not(target_arch = "wasm32"))]
                let t0 = std::time::Instant::now();
                let tmp = smart_interpolate(&a.from_doc, a.from_frame, &l.disp, p.current, e);
                if let (Some(Some(area)), Some(to)) = (a.region, &to_tex) {
                    // Only `area` changes: the rest is the frame as it ends, already rendered, and
                    // the area alone is rendered as it is now (every layer of the frame in it, so
                    // what lies over or under the change draws as it does in the whole frame).
                    if let Some(pix) = figcraft_render::render_area_at(&tmp, &[p.current], area, texture_scale(ctx, area, px), 0.0) {
                        let tex = anim_texture(&mut p.anim_tex, ctx, crate::render_worker::screen_image(&pix, tmp.color_profile));
                        let hole = l.clip.intersect(l.place.rect(area));
                        for part in base_parts(l.clip, Some(hole)) {
                            painter.with_clip_rect(part).image(to.tex.id(), l.place.rect(to.bounds), uv, Color32::WHITE);
                        }
                        painter.with_clip_rect(l.clip).image(tex.id(), l.place.rect(area), uv, Color32::WHITE);
                        drawn = true;
                    }
                } else if let Some(b) = figcraft_render::export_bounds(&tmp, p.current)
                    && let Some(pix) = figcraft_render::render_node(&tmp, p.current, texture_scale(ctx, b, px))
                {
                    let tex = anim_texture(&mut p.anim_tex, ctx, crate::render_worker::screen_image(&pix, tmp.color_profile));
                    painter.with_clip_rect(l.screen).image(tex.id(), l.place.rect(b), uv, Color32::WHITE);
                    drawn = true;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    p.render_ms = t0.elapsed().as_secs_f64() * 1000.0;
                }
            } else if let (Some(ft), Some(to), Some(fp)) = (&a.from_tex, &to_tex, from_place) {
                let f = transition_frames(a.transition, e as f32);
                let from_rect = fp.rect(ft.bounds);
                let from_frame_rect = a.from_doc.world_bounds(a.from_frame).map(|b| fp.rect(b)).unwrap_or(from_rect);
                let to_rect = l.place.rect(to.bounds);
                let off = |r: Rect, o: (f32, f32)| vec2(r.width() * o.0, r.height() * o.1);
                // Without a device a dissolve shows both frames whole; on one, the screen clips.
                let dissolve_both = l.device.is_none() && matches!(a.transition, Transition::Dissolve);
                let clip = painter.with_clip_rect(if dissolve_both { l.screen.union(from_frame_rect) } else { l.screen });
                let draw_from = |p: &egui::Painter| {
                    if f.from_alpha > 0.0 {
                        p.image(ft.tex.id(), from_rect.translate(off(from_frame_rect, f.from_off)), uv, Color32::WHITE.gamma_multiply(f.from_alpha));
                    }
                };
                let draw_to = |p: &egui::Painter| {
                    p.image(to.tex.id(), to_rect.translate(off(l.screen, f.to_off)), uv, Color32::WHITE.gamma_multiply(f.to_alpha));
                };
                if f.from_on_top {
                    draw_to(&clip);
                    draw_from(&clip);
                } else {
                    draw_from(&clip);
                    draw_to(&clip);
                }
                drawn = true;
            }
        }
    }
    if !drawn && let Some(to) = &to_tex {
        let on_clip = painter.with_clip_rect(l.clip);
        // A frame too big for one texture at device scale is drawn from a smaller one: the part in
        // view is drawn from a texture at device scale instead. The detail REPLACES that part of the
        // whole-frame texture (the base is drawn only around it): both start from a transparent
        // pixmap, so drawn over it a translucent layer would count twice.
        let shown = l.clip.intersect(l.place.rect(to.bounds));
        let mut detail = None;
        if shown.is_positive() {
            let (a, b) = (l.place.page(shown.min), l.place.page(shown.max));
            let visible = figcraft_geom::Rect::new(a.x, a.y, b.x, b.y);
            let settling = settling(p, ctx, px, visible, now);
            let moving = settling || p.fling.is_some() || p.drag_scroll.is_some() || p.wheel.is_some();
            detail = p.detail_texture(ctx, &l.disp, p.current, px, to.bounds, visible, moving, now);
        }
        let hole = detail.as_ref().map(|d| l.clip.intersect(l.place.rect(d.bounds)));
        for part in base_parts(l.clip, hole) {
            painter.with_clip_rect(part).image(to.tex.id(), l.place.rect(to.bounds), uv, Color32::WHITE);
        }
        if let Some(d) = detail {
            on_clip.image(d.tex.id(), l.place.rect(d.bounds), uv, Color32::WHITE);
        }
    }
    // Overlays (each with its background and opening transition); closing ones run it backwards.
    let mut ovs: Vec<(f64, OverlayState)> =
        p.overlays.iter().map(|o| (if o.dur > 0.0 { ((now - o.opened) / o.dur).clamp(0.0, 1.0) } else { 1.0 }, o.clone())).collect();
    let mut placed: Vec<(NodeId, Place, Rect)> = l.overlays.clone();
    for ((at, o), pl) in p.closing.iter().zip(l.closing.iter()) {
        if now - at >= o.dur {
            continue;
        }
        ovs.insert(0, (1.0 - ((now - at) / o.dur.max(1e-3)).clamp(0.0, 1.0), o.clone()));
        placed.insert(0, *pl);
    }
    let on_screen = painter.with_clip_rect(l.screen);
    for ((raw, o), (id, place, orect)) in ovs.iter().zip(placed.iter()) {
        let raw = *raw;
        if raw < 1.0 {
            ctx.request_repaint();
        }
        let e = o.easing.eval(raw) as f32;
        let f = if matches!(o.transition, Transition::Instant) {
            transition_frames(Transition::Dissolve, 1.0)
        } else {
            transition_frames(o.transition, e)
        };
        if let Some(bgc) = o.background {
            let c = crate::theme::doc_c32(ctx, bgc);
            on_screen.rect_filled(l.screen, CornerRadius::ZERO, c.gamma_multiply(raw as f32));
        }
        if let Some(tex) = p.texture(ctx, &l.disp, *id, px, now) {
            let r = place.rect(tex.bounds).translate(vec2(orect.width() * f.to_off.0, orect.height() * f.to_off.1));
            on_screen.image(tex.tex.id(), r, uv, Color32::WHITE.gamma_multiply(f.to_alpha));
        }
    }
    // Finished closing animations go (after drawing, so they stayed paired with the layout).
    p.closing.retain(|(at, o)| now - at < o.dur);
    // Hotspot flash after a click on nothing.
    if now < p.flash_until {
        let (root, place, clip) = l.active(p.current);
        let a = ((p.flash_until - now) / FLASH_SECS) as f32;
        for (_, r) in hotspots(&l.disp, root, &place, clip) {
            painter.rect_filled(r, CornerRadius::same(4), ACCENT.gamma_multiply(0.25 * a));
            painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.5, ACCENT.gamma_multiply(a)), StrokeKind::Outside);
        }
        ctx.request_repaint();
    }
}

/// A wheel turned notch by notch stops between notches: the view counts as moving until it has
/// kept still [`SETTLE`] at the same frame and scale, so no detail is rendered (on this thread)
/// mid-scroll.
fn settling(p: &mut Presenter, ctx: &egui::Context, px: f64, visible: figcraft_geom::Rect, now: f64) -> bool {
    let moved_at = match p.view_at {
        Some((id, s, r, at)) if id == p.current && s == px => {
            if r == visible {
                at
            } else {
                now
            }
        }
        _ => f64::NEG_INFINITY,
    };
    p.view_at = Some((p.current, px, visible, moved_at));
    let settling = now - moved_at < SETTLE;
    if settling {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
    }
    settling
}

/// How long the view keeps still before the part in view is rendered at full detail (seconds).
pub(super) const SETTLE: f64 = 0.2;

/// The Smart Animate texture, its image replaced (one texture, reused for every frame).
fn anim_texture(slot: &mut Option<egui::TextureHandle>, ctx: &egui::Context, img: egui::ColorImage) -> egui::TextureHandle {
    match slot {
        Some(t) => {
            t.set(img, egui::TextureOptions::LINEAR);
            t.clone()
        }
        None => {
            let t = ctx.load_texture("present-anim", img, egui::TextureOptions::LINEAR);
            *slot = Some(t.clone());
            t
        }
    }
}

/// Past this share of the frame, a change inside it is drawn by rendering the whole frame.
const MOST_OF_THE_FRAME: f64 = 0.5;

/// What changed in a frame between two states of the player's document.
#[derive(Debug, PartialEq)]
pub(super) enum Change {
    Nothing,
    /// Only this page area (inside the frame) shows anything different.
    Area(figcraft_geom::Rect),
    /// The frame itself, what all of it draws with, or most of it.
    Whole,
}

/// The page area a change inside `frame` touches from `from` to `to`: every layer of it that
/// differs, where it was and where it is (its effects included), with a margin for what Smart
/// Animate moves between them. [`Change::Whole`] when the frame itself differs (beyond which
/// layers it holds), when what every layer draws with does (styles, variables, images, colour
/// profile), or when the area is most of the frame.
pub(super) fn change(from: &Document, to: &Document, frame: NodeId) -> Change {
    const MARGIN: f64 = 8.0;
    let Some(whole) = to.world_render_bounds(frame) else { return Change::Whole };
    if from.styles != to.styles
        || from.variables != to.variables
        || from.color_profile != to.color_profile
        || !from.images.keys().eq(to.images.keys())
        || from.world_render_bounds(frame) != Some(whole)
    {
        return Change::Whole;
    }
    let ids: BTreeSet<NodeId> = from.subtree(frame).into_iter().chain(to.subtree(frame)).collect();
    let mut area: Option<figcraft_geom::Rect> = None;
    for id in ids {
        let (a, b) = (from.node_arc(id), to.node_arc(id));
        if let (Some(a), Some(b)) = (a, b)
            && (Arc::ptr_eq(a, b) || a == b)
        {
            continue;
        }
        if id == frame {
            // Layers added to or taken from the frame count where they are; anything else about
            // the frame, or its other layers changing order, changes all of it.
            let (Some(a), Some(b)) = (a, b) else { return Change::Whole };
            let kept = |of: &Node, other: &Node| of.children.iter().filter(|c| other.children.contains(c)).copied().collect::<Vec<_>>();
            let bare = |n: &Node| Node { children: Vec::new(), ..n.clone() };
            if kept(a, b) != kept(b, a) || bare(a) != bare(b) {
                return Change::Whole;
            }
            continue;
        }
        for r in [from.world_render_bounds(id), to.world_render_bounds(id)].into_iter().flatten() {
            area = Some(area.map_or(r, |a| a.union(r)));
        }
    }
    let Some(area) = area else { return Change::Nothing };
    let area = area.inflate(MARGIN, MARGIN).intersect(whole);
    if !(area.width() > 0.0 && area.height() > 0.0) {
        Change::Nothing
    } else if area.area() <= whole.area() * MOST_OF_THE_FRAME {
        Change::Area(area)
    } else {
        Change::Whole
    }
}

/// The area of a [`change`], when it is one small enough to draw on its own.
pub(super) fn changed_area(from: &Document, to: &Document, frame: NodeId) -> Option<figcraft_geom::Rect> {
    match change(from, to, frame) {
        Change::Area(a) => Some(a),
        _ => None,
    }
}

/// The parts of `clip` outside `hole` (up to four disjoint rectangles; `clip` itself with no hole).
pub(super) fn base_parts(clip: Rect, hole: Option<Rect>) -> Vec<Rect> {
    let Some(h) = hole.filter(|h| h.is_positive()) else {
        return vec![clip];
    };
    let h = h.intersect(clip);
    if !h.is_positive() {
        return vec![clip];
    }
    [
        Rect::from_min_max(clip.min, egui::pos2(clip.max.x, h.min.y)),
        Rect::from_min_max(egui::pos2(clip.min.x, h.max.y), clip.max),
        Rect::from_min_max(egui::pos2(clip.min.x, h.min.y), egui::pos2(h.min.x, h.max.y)),
        Rect::from_min_max(egui::pos2(h.max.x, h.min.y), egui::pos2(clip.max.x, h.max.y)),
    ]
    .into_iter()
    .filter(|r| r.is_positive())
    .collect()
}
