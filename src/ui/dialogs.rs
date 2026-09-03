//! Delete confirmation modal and transient error toast.

use crate::app::{AppEvent, Session};
use crate::model::fmt_size;
use egui::{Align2, Color32, Id, Modal, RichText};
use std::time::Duration;

const ERROR_TTL: Duration = Duration::from_secs(8);

pub fn show(ctx: &egui::Context, sess: &mut Session, events: &mut Vec<AppEvent>) {
    if let Some(id) = sess.pending_delete {
        let tree = &sess.tree;
        let n = tree.node(id);
        let path = tree.path(id);
        let (files, dirs) = if n.is_dir { tree.count_items(id) } else { (0, 0) };
        let modal = Modal::new(Id::new("delete_modal")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Move to Recycle Bin?");
            ui.add_space(6.0);
            ui.label(RichText::new(&n.name).strong().size(16.0));
            ui.label(RichText::new(path.display().to_string()).weak());
            ui.label(fmt_size(n.size));
            if n.is_dir {
                ui.label(format!("Contains {files} files and {dirs} folders"));
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("The item is moved to the Windows Recycle Bin and can be restored from there.")
                    .weak(),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Move to Recycle Bin").color(Color32::LIGHT_RED))
                    .clicked()
                {
                    events.push(AppEvent::ConfirmDelete);
                }
                if ui.button("Cancel").clicked() {
                    events.push(AppEvent::CancelDelete);
                }
            });
        });
        if modal.should_close() {
            events.push(AppEvent::CancelDelete);
        }
    }

    if let Some((msg, at)) = sess.error.clone() {
        if at.elapsed() > ERROR_TTL {
            sess.error = None;
        } else {
            egui::Window::new("Error")
                .anchor(Align2::RIGHT_TOP, [-12.0, 48.0])
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.colored_label(Color32::LIGHT_RED, msg);
                    if ui.button("Dismiss").clicked() {
                        events.push(AppEvent::DismissError);
                    }
                });
        }
    }

    if sess.deleting.is_some() {
        egui::Window::new("Deleting")
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .title_bar(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Moving to the Recycle Bin…");
                });
            });
    }
}
