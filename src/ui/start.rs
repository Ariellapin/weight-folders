//! Start screen (pick a drive/folder) and the scanning progress screen.

use crate::app::{AppEvent, ScanJob, StartState};
use crate::model::{fmt_size, fmt_time};
use crate::snapshot;
use egui::{Button, Color32, RichText, TextEdit};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Trim trailing separators (keeping drive roots like `C:\`).
pub fn normalize_input_path(s: &str) -> PathBuf {
    let t = s.trim();
    let trimmed = t.trim_end_matches(['\\', '/']);
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        PathBuf::from(format!("{trimmed}\\"))
    } else if trimmed.is_empty() {
        PathBuf::from(t)
    } else {
        PathBuf::from(trimmed)
    }
}

pub fn show_start(ui: &mut egui::Ui, s: &mut StartState, events: &mut Vec<AppEvent>) {
    let StartState {
        path,
        drives,
        recent,
        error,
    } = s;

    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading(RichText::new("Weight Folders").size(28.0));
            ui.label("Pick a drive or folder, then click Scan.");
        });
        ui.add_space(20.0);

        let max_w = 760.0f32.min(ui.available_width() - 16.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(max_w);
            ui.group(|ui| {
                ui.set_width(max_w - 16.0);
                ui.label(RichText::new("Drives").strong());
                ui.horizontal_wrapped(|ui| {
                    for d in drives.iter() {
                        let selected = path.trim().eq_ignore_ascii_case(d);
                        if ui.selectable_label(selected, RichText::new(d).size(16.0)).clicked() {
                            *path = d.clone();
                        }
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Folder").strong());
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(path)
                            .hint_text(r"C:\Users\...")
                            .desired_width(ui.available_width() - 110.0),
                    );
                    if ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            *path = p.display().to_string();
                        }
                    }
                });

                let target = normalize_input_path(path);
                let ok = !path.trim().is_empty() && target.is_dir();
                ui.add_space(6.0);
                if ok {
                    if snapshot::exists(&target) {
                        ui.label(
                            RichText::new(
                                "Snapshot found — it will load instantly and be validated in the background.",
                            )
                            .color(Color32::from_rgb(120, 200, 120)),
                        );
                    } else {
                        ui.label(RichText::new("No snapshot yet — a full scan will run.").weak());
                    }
                } else if !path.trim().is_empty() {
                    ui.label(RichText::new("That path is not an existing folder.").color(Color32::LIGHT_RED));
                } else {
                    ui.label(RichText::new(" ").weak());
                }
                ui.add_space(6.0);
                if ui
                    .add_enabled(ok, Button::new(RichText::new("Scan").size(18.0)).min_size([140.0, 34.0].into()))
                    .clicked()
                {
                    events.push(AppEvent::StartScan(target));
                }
                if let Some(e) = error {
                    ui.add_space(4.0);
                    ui.colored_label(Color32::LIGHT_RED, e.as_str());
                }
            });

            if !recent.is_empty() {
                ui.add_space(18.0);
                ui.label(RichText::new("Recent snapshots").strong());
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    egui::Grid::new("recent_grid")
                        .striped(true)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for r in recent.iter() {
                                let root = r.root.display().to_string();
                                if ui.link(&root).clicked() {
                                    *path = root.clone();
                                }
                                ui.label(fmt_time(r.created_at));
                                ui.label(format!("{} files", r.file_count));
                                ui.label(fmt_size(r.total_size));
                                ui.end_row();
                            }
                        });
                });
            }
        });
    });
}

pub fn show_scanning(ui: &mut egui::Ui, job: &ScanJob, events: &mut Vec<AppEvent>) {
    ui.ctx().request_repaint_after(Duration::from_millis(100));
    let p = &job.progress;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.heading(format!("Scanning {}", job.root.display()));
            ui.add_space(12.0);
            ui.spinner();
            ui.add_space(12.0);
            let secs = job.started.elapsed().as_secs_f32();
            ui.label(
                RichText::new(format!(
                    "{} files   •   {} folders   •   {}",
                    p.files.load(Ordering::Relaxed),
                    p.dirs.load(Ordering::Relaxed),
                    fmt_size(p.bytes.load(Ordering::Relaxed))
                ))
                .size(18.0),
            );
            let errs = p.errors.load(Ordering::Relaxed);
            if errs > 0 {
                ui.label(RichText::new(format!("{errs} folders could not be read")).weak());
            }
            ui.label(RichText::new(format!("{secs:.1}s elapsed")).weak());
            ui.add_space(8.0);
            let mut cur = p.current();
            if cur.len() > 110 {
                let cut = cur.char_indices().nth_back(105).map(|(i, _)| i).unwrap_or(0);
                cur = format!("…{}", &cur[cut..]);
            }
            ui.label(RichText::new(cur).monospace().weak());
            ui.add_space(16.0);
            if ui.button("Cancel").clicked() {
                events.push(AppEvent::CancelScan);
            }
        });
    });
}
