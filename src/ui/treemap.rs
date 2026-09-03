//! Squarified treemap: layout, painting and hit-testing.

use crate::app::{ItemAction, Session};
use crate::model::{fmt_size, NodeId, Tree};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use std::hash::{Hash, Hasher};

const MAX_DEPTH: u8 = 4;
/// Directories smaller than this (in px) are not subdivided.
const MIN_SUBDIVIDE: f32 = 28.0;
const PAD: f32 = 2.0;
const TITLE_H: f32 = 16.0;

pub struct Item {
    pub id: NodeId,
    pub rect: Rect,
    pub depth: u8,
    pub is_dir: bool,
    /// Directory rect is tall enough to carry a title bar.
    pub title: bool,
}

#[derive(Default)]
pub struct TreemapCache {
    key: Option<(NodeId, [i32; 4], u64)>,
    items: Vec<Item>,
    /// Item that was right-clicked; drives the context menu.
    pub ctx_item: Option<NodeId>,
    hover_info: Option<(NodeId, u64, u64)>,
}

impl TreemapCache {
    fn ensure(&mut self, tree: &Tree, view: NodeId, rect: Rect, generation: u64) {
        let key = (
            view,
            [
                rect.min.x as i32,
                rect.min.y as i32,
                rect.width() as i32,
                rect.height() as i32,
            ],
            generation,
        );
        if self.key != Some(key) {
            self.items.clear();
            layout_children(tree, view, rect.shrink(1.0), 0, &mut self.items);
            self.key = Some(key);
            self.ctx_item = None;
            self.hover_info = None;
        }
    }
}

/// Lay out `sizes` (already scaled to pixel areas, sorted descending) inside
/// `rect` with the squarified algorithm. Returns one rect per input.
pub fn squarify(sizes: &[f64], rect: Rect) -> Vec<Rect> {
    let n = sizes.len();
    let mut out = vec![Rect::NOTHING; n];
    if n == 0 {
        return out;
    }
    let mut x = rect.min.x as f64;
    let mut y = rect.min.y as f64;
    let mut w = rect.width() as f64;
    let mut h = rect.height() as f64;

    let worst = |sum: f64, mn: f64, mx: f64, side: f64| -> f64 {
        if sum <= 0.0 || side <= 0.0 || mn <= 0.0 {
            return f64::INFINITY;
        }
        let s2 = side * side;
        let sum2 = sum * sum;
        (s2 * mx / sum2).max(sum2 / (s2 * mn))
    };

    let mut i = 0;
    while i < n {
        if w <= 0.0 || h <= 0.0 {
            break;
        }
        let side = w.min(h);
        let mut end = i + 1;
        let mut row_sum = sizes[i];
        let mut cur_worst = worst(row_sum, sizes[i], sizes[i], side);
        while end < n {
            let s = sizes[end];
            let new_sum = row_sum + s;
            let new_worst = worst(new_sum, s, sizes[i], side);
            if new_worst <= cur_worst {
                cur_worst = new_worst;
                row_sum = new_sum;
                end += 1;
            } else {
                break;
            }
        }
        if w >= h {
            // vertical strip on the left
            let strip_w = if h > 0.0 { row_sum / h } else { 0.0 };
            let mut cy = y;
            for j in i..end {
                let ih = if strip_w > 0.0 { sizes[j] / strip_w } else { 0.0 };
                out[j] = Rect::from_min_size(pos2(x as f32, cy as f32), vec2(strip_w as f32, ih as f32));
                cy += ih;
            }
            x += strip_w;
            w -= strip_w;
        } else {
            // horizontal strip on the top
            let strip_h = if w > 0.0 { row_sum / w } else { 0.0 };
            let mut cx = x;
            for j in i..end {
                let iw = if strip_h > 0.0 { sizes[j] / strip_h } else { 0.0 };
                out[j] = Rect::from_min_size(pos2(cx as f32, y as f32), vec2(iw as f32, strip_h as f32));
                cx += iw;
            }
            y += strip_h;
            h -= strip_h;
        }
        i = end;
    }
    out
}

fn layout_children(tree: &Tree, id: NodeId, rect: Rect, depth: u8, out: &mut Vec<Item>) {
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let node = tree.node(id);
    let ids: Vec<NodeId> = node
        .children
        .iter()
        .copied()
        .filter(|&c| tree.node(c).size > 0)
        .collect();
    if ids.is_empty() {
        return;
    }
    let total: f64 = ids.iter().map(|&c| tree.node(c).size as f64).sum();
    let area = (rect.width() * rect.height()) as f64;
    let scale = area / total;
    let sizes: Vec<f64> = ids.iter().map(|&c| tree.node(c).size as f64 * scale).collect();
    let rects = squarify(&sizes, rect);

    for (k, &cid) in ids.iter().enumerate() {
        let r = rects[k];
        if r.width() < 1.0 || r.height() < 1.0 {
            continue;
        }
        let n = tree.node(cid);
        let title = n.is_dir && r.width() >= 50.0 && r.height() >= TITLE_H + 14.0;
        out.push(Item {
            id: cid,
            rect: r,
            depth,
            is_dir: n.is_dir,
            title,
        });
        if n.is_dir
            && !n.error
            && depth + 1 < MAX_DEPTH
            && r.width() >= MIN_SUBDIVIDE
            && r.height() >= MIN_SUBDIVIDE
        {
            let top = if title { TITLE_H } else { PAD };
            let inner = Rect::from_min_max(
                pos2(r.min.x + PAD, r.min.y + top),
                pos2(r.max.x - PAD, r.max.y - PAD),
            );
            layout_children(tree, cid, inner, depth + 1, out);
        }
    }
}

fn hit_test(items: &[Item], p: Pos2) -> Option<NodeId> {
    let mut best: Option<(u8, NodeId)> = None;
    for it in items {
        if it.rect.contains(p) {
            match best {
                Some((d, _)) if d >= it.depth => {}
                _ => best = Some((it.depth, it.id)),
            }
        }
    }
    best.map(|(_, id)| id)
}

fn file_color(name: &str) -> Color32 {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let hue = match ext {
        Some(e) if !e.is_empty() => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            e.hash(&mut h);
            (h.finish() % 360) as f32 / 360.0
        }
        _ => 0.58,
    };
    Color32::from(egui::ecolor::Hsva::new(hue, 0.55, 0.72, 1.0))
}

fn dir_color(depth: u8) -> Color32 {
    let g = 48 + depth.min(6) * 10;
    Color32::from_rgb(g, g + 4, g + 10)
}

fn lighten(c: Color32, amt: u8) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_add(amt),
        c.g().saturating_add(amt),
        c.b().saturating_add(amt),
    )
}

pub fn show(ui: &mut Ui, sess: &mut Session, actions: &mut Vec<ItemAction>) {
    let Session {
        tree,
        treemap: cache,
        view,
        selected,
        generation,
        ..
    } = sess;
    let view = *view;
    let selected = *selected;

    let avail = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(avail, Sense::click());
    if !ui.is_rect_visible(rect) || rect.width() < 4.0 || rect.height() < 4.0 {
        return;
    }
    cache.ensure(tree, view, rect, *generation);

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(22));

    let hovered = response.hover_pos().and_then(|p| hit_test(&cache.items, p));

    let font_small = FontId::proportional(11.0);
    let text_col = Color32::from_gray(235);
    for it in &cache.items {
        let n = tree.node(it.id);
        let base = if n.error {
            Color32::from_rgb(120, 45, 45)
        } else if n.is_dir {
            dir_color(it.depth)
        } else {
            file_color(&n.name)
        };
        let is_hover = hovered == Some(it.id);
        let is_sel = selected == Some(it.id);
        let fill = if is_hover { lighten(base, 35) } else { base };
        painter.rect_filled(it.rect, 0.0, fill);
        let stroke = if is_sel {
            Stroke::new(2.0, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_black_alpha(150))
        };
        painter.rect_stroke(it.rect, 0.0, stroke, StrokeKind::Inside);

        let inner = it.rect.shrink(2.0);
        let clip = painter.with_clip_rect(inner);
        if it.is_dir && it.title {
            let label = format!("{}  {}", n.name, fmt_size(n.size));
            clip.text(
                pos2(it.rect.min.x + 4.0, it.rect.min.y + 2.0),
                Align2::LEFT_TOP,
                label,
                font_small.clone(),
                text_col,
            );
        } else if it.rect.width() >= 36.0 && it.rect.height() >= 14.0 {
            let galley = painter.layout_no_wrap(n.name.clone(), font_small.clone(), text_col);
            let size = galley.size();
            if size.x <= inner.width() {
                let pos = if it.rect.height() >= 30.0 {
                    // name centered, size below
                    let p = pos2(inner.center().x - size.x / 2.0, inner.center().y - size.y);
                    clip.text(
                        pos2(inner.center().x, inner.center().y + 1.0),
                        Align2::CENTER_TOP,
                        fmt_size(n.size),
                        FontId::proportional(10.0),
                        Color32::from_gray(210),
                    );
                    p
                } else {
                    pos2(inner.center().x - size.x / 2.0, inner.center().y - size.y / 2.0)
                };
                clip.galley(pos, galley, text_col);
            } else {
                clip.galley(
                    pos2(inner.min.x, inner.center().y - size.y / 2.0),
                    galley,
                    text_col,
                );
            }
        }
    }

    if cache.items.is_empty() {
        let n = tree.node(view);
        let msg = if n.error {
            "This folder could not be read (access denied?)"
        } else {
            "Empty folder"
        };
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            msg,
            FontId::proportional(16.0),
            Color32::from_gray(140),
        );
    }

    // Interaction
    if response.secondary_clicked() {
        cache.ctx_item = hovered;
    }
    if response.clicked() {
        if let Some(id) = hovered {
            actions.push(ItemAction::Select(id));
        }
    }
    if response.double_clicked() {
        if let Some(id) = hovered {
            if tree.node(id).is_dir {
                actions.push(ItemAction::Zoom(id));
            } else {
                actions.push(ItemAction::Open(id));
            }
        }
    }

    let mut response = response;
    if let Some(id) = hovered {
        let (files, dirs) = match cache.hover_info {
            Some((hid, f, d)) if hid == id => (f, d),
            _ => {
                let (f, d) = if tree.node(id).is_dir { tree.count_items(id) } else { (0, 0) };
                cache.hover_info = Some((id, f, d));
                (f, d)
            }
        };
        let n = tree.node(id);
        let path = tree.path(id);
        response = response.on_hover_ui_at_pointer(|ui| {
            ui.label(egui::RichText::new(&n.name).strong());
            ui.label(egui::RichText::new(path.display().to_string()).weak());
            ui.label(fmt_size(n.size));
            if n.is_dir {
                ui.label(format!("{files} files, {dirs} folders"));
                if n.error {
                    ui.colored_label(Color32::LIGHT_RED, "Could not be read");
                }
                ui.label(egui::RichText::new("Double-click to zoom in").weak());
            } else {
                ui.label(egui::RichText::new("Double-click to open").weak());
            }
        });
    }

    if let Some(id) = cache.ctx_item {
        if (id as usize) < tree.nodes.len() {
            response.context_menu(|ui| super::item_context_menu(ui, tree, id, actions));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_tiles_and_preserves_area() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let raw = [50.0, 25.0, 12.5, 6.25, 3.125, 3.125];
        let total: f64 = raw.iter().sum();
        let area = 400.0 * 300.0;
        let sizes: Vec<f64> = raw.iter().map(|s| s / total * area).collect();
        let rects = squarify(&sizes, rect);
        assert_eq!(rects.len(), sizes.len());
        let mut sum = 0.0f64;
        for (r, s) in rects.iter().zip(sizes.iter()) {
            let a = (r.width() * r.height()) as f64;
            assert!((a - s).abs() < 1e-2, "area {a} vs {s}");
            sum += a;
            assert!(rect.contains_rect(r.expand(-0.01)), "{r:?} outside {rect:?}");
        }
        assert!((sum - area).abs() < 1e-1);
        // no overlaps
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let inter = rects[i].intersect(rects[j]);
                assert!(inter.width() <= 0.01 || inter.height() <= 0.01, "overlap {i} {j}");
            }
        }
    }

    #[test]
    fn squarify_handles_empty_and_single() {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 50.0));
        assert!(squarify(&[], rect).is_empty());
        let r = squarify(&[5000.0], rect);
        assert_eq!(r.len(), 1);
        assert!((r[0].width() - 100.0).abs() < 1e-3);
        assert!((r[0].height() - 50.0).abs() < 1e-3);
    }
}
