//! The water reminder (an easter egg): after an hour with the app open, the interface turns blue
//! and a card asks whether you've had some water. It stays until you answer it; then the colours
//! come back and the hour starts again. Preferences don't list it: it switches off at the bottom
//! of Help ▸ About FigCraft (`pref.waterReminder`).

use egui::{Color32, RichText};

use crate::FigApp;
use crate::i18n::tr;
use crate::theme::{Tokens, regular, semibold};

/// How long the app is open before it asks (seconds).
pub const AFTER_SECS: f64 = 3600.0;

/// Where the reminder is.
#[derive(Clone, Copy, Debug, Default)]
pub struct Water {
    /// When the current hour started (egui's clock), once counting.
    pub since: Option<f64>,
    /// The card is up and the interface is blue.
    pub due: bool,
}

impl Water {
    /// Move the clock to `now` (seconds): due once an hour has passed since it started. With the
    /// reminder off it stops and forgets. Returns the seconds until it is due (`None`: due, or off).
    pub fn tick(&mut self, on: bool, now: f64) -> Option<f64> {
        if !on {
            *self = Water::default();
            return None;
        }
        if self.due {
            return None;
        }
        let since = *self.since.get_or_insert(now);
        let left = AFTER_SECS - (now - since);
        if left <= 0.0 {
            self.due = true;
            None
        } else {
            Some(left)
        }
    }

    /// Answered: the colours come back and the next hour starts at `now`.
    pub fn answer(&mut self, now: f64) {
        *self = Water { since: Some(now), due: false };
    }
}

/// The theme's tokens with water in them: blue backgrounds and a blue accent, the text as it was
/// (light text on deep blues, dark text on pale ones, so it reads as well as before).
pub fn tint(t: Tokens) -> Tokens {
    let blue = |light: (u8, u8, u8), dark: (u8, u8, u8)| {
        let (r, g, b) = if t.dark { dark } else { light };
        Color32::from_rgb(r, g, b)
    };
    Tokens {
        bg: blue((0xE6, 0xF1, 0xFC), (0x10, 0x22, 0x38)),
        panel: blue((0xEE, 0xF5, 0xFD), (0x14, 0x28, 0x40)),
        canvas: blue((0xDD, 0xEB, 0xF9), (0x0D, 0x1C, 0x2E)),
        tab_bar: blue((0xD6, 0xE7, 0xF8), (0x0B, 0x18, 0x28)),
        tab_active: blue((0xEE, 0xF5, 0xFD), (0x14, 0x28, 0x40)),
        field: blue((0xE0, 0xEC, 0xFA), (0x1B, 0x33, 0x4F)),
        hover: blue((0xD8, 0xE8, 0xF9), (0x1F, 0x3A, 0x5A)),
        menu: blue((0x0E, 0x2A, 0x4A), (0x0E, 0x2A, 0x4A)),
        accent: Color32::from_rgb(0x1E, 0x7F, 0xE0),
        accent_hover: Color32::from_rgb(0x18, 0x6C, 0xC2),
        selected_row: blue((0xCB, 0xE2, 0xFA), (0x1D, 0x45, 0x70)),
        ..t
    }
}

/// The card's colours: deep navy, and two waters (the back wave lighter) dark enough that the
/// white title and pale body text keep 4.5:1 on them.
const CARD: Color32 = Color32::from_rgb(0x0E, 0x2A, 0x4A);
const WATER_BACK: Color32 = Color32::from_rgb(0x27, 0x63, 0xA3);
const WATER_FRONT: Color32 = Color32::from_rgb(0x19, 0x4D, 0x86);
const BODY_TEXT: Color32 = Color32::from_rgb(0xD6, 0xE7, 0xF8);
const RADIUS: f32 = 10.0;
/// How full the card gets, and how long it takes to fill (seconds).
const FULL: f32 = 0.55;
const FILL_SECS: f64 = 1.8;

/// The card, while the reminder is due: bottom centre, above the toasts, until answered. Water
/// fills it from the bottom like a glass, waves drifting on top and bubbles rising; with reduce
/// motion it is still water, already full.
pub fn card(app: &mut FigApp, ctx: &egui::Context) {
    let shown_id = egui::Id::unique("water_reminder_shown");
    if !app.ui.water.due {
        ctx.data_mut(|d| d.remove::<f64>(shown_id));
        return;
    }
    let now = ctx.input(|i| i.time);
    let still = app.reduce_motion();
    let shown = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(shown_id, || now));
    let (level, t) = if still {
        (FULL, 0.0)
    } else {
        let k = ((now - shown) / FILL_SECS).clamp(0.0, 1.0) as f32;
        (FULL * (1.0 - (1.0 - k).powi(3)), now as f32)
    };
    let screen = ctx.content_rect();
    let mut answered = false;
    egui::Area::new(egui::Id::unique("water_reminder"))
        .fixed_pos(egui::pos2(screen.center().x, screen.max.y - 150.0))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let under = ui.painter().add(egui::Shape::Noop);
            let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
                ui.set_width(340.0);
                let title = tr("Have you had some water?");
                let r = ui.label(RichText::new(title).font(semibold(13.0)).color(Color32::WHITE));
                crate::a11y::alert(&r, title);
                ui.label(
                    RichText::new(tr("You've been designing for an hour. A glass of water helps you stay sharp."))
                        .font(regular(11.0))
                        .color(BODY_TEXT),
                );
                ui.add_space(8.0);
                answered = crate::widgets::button(ui, tr("I've had some"), true, true).clicked();
            });
            ui.painter().set(under, glass(frame.response.rect, level, t));
        });
    if answered {
        app.ui.water.answer(now);
    } else if !still {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// The card's background with water `level` (0 empty – 1 full) in it at time `t` (seconds).
fn glass(rect: egui::Rect, level: f32, t: f32) -> egui::Shape {
    let mut shapes = vec![egui::Shape::rect_filled(rect, RADIUS, CARD)];
    shapes.push(egui::Shape::mesh(water(rect, level, t * 1.3 + 1.7, 3.5, WATER_BACK)));
    shapes.push(egui::Shape::mesh(water(rect, level - 0.03, -t * 1.9, 4.5, WATER_FRONT)));
    // Bubbles rise through the water and fade as they reach the surface.
    let surface = rect.max.y - rect.height() * level;
    for k in 0..5u8 {
        let k = f32::from(k);
        let p = ((t + k * 0.9) / (2.6 + k * 0.55)).fract();
        let x = rect.min.x + rect.width() * (0.14 + 0.18 * k) + (t * 2.0 + k).sin() * 3.0;
        let y = rect.max.y - 6.0 - p * (rect.max.y - 6.0 - surface);
        if y > surface + 4.0 {
            let alpha = ((1.0 - p) * 120.0) as u8;
            shapes.push(egui::Shape::circle_stroke(egui::pos2(x, y), 1.5 + k % 3.0, egui::Stroke::new(1.0, Color32::from_white_alpha(alpha))));
        }
    }
    egui::Shape::Vec(shapes)
}

/// Water filling `rect` to `level` with a wave of `amp` pixels on top at `phase`: thin columns
/// from the wave down to the card's rounded bottom, so it stays inside the corners.
fn water(rect: egui::Rect, level: f32, phase: f32, amp: f32, color: Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    if level <= 0.0 {
        return mesh;
    }
    let base = rect.max.y - rect.height() * level.min(1.0);
    let bottom = |x: f32| {
        let d = (x - rect.min.x).min(rect.max.x - x);
        if d >= RADIUS {
            rect.max.y
        } else {
            let o = RADIUS - d.max(0.0);
            rect.max.y - RADIUS + (RADIUS * RADIUS - o * o).max(0.0).sqrt()
        }
    };
    let steps = (rect.width() / 3.0).ceil().max(1.0) as u32;
    for i in 0..=steps {
        let u = i as f32 / steps as f32;
        let x = rect.min.x + rect.width() * u;
        let wave = (u * 9.0 + phase).sin() * amp + (u * 4.0 - phase * 0.6).sin() * amp * 0.4;
        let low = bottom(x);
        let top = (base + wave).clamp(rect.min.y, low);
        mesh.colored_vertex(egui::pos2(x, top), color);
        mesh.colored_vertex(egui::pos2(x, low), color);
        if i > 0 {
            let n = 2 * i;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n + 1, n);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::{AFTER_SECS, BODY_TEXT, CARD, RADIUS, WATER_BACK, WATER_FRONT, Water, tint, water};
    use crate::theme::Tokens;

    #[test]
    fn it_comes_after_an_hour_and_again_an_hour_after_the_answer() {
        let mut w = Water::default();
        assert!(w.tick(true, 10.0).is_some_and(|left| (left - AFTER_SECS).abs() < 1e-9));
        assert!(w.tick(true, 10.0 + AFTER_SECS - 1.0).is_some() && !w.due);
        assert!(w.tick(true, 10.0 + AFTER_SECS).is_none() && w.due, "due at the hour");
        w.answer(5000.0);
        assert!(!w.due && w.tick(true, 5000.0 + 60.0).is_some(), "the next hour starts at the answer");
        // Off: nothing is due and the clock forgets.
        w.due = true;
        assert!(w.tick(false, 9e9).is_none() && !w.due && w.since.is_none());
    }

    /// Text keeps reading on the blue panels (4.5:1 or more), in both themes.
    #[test]
    fn the_blue_keeps_text_readable() {
        fn lum(c: egui::Color32) -> f64 {
            let f = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.039_28 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        }
        let ratio = |a, b| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        for t in [Tokens::light(), Tokens::dark()] {
            let b = tint(t);
            assert_ne!(b.panel, t.panel, "it turns blue");
            for bg in [b.bg, b.panel, b.field] {
                assert!(ratio(b.text, bg) >= 4.5, "text on {bg:?}: {:.2}", ratio(b.text, bg));
            }
        }
        // The card's text over its water.
        for bg in [CARD, WATER_BACK, WATER_FRONT] {
            for fg in [egui::Color32::WHITE, BODY_TEXT] {
                assert!(ratio(fg, bg) >= 4.5, "{fg:?} on {bg:?}: {:.2}", ratio(fg, bg));
            }
        }
    }

    /// The water stays inside the card and its rounded bottom corners whatever the wave, and
    /// more water stands higher.
    #[test]
    fn the_water_stays_in_the_card_and_rises() {
        let card = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 120.0));
        let corners = [egui::pos2(card.min.x + RADIUS, card.max.y - RADIUS), egui::pos2(card.max.x - RADIUS, card.max.y - RADIUS)];
        for phase in [0.0, 1.0, 2.5] {
            let m = water(card, 0.55, phase, 4.5, CARD);
            assert!(!m.indices.is_empty());
            for v in &m.vertices {
                assert!(card.expand(1e-3).contains(v.pos), "{:?} outside the card", v.pos);
                for c in corners {
                    let outward = if c.x < card.center().x { v.pos.x < c.x } else { v.pos.x > c.x };
                    if outward && v.pos.y > c.y {
                        assert!(v.pos.distance(c) <= RADIUS + 1e-3, "{:?} past the rounded corner", v.pos);
                    }
                }
            }
        }
        let surface = |level: f32| water(card, level, 0.0, 4.0, CARD).vertices.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min);
        assert!(surface(0.5) < surface(0.2), "more water stands higher");
        assert!(water(card, 0.0, 0.0, 4.0, CARD).vertices.is_empty(), "no water, nothing drawn");
    }
}
