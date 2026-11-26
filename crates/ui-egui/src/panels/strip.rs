//! The vertical tool strip at the far right (Presets, Edit, Crop, Remove, Masking, Red Eye …).

use egui::vec2;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// Height of one tool button.
const BUTTON_H: f32 = 40.0;
/// Space between tool buttons.
const GAP: f32 = 6.0;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::right("tool_strip")
        .exact_size(t.strip_w)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.chrome)
                .inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 8 })
                .stroke(egui::Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| {
            crate::widgets::announce_region(ui, egui::accesskit::Role::Toolbar, crate::i18n::tr("Tool strip"));
            let has_photo = app.session.active().is_some();
            // one column that scrolls when the strip is too short for it (a short window or a raised
            // interface zoom), so no button is drawn over another or left out of reach; the column
            // is as wide as the buttons, which are a little wider than the strip's inside, so the
            // strip takes the same room beside the panels as when the buttons were laid out directly
            let inside = ui.available_rect_before_wrap();
            let column = egui::Rect::from_center_size(inside.center(), vec2(t.strip_w, inside.height()));
            let mut column_ui = ui.new_child(egui::UiBuilder::new().max_rect(column).layout(*ui.layout()));
            egui::ScrollArea::vertical().id_salt("tool_strip").auto_shrink([false, true]).show(&mut column_ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = GAP;
                    let sz = vec2(t.strip_w, BUTTON_H);
                    if tool(ui, "presets", Icon::Presets, sz, app.ui.presets, has_photo, "Presets (Shift+P)").clicked() {
                        let _ = app.run("panel.presets", json!({}));
                    }
                    for (id, icon, panel, tip) in [
                        ("edit", Icon::Sliders, RightPanel::Edit, "Edit (E)"),
                        ("crop", Icon::Crop, RightPanel::Crop, "Crop & Rotate (C)"),
                        ("remove", Icon::Eraser, RightPanel::Remove, "Remove (H)"),
                        ("masking", Icon::Mask, RightPanel::Masking, "Masking (M)"),
                        ("redeye", Icon::Eye, RightPanel::RedEye, "Red Eye"),
                    ] {
                        let on = app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles);
                        if tool(ui, id, icon, sz, on, has_photo, tip).clicked() {
                            let _ = app.run(&format!("panel.{id}"), json!({}));
                        }
                        if id == "edit" {
                            separator(ui, &t);
                        }
                    }
                    separator(ui, &t);
                    if tool(ui, "versions", Icon::Versions, sz, app.ui.right == RightPanel::Versions, has_photo, "Versions (Shift+V)").clicked() {
                        let _ = app.run("panel.versions", json!({}));
                    }
                    if tool(ui, "activity", Icon::Activity, sz, app.ui.right == RightPanel::Activity, true, "History & Activity (Y)").clicked() {
                        let _ = app.run("panel.activity", json!({}));
                    }
                    if tool(ui, "more", Icon::More, sz, false, true, "More").clicked() {
                        app.ui.dialog = Some(crate::state::Dialog::About);
                    }
                    // Keywords and Info: two buttons and one gap tall, so Info ends at the bottom
                    // margin as Presets starts at the top one; when there is no room for that they
                    // follow More and the strip scrolls
                    let used = ui.min_rect().height() + GAP;
                    ui.add_space((inside.height() - used - (2.0 * BUTTON_H + GAP)).max(0.0));
                    if tool(ui, "keywords", Icon::Tag, sz, app.ui.right == RightPanel::Keywords, has_photo, "Keywords (K)").clicked() {
                        let _ = app.run("panel.keywords", json!({}));
                    }
                    if tool(ui, "info", Icon::Info, sz, app.ui.right == RightPanel::Info, has_photo, "Info (I)").clicked() {
                        let _ = app.run("panel.info", json!({}));
                    }
                });
            });
            ui.advance_cursor_after_rect(column_ui.min_rect());
        });
}

/// An [`icon_button`] that scrolls into view when it takes the keyboard focus, so Tab never
/// lands on a button the strip has scrolled out of sight.
fn tool(ui: &mut egui::Ui, id: &str, icon: Icon, size: egui::Vec2, active: bool, enabled: bool, tip: &str) -> egui::Response {
    let resp = icon_button(ui, id, icon, size, active, enabled, tip);
    if resp.gained_focus() {
        resp.scroll_to_me(None);
    }
    resp
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(t.strip_w - 16.0, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.button_border.gamma_multiply(0.7));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use crate::headless::HeadlessView;

    /// The strip's widgets after a few frames in a window `height` points tall.
    fn strip_widgets(height: f32) -> Vec<(String, egui::Rect)> {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        let mut view = HeadlessView::new();
        let mut widgets = vec![];
        for frame in 0..3 {
            let raw = HeadlessView::raw_input(vec2(300.0, height), 1.0, frame as f64 / 60.0, vec![]);
            view.run(raw, |ui| show(&mut app, ui));
            widgets = crate::widgets::take_registry(&view.ctx);
        }
        widgets
    }

    #[test]
    fn info_ends_as_far_above_the_bottom_as_presets_starts_below_the_top() {
        let widgets = strip_widgets(700.0);
        let rect = |id: &str| widgets.iter().find(|(w, _)| w == id).unwrap_or_else(|| panic!("{id} not registered")).1;
        let (presets, keywords, info) = (rect("icon:presets"), rect("icon:keywords"), rect("icon:info"));
        let (top, bottom) = (presets.top(), 700.0 - info.bottom());
        assert!((top - bottom).abs() < 0.5, "Presets starts {top} below the top, Info ends {bottom} above the bottom");
        assert_eq!(info.top() - keywords.bottom(), GAP, "the bottom buttons are spaced like the top ones");
    }

    /// A window too short for both groups (the app's minimum height, or a raised interface zoom):
    /// Keywords and Info follow More instead of being drawn over it (#4711).
    #[test]
    fn no_button_covers_another_when_the_strip_is_short() {
        let buttons: Vec<_> = strip_widgets(400.0).into_iter().filter(|(id, _)| id.starts_with("icon:")).collect();
        assert!(["icon:more", "icon:keywords"].iter().all(|id| buttons.iter().any(|(b, _)| b == id)), "{buttons:?}");
        for (i, (a, ra)) in buttons.iter().enumerate() {
            for (b, rb) in &buttons[i + 1..] {
                assert!(!ra.intersects(*rb), "{a} {ra:?} overlaps {b} {rb:?}");
            }
        }
    }
}
