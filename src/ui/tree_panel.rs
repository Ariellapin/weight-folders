//! Collapsible folder tree synchronized with the treemap selection.

use crate::app::{ItemAction, Session};
use crate::model::{fmt_size, NodeId, Tree};
use egui::collapsing_header::CollapsingState;
use egui::{Align, Id, RichText, Ui};
use std::collections::HashSet;

/// Directories with more children than this show a "… N more" line.
const MAX_CHILDREN: usize = 400;

pub fn show(ui: &mut Ui, sess: &mut Session, actions: &mut Vec<ItemAction>) {
    let tree = &sess.tree;
    let selected = sess.selected;
    let scroll_to = sess.expand_to;
    let expand: HashSet<NodeId> = match scroll_to {
        Some(id) => {
            let mut a = tree.ancestors(id);
            a.pop(); // open ancestors, not the target itself
            a.into_iter().collect()
        }
        None => HashSet::new(),
    };

    ui.add_space(4.0);
    ui.label(RichText::new("Folders").strong());
    ui.separator();
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_node(ui, tree, tree.root, selected, scroll_to, &expand, actions);
        });
}

fn show_node(
    ui: &mut Ui,
    tree: &Tree,
    id: NodeId,
    selected: Option<NodeId>,
    scroll_to: Option<NodeId>,
    expand: &HashSet<NodeId>,
    actions: &mut Vec<ItemAction>,
) {
    let n = tree.node(id);
    let parent_size = n
        .parent
        .map(|p| tree.node(p).size)
        .unwrap_or(n.size)
        .max(1);
    let pct = n.size as f64 * 100.0 / parent_size as f64;
    let is_sel = selected == Some(id);
    let label = if n.parent.is_none() {
        RichText::new(format!("{}   {}", n.name, fmt_size(n.size))).strong()
    } else if n.is_dir {
        let t = RichText::new(format!("{}   {}   {pct:.1}%", n.name, fmt_size(n.size))).strong();
        if n.error {
            t.color(egui::Color32::LIGHT_RED)
        } else {
            t
        }
    } else {
        RichText::new(format!("{}   {}   {pct:.1}%", n.name, fmt_size(n.size)))
    };

    if n.is_dir {
        let mut state =
            CollapsingState::load_with_default_open(ui.ctx(), Id::new(("tree_node", id)), n.parent.is_none());
        if expand.contains(&id) && !state.is_open() {
            state.set_open(true);
        }
        let header = state.show_header(ui, |ui| {
            let r = ui.selectable_label(is_sel, label);
            if r.clicked() {
                actions.push(ItemAction::Select(id));
            }
            if r.double_clicked() {
                actions.push(ItemAction::Zoom(id));
            }
            r.context_menu(|ui| super::item_context_menu(ui, tree, id, actions));
            if scroll_to == Some(id) {
                r.scroll_to_me(Some(Align::Center));
            }
        });
        header.body(|ui| {
            let ch = &n.children;
            for &c in ch.iter().take(MAX_CHILDREN) {
                show_node(ui, tree, c, selected, scroll_to, expand, actions);
            }
            if ch.len() > MAX_CHILDREN {
                ui.label(
                    RichText::new(format!(
                        "… {} more (use search or the treemap)",
                        ch.len() - MAX_CHILDREN
                    ))
                    .weak(),
                );
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            let r = ui.selectable_label(is_sel, label);
            if r.clicked() {
                actions.push(ItemAction::Select(id));
            }
            if r.double_clicked() {
                actions.push(ItemAction::Open(id));
            }
            r.context_menu(|ui| super::item_context_menu(ui, tree, id, actions));
            if scroll_to == Some(id) {
                r.scroll_to_me(Some(Align::Center));
            }
        });
    }
}
