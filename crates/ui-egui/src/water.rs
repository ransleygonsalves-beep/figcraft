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

/// The card, while the reminder is due: bottom centre, above the toasts, until answered.
pub fn card(app: &mut FigApp, ctx: &egui::Context) {
    if !app.ui.water.due {
        return;
    }
    let screen = ctx.content_rect();
    let mut answered = false;
    egui::Area::new(egui::Id::new("water_reminder"))
        .fixed_pos(egui::pos2(screen.center().x, screen.max.y - 150.0))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgb(0x0E, 0x2A, 0x4A))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_max_width(360.0);
                    let title = tr("Have you had some water?");
                    let r = ui.label(RichText::new(title).font(semibold(13.0)).color(Color32::WHITE));
                    crate::a11y::alert(&r, title);
                    ui.label(
                        RichText::new(tr("You've been designing for an hour. A glass of water helps you stay sharp."))
                            .font(regular(11.0))
                            .color(Color32::from_rgb(0xD6, 0xE7, 0xF8)),
                    );
                    ui.add_space(8.0);
                    answered = crate::widgets::button(ui, tr("I've had some"), true, true).clicked();
                });
        });
    if answered {
        app.ui.water.answer(ctx.input(|i| i.time));
    }
}

#[cfg(test)]
mod tests {
    use super::{AFTER_SECS, Water, tint};
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
    }
}
