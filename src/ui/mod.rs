//! Screen composition for the "Ready" state plus shared widgets.

pub mod dialogs;
pub mod search;
pub mod start;
pub mod tree_panel;
pub mod treemap;

use crate::app::{AppEvent, ItemAction, Session, Validation};
use crate::model::{fmt_size, NodeId, Tree};
use egui::{Align, Button, Key, Layout, Modifiers, RichText, Ui};

pub fn show_ready(
    ui: &mut Ui,
    sess: &mut Session,
    events: &mut Vec<AppEvent>,
    actions: &mut Vec<ItemAction>,
) {
    let ctx = ui.ctx().clone();
    search::tick(sess);

    // Keyboard shortcuts (only when no text field has focus).
    if !ctx.egui_wants_keyboard_input() && sess.pending_delete.is_none() {
        if ctx.input(|i| i.key_pressed(Key::Backspace)) {
            actions.push(ItemAction::ZoomOut);
        }
        if ctx.input(|i| i.key_pressed(Key::Delete)) {
            if let Some(s) = sess.selected {
                actions.push(ItemAction::Delete(s));
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(s) = sess.selected {
                if sess.tree.node(s).is_dir {
                    actions.push(ItemAction::Zoom(s));
                } else {
                    actions.push(ItemAction::Open(s));
                }
            }
        }
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::F)) {
        ctx.memory_mut(|m| m.request_focus(search::search_id()));
    }

    egui::Panel::top("top_bar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("< Start").clicked() {
                events.push(AppEvent::BackToStart);
            }
            ui.separator();
            ui.label(RichText::new(sess.root.display().to_string()).strong());
            ui.separator();
            let scanning = sess.is_scanning();
            if ui.add_enabled(!scanning, Button::new("Rescan")).clicked() {
                events.push(AppEvent::Rescan);
            }
            match &sess.validation {
                Validation::Running(job) => {
                    ui.spinner();
                    let p = &job.progress;
                    ui.label(format!(
                        "validating… {} files, {}",
                        p.files.load(std::sync::atomic::Ordering::Relaxed),
                        fmt_size(p.bytes.load(std::sync::atomic::Ordering::Relaxed))
                    ));
                }
                Validation::Done { summary, .. } => {
                    let text = if summary.is_empty() {
                        "validated, no changes".to_string()
                    } else {
                        format!(
                            "validated: +{} -{} ~{}",
                            summary.added, summary.removed, summary.changed
                        )
                    };
                    ui.label(RichText::new(text).color(egui::Color32::from_rgb(120, 200, 120)));
                }
                Validation::Failed(msg) => {
                    ui.label(RichText::new("validation failed").color(egui::Color32::LIGHT_RED))
                        .on_hover_text(msg);
                }
                Validation::NotStarted => {}
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                search::show_search_box(ui, &mut sess.search);
            });
        });
        ui.add_space(4.0);
    });

    egui::Panel::bottom("status_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(&sess.status);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let t = &sess.tree;
                let mut totals = format!(
                    "{} files, {} folders, {}",
                    t.file_count,
                    t.dir_count,
                    fmt_size(t.total_size())
                );
                if t.error_count > 0 {
                    totals.push_str(&format!(", {} unreadable", t.error_count));
                }
                ui.label(RichText::new(totals).weak());
            });
        });
    });

    if !sess.search.applied.is_empty() {
        egui::Panel::bottom("search_results")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| search::show_results(ui, sess, actions));
    }

    egui::Panel::left("tree_panel")
        .resizable(true)
        .default_size(360.0)
        .show(ui, |ui| tree_panel::show(ui, sess, actions));

    egui::CentralPanel::default().show(ui, |ui| {
        breadcrumb(ui, sess, actions);
        ui.add_space(2.0);
        treemap::show(ui, sess, actions);
    });

    dialogs::show(&ctx, sess, events);

    // Consumed by the tree panel during this frame.
    sess.expand_to = None;
}

fn breadcrumb(ui: &mut Ui, sess: &Session, actions: &mut Vec<ItemAction>) {
    let tree = &sess.tree;
    ui.horizontal_wrapped(|ui| {
        let anc = tree.ancestors(sess.view);
        for (i, &a) in anc.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("›").weak());
            }
            let n = tree.node(a);
            let last = i + 1 == anc.len();
            let text = format!("{}  ({})", n.name, fmt_size(n.size));
            if last {
                ui.label(RichText::new(text).strong());
            } else if ui.link(text).clicked() {
                actions.push(ItemAction::Zoom(a));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_up = sess.view != tree.root;
            if ui.add_enabled(can_up, Button::new("Up")).clicked() {
                actions.push(ItemAction::ZoomOut);
            }
            if let Some(s) = sess.selected {
                let n = tree.node(s);
                ui.label(
                    RichText::new(format!("Selected: {}  ({})", n.name, fmt_size(n.size))).weak(),
                );
            }
        });
    });
}

/// Right-click menu shared by the treemap, tree panel and search results.
pub fn item_context_menu(ui: &mut Ui, tree: &Tree, id: NodeId, actions: &mut Vec<ItemAction>) {
    let n = tree.node(id);
    ui.set_min_width(200.0);
    ui.label(RichText::new(&n.name).strong());
    ui.label(RichText::new(fmt_size(n.size)).weak());
    ui.separator();
    if n.is_dir && ui.button("Zoom in").clicked() {
        actions.push(ItemAction::Zoom(id));
        ui.close();
    }
    if ui.button("Open").clicked() {
        actions.push(ItemAction::Open(id));
        ui.close();
    }
    if ui.button("Reveal in Explorer").clicked() {
        actions.push(ItemAction::Reveal(id));
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        actions.push(ItemAction::CopyPath(id));
        ui.close();
    }
    ui.separator();
    if id != tree.root
        && ui
            .button(RichText::new("Delete (Recycle Bin)…").color(egui::Color32::LIGHT_RED))
            .clicked()
    {
        actions.push(ItemAction::Delete(id));
        ui.close();
    }
}
