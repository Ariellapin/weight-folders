//! Name search over the loaded snapshot with a small results panel.

use crate::app::{ItemAction, Session};
use crate::model::{fmt_size, NodeId, Tree};
use egui::{Align, Id, Layout, RichText, TextEdit, Ui};
use std::cmp::Reverse;
use std::time::{Duration, Instant};

const MAX_RESULTS: usize = 1000;
const DEBOUNCE: Duration = Duration::from_millis(150);

pub fn search_id() -> Id {
    Id::new("search_box")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    All,
    Files,
    Folders,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// Query the current results were computed for.
    pub applied: String,
    pub kind: Kind,
    pub min_mb: f64,
    pub results: Vec<NodeId>,
    pub total_matches: usize,
    pub changed_at: Option<Instant>,
    /// Force a re-run (tree changed or filter changed).
    pub dirty: bool,
}

pub fn show_search_box(ui: &mut Ui, st: &mut SearchState) {
    if !st.query.is_empty() && ui.small_button("x").clicked() {
        st.query.clear();
        st.dirty = true;
    }
    let r = ui.add(
        TextEdit::singleline(&mut st.query)
            .id(search_id())
            .hint_text("Search names (Ctrl+F, * and ? wildcards)")
            .desired_width(300.0),
    );
    if r.changed() {
        st.changed_at = Some(Instant::now());
    }
}

/// Run the search when the debounce elapsed or something marked it dirty.
pub fn tick(sess: &mut Session) {
    let st = &mut sess.search;
    let due = st
        .changed_at
        .map(|t| t.elapsed() >= DEBOUNCE)
        .unwrap_or(false);
    if due || st.dirty {
        st.changed_at = None;
        st.dirty = false;
        run(&sess.tree, st);
    }
}

fn run(tree: &Tree, st: &mut SearchState) {
    st.applied = st.query.trim().to_string();
    st.results.clear();
    st.total_matches = 0;
    if st.applied.is_empty() {
        return;
    }
    let pat: String = st.applied.chars().flat_map(|c| c.to_lowercase()).collect();
    let wildcard = pat.contains(['*', '?']);
    let min = (st.min_mb.max(0.0) * 1024.0 * 1024.0) as u64;
    let mut buf = String::new();
    for (i, n) in tree.nodes.iter().enumerate() {
        if i as NodeId == tree.root {
            continue;
        }
        match st.kind {
            Kind::Files if n.is_dir => continue,
            Kind::Folders if !n.is_dir => continue,
            _ => {}
        }
        if n.size < min {
            continue;
        }
        buf.clear();
        buf.extend(n.name.chars().flat_map(|c| c.to_lowercase()));
        let hit = if wildcard {
            wild_match(&pat, &buf)
        } else {
            buf.contains(pat.as_str())
        };
        if hit {
            st.results.push(i as NodeId);
        }
    }
    st.total_matches = st.results.len();
    st.results
        .sort_by_key(|&id| Reverse(tree.node(id).size));
    st.results.truncate(MAX_RESULTS);
}

/// Glob-style match: `*` any run, `?` one char. Whole-name match.
pub fn wild_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn show_results(ui: &mut Ui, sess: &mut Session, actions: &mut Vec<ItemAction>) {
    let Session {
        tree,
        search: st,
        selected,
        ..
    } = sess;
    let selected = *selected;

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let shown = st.results.len();
        let text = if st.total_matches > shown {
            format!(
                "{} matches for \"{}\" (showing largest {shown})",
                st.total_matches, st.applied
            )
        } else {
            format!("{shown} matches for \"{}\"", st.applied)
        };
        ui.label(RichText::new(text).strong());
        ui.separator();
        let mut changed = false;
        changed |= ui.selectable_value(&mut st.kind, Kind::All, "All").changed();
        changed |= ui.selectable_value(&mut st.kind, Kind::Files, "Files").changed();
        changed |= ui.selectable_value(&mut st.kind, Kind::Folders, "Folders").changed();
        ui.separator();
        ui.label("Min size (MB):");
        changed |= ui
            .add(egui::DragValue::new(&mut st.min_mb).speed(1.0).range(0.0..=10_000_000.0))
            .changed();
        if changed {
            st.dirty = true;
        }
    });
    ui.separator();

    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let results = &st.results;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_h, results.len(), |ui, range| {
            for i in range {
                let id = results[i];
                let n = tree.node(id);
                ui.horizontal(|ui| {
                    let icon = if n.is_dir { "[D]" } else { "[F]" };
                    let r = ui.selectable_label(
                        selected == Some(id),
                        format!("{icon} {}", n.name),
                    );
                    if r.clicked() {
                        actions.push(ItemAction::Zoom(id));
                    }
                    if r.double_clicked() {
                        actions.push(ItemAction::Open(id));
                    }
                    r.context_menu(|ui| super::item_context_menu(ui, tree, id, actions));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(tree.path(id).display().to_string()).weak());
                        ui.separator();
                        ui.label(RichText::new(fmt_size(n.size)).monospace());
                    });
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wild_match("*.rs", "main.rs"));
        assert!(!wild_match("*.rs", "main.rsx"));
        assert!(wild_match("ma?n.*", "main.rs"));
        assert!(wild_match("*", "anything"));
        assert!(wild_match("a*b*c", "axxbyyc"));
        assert!(!wild_match("a*b*c", "axxbyy"));
        assert!(wild_match("*target*", "my target dir"));
        assert!(!wild_match("abc", "abcd"));
    }
}
