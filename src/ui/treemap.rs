//! Squarified treemap: layout, painting and hit-testing.

use crate::app::{ItemAction, Session};
use crate::model::{fmt_size, NodeId, Tree};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};

/// Nesting is limited by pixel size, not depth; this is only a safety net.
const MAX_DEPTH: u8 = 32;
/// Safety cap on laid-out rectangles per frame.
const MAX_ITEMS: usize = 40_000;
/// Directories smaller than this (in px) are not subdivided.
const MIN_SUBDIVIDE: f32 = 28.0;
/// Rectangles smaller than this (in px) are dropped.
const MIN_ITEM: f32 = 2.0;
const PAD: f32 = 2.0;
const TITLE_H: f32 = 16.0;
const LEGEND_W: f32 = 180.0;
const LEGEND_H: f32 = 10.0;

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
    /// Smallest / largest file size among the laid-out files; drives the
    /// blue-to-red color scale of the current view.
    file_min: u64,
    file_max: u64,
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
            let (mut lo, mut hi) = (u64::MAX, 0u64);
            for it in self.items.iter().filter(|it| !it.is_dir) {
                let s = tree.node(it.id).size;
                lo = lo.min(s);
                hi = hi.max(s);
            }
            if lo == u64::MAX {
                lo = 0;
            }
            self.file_min = lo;
            self.file_max = hi;
            self.key = Some(key);
            self.ctx_item = None;
            self.hover_info = None;
        }
    }

    /// 0.0 for the smallest visible file, 1.0 for the largest, log-scaled in
    /// between so that a mix of KiB and GiB files still spreads out.
    fn size_t(&self, size: u64) -> f32 {
        let lo = (self.file_min.max(1) as f64).ln();
        let hi = (self.file_max.max(1) as f64).ln();
        if hi - lo < 1e-6 {
            return 0.5;
        }
        let s = (size.max(1) as f64).ln();
        (((s - lo) / (hi - lo)).clamp(0.0, 1.0)) as f32
    }
}

/// Blue (smallest) → cyan → green → yellow → red (largest).
pub fn scale_color(t: f32) -> Color32 {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [45.0, 95.0, 225.0]),
        (0.25, [40.0, 190.0, 220.0]),
        (0.5, [70.0, 195.0, 80.0]),
        (0.75, [235.0, 205.0, 50.0]),
        (1.0, [225.0, 50.0, 45.0]),
    ];
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 2 < STOPS.len() && t > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, c0) = STOPS[i];
    let (t1, c1) = STOPS[i + 1];
    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
    let mix = |a: f32, b: f32| (a + (b - a) * f).round() as u8;
    Color32::from_rgb(mix(c0[0], c1[0]), mix(c0[1], c1[1]), mix(c0[2], c1[2]))
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
        if out.len() >= MAX_ITEMS {
            return;
        }
        let r = rects[k];
        if r.width() < MIN_ITEM || r.height() < MIN_ITEM {
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
            scale_color(cache.size_t(n.size))
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

    // Color legend (bottom-right) for the files currently shown.
    if cache.file_max > 0 && rect.width() > LEGEND_W + 160.0 && rect.height() > 60.0 {
        let bar = Rect::from_min_size(
            pos2(rect.max.x - LEGEND_W - 10.0, rect.max.y - LEGEND_H - 8.0),
            vec2(LEGEND_W, LEGEND_H),
        );
        let bg = bar.expand2(vec2(6.0, 4.0));
        painter.rect_filled(bg, 3.0, Color32::from_black_alpha(170));
        let steps = 36;
        let w = bar.width() / steps as f32;
        for i in 0..steps {
            let t = i as f32 / (steps - 1) as f32;
            let r = Rect::from_min_size(pos2(bar.min.x + i as f32 * w, bar.min.y), vec2(w + 0.5, bar.height()));
            painter.rect_filled(r, 0.0, scale_color(t));
        }
        let lf = FontId::proportional(10.0);
        painter.text(
            pos2(bar.min.x - 8.0, bar.center().y),
            Align2::RIGHT_CENTER,
            fmt_size(cache.file_min),
            lf.clone(),
            Color32::from_gray(200),
        );
        painter.text(
            pos2(bar.max.x + 8.0, bar.center().y),
            Align2::LEFT_CENTER,
            fmt_size(cache.file_max),
            lf,
            Color32::from_gray(200),
        );
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
    fn color_scale_runs_blue_to_red() {
        let lo = scale_color(0.0);
        let hi = scale_color(1.0);
        assert!(lo.b() > lo.r() && lo.b() > lo.g(), "smallest is blue: {lo:?}");
        assert!(hi.r() > hi.g() && hi.r() > hi.b(), "largest is red: {hi:?}");
        let mid = scale_color(0.5);
        assert!(mid.g() > mid.r() && mid.g() > mid.b(), "middle is green: {mid:?}");

        let mut c = TreemapCache::default();
        c.file_min = 1024;
        c.file_max = 1024 * 1024 * 1024;
        assert_eq!(c.size_t(1024), 0.0);
        assert_eq!(c.size_t(1024 * 1024 * 1024), 1.0);
        let t = c.size_t(1024 * 1024);
        assert!((t - 0.5).abs() < 0.01, "log midpoint: {t}");
        assert_eq!(c.size_t(0), 0.0);
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
