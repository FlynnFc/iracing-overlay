//! First-run license entry, using the same graphite surfaces and Inter type as Settings.

use egui::{Color32, Context, RichText, Stroke};

use super::settings::{CONTENT_BG, MUTED, TEXT, WINDOW_BG, settings_style};

/// Input state only: acceptance and persistence belong to `crate::licence`.
#[derive(Debug, Default)]
pub struct LicensePage {
    pub key: String,
    pub error: Option<String>,
    pub completed: bool,
    reveal: bool,
    focused: bool,
}

/// A user intent, never an assertion of paid entitlement.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    Continue,
    Quit,
}

impl LicensePage {
    /// Draws an opaque, scrollable first-run surface in a focusable native window.
    pub fn draw(&mut self, ctx: &Context) -> Action {
        let mut action = Action::None;
        egui::CentralPanel::default().frame(egui::Frame::none().fill(WINDOW_BG).inner_margin(28.0)).show(ctx, |ui| {
            settings_style(ui);
            ui.visuals_mut().selection.bg_fill = Color32::from_rgb(32, 83, 70);
            ui.visuals_mut().widgets.active.bg_stroke = Stroke::new(1.0, super::ACCENT);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RACE OVERLAY").size(11.0).strong().color(super::ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            action = Action::Quit;
                        }
                    });
                });
                ui.add_space(30.0);
                let mut heading = egui::text::LayoutJob::default();
                for (text, color) in [("Your race. ", TEXT), ("A clearer view.", super::ACCENT)] {
                    heading.append(
                        text,
                        0.0,
                        egui::TextFormat { font_id: egui::FontId::proportional(30.0), color, ..Default::default() },
                    );
                }
                ui.label(heading);
                ui.add_space(6.0);
                ui.label(RichText::new("Activate your overlay and get ready for the grid.").size(14.0).color(MUTED));
                ui.add_space(26.0);
                let form_action = self.form(ui, ctx);
                if form_action != Action::None {
                    action = form_action;
                }
                ui.add_space(20.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("ONE SETUP. EVERY SESSION.").size(10.0).strong().color(MUTED));
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Your setup is remembered on this PC. Next time, head straight to the track.")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Built for race day.").size(11.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).size(11.0).color(MUTED));
                    });
                });
            });
        });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Action::Quit;
        }
        action
    }

    fn form(&mut self, ui: &mut egui::Ui, ctx: &Context) -> Action {
        let mut action = Action::None;
        egui::Frame::none()
            .fill(CONTENT_BG)
            .rounding(12.0)
            .stroke(Stroke::new(1.0, Color32::from_rgb(44, 71, 63)))
            .inner_margin(22.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Enter your license key").size(21.0).strong().color(TEXT));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("You can find it in your purchase email or Polar account.").size(13.0).color(MUTED),
                );
                ui.add_space(20.0);
                ui.label(RichText::new("LICENSE KEY").size(10.0).strong().color(MUTED));
                let field = ui.add_sized(
                    [ui.available_width(), 44.0],
                    egui::TextEdit::singleline(&mut self.key)
                        .id_source("first-run-license")
                        .hint_text("Paste your license key")
                        .password(!self.reveal)
                        .font(egui::TextStyle::Monospace)
                        .margin(egui::vec2(12.0, 12.0))
                        .char_limit(512),
                );
                if !self.focused {
                    field.request_focus();
                    self.focused = true;
                }
                if field.changed() {
                    self.error = None;
                }
                ui.checkbox(&mut self.reveal, RichText::new("Show key").size(12.0).color(MUTED));
                ui.add_space(12.0);
                let enabled = !self.key.trim().is_empty() && !self.completed;
                let button = ui.add_enabled(
                    enabled,
                    egui::Button::new(
                        RichText::new(if self.completed { "Ready to race" } else { "Activate & continue" })
                            .size(14.0)
                            .strong()
                            .color(WINDOW_BG),
                    )
                    .fill(super::ACCENT)
                    .min_size(egui::vec2(ui.available_width(), 44.0)),
                );
                if enabled
                    && (button.clicked()
                        || (field.lost_focus() && ctx.input(|input| input.key_pressed(egui::Key::Enter))))
                {
                    action = Action::Continue;
                }
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.label(RichText::new(error).size(12.0).color(super::ALERT));
                } else if self.completed {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Preview complete. Your settings have not been changed.")
                            .size(12.0)
                            .color(super::SIGNAL),
                    );
                }
            });
        action
    }
}
