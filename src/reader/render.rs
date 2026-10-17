//! Offscreen painter for the reading panel: rows, word highlight, footer.
//!
//! Renders into a plain `&mut [u32]` pixel buffer in softbuffer's format
//! (`0x00RRGGBB`), with text shaped by cosmic-text. All layout sizes are
//! logical pixels multiplied by the `scale` passed to [`Painter::resize`].
//!
//! Efficiency (owner rules: nothing costs until the panel is shown, shape text
//! only when it changes, draw only visible rows):
//! - each chunk owns one [`Buffer`], shaped once in [`Painter::set_playlist`]
//!   and again on [`Painter::resize`] only when the shaped width changed;
//! - [`Painter::set_highlight`] re-styles and re-shapes only the old and new
//!   current rows, and is a no-op when the `(current, word)` pair is unchanged;
//! - [`Painter::paint`] never shapes a row: it fills the background, draws
//!   only rows intersecting the visible list area, and clips every pixel. The
//!   one-line footer label is the only text shaped from `paint`, and only when
//!   its text changed (there is no earlier hook: it arrives with the frame).
//! - row base colors (`secondary` before the current row, `text` otherwise)
//!   are a paint-time [`Buffer::draw`] parameter, so moving the current row
//!   never re-shapes the rows in between.

use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap};

use crate::word_timing::word_ranges;

/// Logical layout sizes (multiplied by the scale in [`Painter::resize`]).
const PAD: f32 = 12.0;
const INNER: f32 = 8.0;
const GAP: f32 = 6.0;
const FOOTER_H: f32 = 100.0;
const ROW_RADIUS: f32 = 8.0;
const FONT_SIZE: f32 = 15.0;
const LINE_HEIGHT: f32 = 21.0;
const LABEL_SIZE: f32 = 13.0;
const LABEL_LINE: f32 = 18.0;

/// Panel colors (`0x00RRGGBB`, matching the pixel buffer format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub bg: u32,
    pub text: u32,
    pub secondary: u32,
    pub accent: u32,
    pub current_row: u32,
    pub track: u32,
}

impl Theme {
    #[must_use]
    pub fn light() -> Self {
        Self {
            bg: 0xF5F5F7,
            text: 0x1D1D1F,
            secondary: 0x6E6E73,
            accent: 0x0A84FF,
            current_row: 0xDCE9FB,
            track: 0xD9D9DE,
        }
    }

    #[must_use]
    pub fn dark() -> Self {
        Self {
            bg: 0x1E1E1E,
            text: 0xF2F2F7,
            secondary: 0x98989D,
            accent: 0x0A84FF,
            current_row: 0x1F3348,
            track: 0x3A3A3C,
        }
    }
}

/// What a point in the panel hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Row(usize),
    Previous,
    PlayPause,
    Next,
    None,
}

/// Per-frame inputs to [`Painter::paint`].
#[derive(Debug)]
pub struct Frame<'a> {
    pub label: &'a str,
    pub progress: f32,
    pub can_previous: bool,
    pub can_next: bool,
    pub playing: bool,
    /// Whether the Play/Pause button is enabled (a reading is active).
    pub can_play_pause: bool,
    pub theme: &'a Theme,
}

/// Integer rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.w)
            && y < self.y.saturating_add(self.h)
    }
}

fn rgb_color(value: u32) -> Color {
    Color::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Source-over blend of one channel; 50% alpha gives the midpoint.
fn blend_channel(src: u32, dst: u32, alpha: u32) -> u32 {
    (src * alpha + dst * (255 - alpha) + 127) / 255
}

fn blend_pixel(dst: u32, src: u32, alpha: u8) -> u32 {
    let a = u32::from(alpha);
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return src;
    }
    let r = blend_channel((src >> 16) & 0xFF, (dst >> 16) & 0xFF, a);
    let g = blend_channel((src >> 8) & 0xFF, (dst >> 8) & 0xFF, a);
    let b = blend_channel(src & 0xFF, dst & 0xFF, a);
    (r << 16) | (g << 8) | b
}

/// Clip `rect` to `[0, bound_w) × [0, bound_h)`; `None` when nothing shows.
fn clip(rect: Rect, bound_w: i32, bound_h: i32) -> Option<(i32, i32, i32, i32)> {
    if rect.w <= 0 || rect.h <= 0 || bound_w <= 0 || bound_h <= 0 {
        return None;
    }
    let x0 = rect.x.max(0);
    let y0 = rect.y.max(0);
    // `i64` sums so huge origins can't overflow.
    let x1 = (rect.x as i64 + rect.w as i64).min(bound_w as i64) as i32;
    let y1 = (rect.y as i64 + rect.h as i64).min(bound_h as i64) as i32;
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

/// Fill `rect` with a solid color, clipped. Callers guarantee
/// `pixels.len() >= bound_w * bound_h`, so the clipped indices are in bounds.
fn fill_rect(pixels: &mut [u32], bound_w: i32, bound_h: i32, rect: Rect, color: u32) {
    let Some((x0, y0, x1, y1)) = clip(rect, bound_w, bound_h) else {
        return;
    };
    for y in y0..y1 {
        let base = y as usize * bound_w as usize;
        for x in x0..x1 {
            pixels[base + x as usize] = color;
        }
    }
}

/// Stamp one glyph pixel: bounds-check once, then blend in place.
/// Callers guarantee `pixels.len() >= bound_w * bound_h`, as for `fill_rect`.
fn stamp_pixel(pixels: &mut [u32], bound_w: i32, bound_h: i32, x: i32, y: i32, color: Color) {
    let alpha = color.a();
    if alpha == 0 {
        return;
    }
    if x < 0 || y < 0 || x >= bound_w || y >= bound_h {
        return;
    }
    let src = (u32::from(color.r()) << 16) | (u32::from(color.g()) << 8) | u32::from(color.b());
    let index = y as usize * bound_w as usize + x as usize;
    if alpha == 255 {
        pixels[index] = src;
    } else {
        pixels[index] = blend_pixel(pixels[index], src, alpha);
    }
}

/// Stamp a cosmic-text glyph rect (per-pixel coverage in the color's alpha),
/// clipped. Transparent pixels are skipped.
fn stamp_rect(pixels: &mut [u32], bound_w: i32, bound_h: i32, rect: Rect, color: Color) {
    let alpha = color.a();
    if alpha == 0 {
        return;
    }
    let src = (u32::from(color.r()) << 16) | (u32::from(color.g()) << 8) | u32::from(color.b());
    if alpha == 255 {
        fill_rect(pixels, bound_w, bound_h, rect, src);
        return;
    }
    let Some((x0, y0, x1, y1)) = clip(rect, bound_w, bound_h) else {
        return;
    };
    for y in y0..y1 {
        let base = y as usize * bound_w as usize;
        for x in x0..x1 {
            let index = base + x as usize;
            pixels[index] = blend_pixel(pixels[index], src, alpha);
        }
    }
}

/// Pixels trimmed from each side of a row `d` rows from the nearest edge of a
/// rounded rect of radius `r` (quarter-circle inset).
fn corner_inset(d: i32, r: i32) -> i32 {
    if d >= r || r <= 0 {
        return 0;
    }
    let t = f64::from(r - d);
    let rr = f64::from(r);
    r - (rr * rr - t * t).sqrt().floor() as i32
}

fn fill_rounded_rect(
    pixels: &mut [u32],
    bound_w: i32,
    bound_h: i32,
    rect: Rect,
    radius: i32,
    color: u32,
) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let r = radius.max(0).min(rect.w / 2).min(rect.h / 2);
    if r == 0 {
        fill_rect(pixels, bound_w, bound_h, rect, color);
        return;
    }
    let Some((x0, y0, x1, y1)) = clip(rect, bound_w, bound_h) else {
        return;
    };
    for y in y0..y1 {
        let d = (y - rect.y).min(rect.y + rect.h - 1 - y);
        let inset = corner_inset(d, r);
        let xs = (rect.x + inset).max(x0);
        let xe = (rect.x + rect.w - inset).min(x1);
        if xs >= xe {
            continue;
        }
        let base = y as usize * bound_w as usize;
        for x in xs..xe {
            pixels[base + x as usize] = color;
        }
    }
}

/// X of the edge `(x0, y0) → (x1, y1)` at row `y`; `i64` math can't overflow.
fn edge_x(x0: i32, y0: i32, x1: i32, y1: i32, y: i32) -> i32 {
    if y1 == y0 {
        return x0;
    }
    (x0 as i64 + (x1 as i64 - x0 as i64) * (y as i64 - y0 as i64) / (y1 as i64 - y0 as i64))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn fill_triangle(
    pixels: &mut [u32],
    bound_w: i32,
    bound_h: i32,
    mut verts: [(i32, i32); 3],
    color: u32,
) {
    if bound_w <= 0 || bound_h <= 0 {
        return;
    }
    verts.sort_by_key(|v| v.1);
    let [(ax, ay), (bx, by), (cx, cy)] = verts;
    if ay == cy {
        return;
    }
    let y0 = ay.max(0);
    let y1 = cy.min(bound_h);
    for y in y0..y1 {
        let xa = edge_x(ax, ay, cx, cy, y);
        let xb = if y < by {
            edge_x(ax, ay, bx, by, y)
        } else {
            edge_x(bx, by, cx, cy, y)
        };
        let xs = xa.min(xb).max(0);
        let xe = xa.max(xb).min(bound_w - 1);
        if xs > xe {
            continue;
        }
        let base = y as usize * bound_w as usize;
        for x in xs..=xe {
            pixels[base + x as usize] = color;
        }
    }
}

/// One chunk's shaped text plus the source text for re-styling.
struct Row {
    buffer: Buffer,
    text: String,
}

/// Rich spans for the current row: every word plain except `highlight`, which
/// is bold in `accent`. Whitespace between words keeps default attributes.
fn rich_spans(text: &str, highlight: usize, accent: Color) -> Vec<(&str, Attrs<'_>)> {
    let ranges = word_ranges(text);
    let mut spans = Vec::with_capacity(ranges.len() * 2 + 1);
    let mut pos = 0;
    for (index, range) in ranges.iter().enumerate() {
        if range.start > pos {
            spans.push((&text[pos..range.start], Attrs::new()));
        }
        let attrs = if index == highlight {
            Attrs::new().color(accent).weight(Weight::BOLD)
        } else {
            Attrs::new()
        };
        spans.push((&text[range.clone()], attrs));
        pos = range.end;
    }
    if pos < text.len() {
        spans.push((&text[pos..], Attrs::new()));
    }
    if spans.is_empty() {
        spans.push((text, Attrs::new()));
    }
    spans
}

/// Offscreen reading-panel painter; see the module docs.
pub struct Painter {
    font_system: FontSystem,
    cache: SwashCache,
    rows: Vec<Row>,
    /// Cached content-space (physical px) row box tops and heights.
    row_tops: Vec<i32>,
    row_heights: Vec<i32>,
    /// Single-line footer label, shaped only when its text changes.
    label: Buffer,
    label_text: String,
    highlight: (Option<usize>, Option<usize>),
    /// Content offset in physical pixels, clamped to the content.
    scroll: f32,
    content_h: i32,
    width: u32,
    height: u32,
    scale: f32,
    /// `(width_px, scale bits)` the rows are shaped for.
    shaped_for: (u32, u32),
    /// Button state from the last painted frame (for [`Painter::hit`]).
    can_previous: bool,
    can_next: bool,
    can_play_pause: bool,
    accent: u32,
    #[cfg(test)]
    reshapes: usize,
}

impl Painter {
    /// Load system fonts. Round 3 shows the error as
    /// "Couldn't open the reading panel: no system fonts were found."
    pub fn new() -> Result<Self, String> {
        let mut font_system = FontSystem::new();
        // 0.19 has no `db()` getter; `db_mut` on our own fresh system is the
        // same check.
        if font_system.db_mut().is_empty() {
            return Err("no system fonts were found".to_string());
        }
        let label = Buffer::new(&mut font_system, Metrics::new(LABEL_SIZE, LABEL_LINE));
        Ok(Self {
            font_system,
            cache: SwashCache::new(),
            rows: Vec::new(),
            row_tops: Vec::new(),
            row_heights: Vec::new(),
            label,
            label_text: String::new(),
            highlight: (None, None),
            scroll: 0.0,
            content_h: 0,
            width: 0,
            height: 0,
            scale: 1.0,
            shaped_for: (0, 1.0f32.to_bits()),
            can_previous: false,
            can_next: false,
            can_play_pause: false,
            accent: Theme::light().accent,
            #[cfg(test)]
            reshapes: 0,
        })
    }

    /// Replace the playlist: one buffer per chunk, all shaped once.
    pub fn set_playlist(&mut self, chunks: &[String]) {
        let metrics = self.row_metrics();
        let wrap = self.wrap_width_px();
        self.rows.clear();
        self.rows.reserve(chunks.len());
        for chunk in chunks {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            // Word wrap, falling back to glyph wrap so long tokens can't run
            // past the padding.
            buffer.set_wrap(Wrap::WordOrGlyph);
            buffer.set_size(Some(wrap), None);
            // No forced alignment: with `Advanced` shaping, RTL chunks come
            // out right-aligned and shaped.
            buffer.set_text(chunk, &Attrs::new(), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.rows.push(Row {
                buffer,
                text: chunk.clone(),
            });
        }
        self.highlight = (None, None);
        self.scroll = 0.0;
        self.shaped_for = (self.width, self.scale.to_bits());
        self.relayout();
    }

    pub fn resize(&mut self, width_px: u32, height_px: u32, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.width = width_px;
        self.height = height_px;
        self.scale = scale;
        if (width_px, scale.to_bits()) != self.shaped_for {
            self.shaped_for = (width_px, scale.to_bits());
            let metrics = self.row_metrics();
            let wrap = self.wrap_width_px();
            for row in &mut self.rows {
                row.buffer.set_metrics(metrics);
                row.buffer.set_size(Some(wrap), None);
                row.buffer.shape_until_scroll(&mut self.font_system, false);
            }
            let label_metrics = self.label_metrics();
            self.label.set_metrics(label_metrics);
            self.label
                .set_size(Some(wrap), Some(label_metrics.line_height));
            self.label.shape_until_scroll(&mut self.font_system, false);
            self.relayout();
        }
        self.clamp_scroll();
    }

    /// Move the highlight. Re-styles and re-shapes only the old and new
    /// current rows; a no-op when the pair is unchanged.
    pub fn set_highlight(&mut self, current: Option<usize>, word: Option<usize>) {
        if (current, word) == self.highlight {
            return;
        }
        let old = self.highlight.0;
        self.highlight = (current, word);
        let accent = rgb_color(self.accent);
        match (old, current) {
            (Some(o), Some(n)) if o == n => self.restyle_row(n, word, accent),
            _ => {
                if let Some(o) = old {
                    self.restyle_row(o, None, accent);
                }
                if let Some(n) = current {
                    self.restyle_row(n, word, accent);
                }
            }
        }
        // Bold words are wider: re-measure in case wrapping changed. This only
        // reads the existing layout, it doesn't shape.
        self.relayout();
        self.clamp_scroll();
    }

    /// Scroll by physical pixels, clamped to the content.
    pub fn scroll_by(&mut self, dy_px: f32) {
        if !dy_px.is_finite() {
            return;
        }
        self.scroll = (self.scroll + dy_px).clamp(0.0, self.max_scroll());
    }

    /// Center the current row in the visible list area, clamped.
    pub fn scroll_to_current(&mut self) {
        let Some(index) = self.highlight.0 else {
            return;
        };
        let (Some(&top), Some(&box_h)) = (self.row_tops.get(index), self.row_heights.get(index))
        else {
            return;
        };
        let center = top as f32 + box_h as f32 / 2.0;
        self.scroll = (center - self.list_height_px() as f32 / 2.0).clamp(0.0, self.max_scroll());
    }

    /// Paint a frame into `pixels` (row-major, `width × height`). Never shapes
    /// a row; draws only visible rows; clips every pixel. Silently skips when
    /// the buffer is smaller than the surface (round 3 owns the buffer).
    pub fn paint(&mut self, frame: &Frame, pixels: &mut [u32]) {
        let (w, h) = self.size_px();
        if w <= 0 || h <= 0 {
            return;
        }
        if (w as u64) * (h as u64) > pixels.len() as u64 {
            return;
        }
        self.can_previous = frame.can_previous;
        self.can_next = frame.can_next;
        self.can_play_pause = frame.can_play_pause;
        self.accent = frame.theme.accent;
        fill_rect(pixels, w, h, Rect { x: 0, y: 0, w, h }, frame.theme.bg);
        self.paint_rows(frame, pixels);
        self.paint_footer(frame, pixels);
    }

    /// Hit-test a physical point. Button enabled state comes from the last
    /// painted frame, so paint before hit.
    pub fn hit(&self, x_px: f32, y_px: f32) -> Hit {
        if !x_px.is_finite() || !y_px.is_finite() {
            return Hit::None;
        }
        let (w, h) = self.size_px();
        // Saturating casts: huge values land outside and return `None`.
        let x = x_px.floor() as i32;
        let y = y_px.floor() as i32;
        if x < 0 || y < 0 || x >= w || y >= h {
            return Hit::None;
        }
        if y < self.list_height_px() {
            let content_y = (y as f32 + self.scroll).floor() as i32;
            for (index, (&top, &box_h)) in self
                .row_tops
                .iter()
                .zip(self.row_heights.iter())
                .enumerate()
            {
                if content_y >= top && content_y < top.saturating_add(box_h) {
                    return Hit::Row(index);
                }
            }
            return Hit::None;
        }
        let (prev, play, next) = self.button_rects();
        if prev.contains(x, y) {
            return if self.can_previous {
                Hit::Previous
            } else {
                Hit::None
            };
        }
        if play.contains(x, y) {
            return if self.can_play_pause {
                Hit::PlayPause
            } else {
                Hit::None
            };
        }
        if next.contains(x, y) {
            return if self.can_next { Hit::Next } else { Hit::None };
        }
        Hit::None
    }

    /// Current content offset in physical pixels (for redraw comparison).
    #[must_use]
    pub fn scroll_offset(&self) -> f32 {
        self.scroll
    }

    #[cfg(test)]
    pub fn reshape_count(&self) -> usize {
        self.reshapes
    }

    /// Re-style one row (rich when it carries the word highlight, plain
    /// otherwise) and re-shape it eagerly so `paint` never shapes.
    fn restyle_row(&mut self, index: usize, word: Option<usize>, accent: Color) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        match word {
            Some(highlight) => {
                let spans = rich_spans(&row.text, highlight, accent);
                row.buffer
                    .set_rich_text(spans, &Attrs::new(), Shaping::Advanced, None);
            }
            None => {
                row.buffer
                    .set_text(&row.text, &Attrs::new(), Shaping::Advanced, None);
            }
        }
        row.buffer.shape_until_scroll(&mut self.font_system, false);
        #[cfg(test)]
        {
            self.reshapes += 1;
        }
    }

    /// Re-measure cached row boxes from the existing layout (no shaping).
    fn relayout(&mut self) {
        let pad = self.px(PAD);
        let inner = self.px(INNER);
        let gap = self.px(GAP);
        let min_h = self.row_metrics().line_height;
        self.row_tops.clear();
        self.row_heights.clear();
        self.row_tops.reserve(self.rows.len());
        self.row_heights.reserve(self.rows.len());
        let mut top = pad;
        for row in &self.rows {
            let mut text_h = min_h;
            for run in row.buffer.layout_runs() {
                text_h = text_h.max(run.line_top + run.line_height);
            }
            let box_h = text_h.ceil() as i32 + 2 * inner;
            self.row_tops.push(top);
            self.row_heights.push(box_h);
            top += box_h + gap;
        }
        self.content_h = if self.rows.is_empty() {
            0
        } else {
            top - gap + pad
        };
    }

    fn paint_rows(&mut self, frame: &Frame, pixels: &mut [u32]) {
        let (w, _) = self.size_px();
        let list_h = self.list_height_px();
        if list_h <= 0 || self.rows.is_empty() {
            return;
        }
        let pad = self.px(PAD);
        let inner = self.px(INNER);
        let radius = self.px(ROW_RADIUS);
        let current = self.highlight.0;
        let ox = pad + inner;
        for i in 0..self.rows.len() {
            let top = self.row_tops[i];
            let box_h = self.row_heights[i];
            let screen_top = (top as f32 - self.scroll).round() as i32;
            if screen_top + box_h <= 0 || screen_top >= list_h {
                continue;
            }
            if Some(i) == current {
                fill_rounded_rect(
                    pixels,
                    w,
                    list_h,
                    Rect {
                        x: pad,
                        y: screen_top,
                        w: w - 2 * pad,
                        h: box_h,
                    },
                    radius,
                    frame.theme.current_row,
                );
            }
            let base = if current.is_some_and(|c| i < c) {
                frame.theme.secondary
            } else {
                frame.theme.text
            };
            let oy = screen_top + inner;
            let row = &mut self.rows[i];
            row.buffer.draw(
                &mut self.font_system,
                &mut self.cache,
                rgb_color(base),
                |gx, gy, gw, gh, color| {
                    let x = ox.saturating_add(gx);
                    let y = oy.saturating_add(gy);
                    if gw == 1 && gh == 1 {
                        stamp_pixel(pixels, w, list_h, x, y, color);
                    } else {
                        stamp_rect(
                            pixels,
                            w,
                            list_h,
                            Rect {
                                x,
                                y,
                                w: gw.min(i32::MAX as u32) as i32,
                                h: gh.min(i32::MAX as u32) as i32,
                            },
                            color,
                        );
                    }
                },
            );
        }
    }

    fn paint_footer(&mut self, frame: &Frame, pixels: &mut [u32]) {
        let (w, h) = self.size_px();
        let pad = self.px(PAD);
        let footer_top = h - self.px(FOOTER_H);
        fill_rect(
            pixels,
            w,
            h,
            Rect {
                x: 0,
                y: footer_top,
                w,
                h: self.px(1.0).max(1),
            },
            frame.theme.track,
        );
        // The label is the only text shaped from `paint`, and only when it
        // changed since the last frame.
        if self.label_text.as_str() != frame.label {
            self.label_text = frame.label.to_string();
            self.label
                .set_text(frame.label, &Attrs::new(), Shaping::Advanced, None);
            self.label.shape_until_scroll(&mut self.font_system, false);
        }
        let label_top = footer_top + self.px(12.0);
        let label = &mut self.label;
        label.draw(
            &mut self.font_system,
            &mut self.cache,
            rgb_color(frame.theme.secondary),
            |gx, gy, gw, gh, color| {
                let x = pad.saturating_add(gx);
                let y = label_top.saturating_add(gy);
                if gw == 1 && gh == 1 {
                    stamp_pixel(pixels, w, h, x, y, color);
                } else {
                    stamp_rect(
                        pixels,
                        w,
                        h,
                        Rect {
                            x,
                            y,
                            w: gw.min(i32::MAX as u32) as i32,
                            h: gh.min(i32::MAX as u32) as i32,
                        },
                        color,
                    );
                }
            },
        );
        let bar_top = label_top + self.px(LABEL_LINE) + self.px(6.0);
        let bar_h = self.px(4.0).max(1);
        let bar_w = w - 2 * pad;
        if bar_w > 0 {
            fill_rounded_rect(
                pixels,
                w,
                h,
                Rect {
                    x: pad,
                    y: bar_top,
                    w: bar_w,
                    h: bar_h,
                },
                bar_h / 2,
                frame.theme.track,
            );
            let p = if frame.progress.is_finite() {
                frame.progress.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let fill_w = (p * bar_w as f32).round() as i32;
            if fill_w > 0 {
                fill_rounded_rect(
                    pixels,
                    w,
                    h,
                    Rect {
                        x: pad,
                        y: bar_top,
                        w: fill_w.min(bar_w),
                        h: bar_h,
                    },
                    bar_h / 2,
                    frame.theme.accent,
                );
            }
        }
        let (prev, play, next) = self.button_rects();
        let text = frame.theme.text;
        let track = frame.theme.track;
        self.paint_prev_icon(
            pixels,
            w,
            h,
            prev,
            if frame.can_previous { text } else { track },
        );
        self.paint_play_icon(
            pixels,
            w,
            h,
            play,
            if frame.can_play_pause { text } else { track },
            frame.playing,
        );
        self.paint_next_icon(
            pixels,
            w,
            h,
            next,
            if frame.can_next { text } else { track },
        );
    }

    fn paint_prev_icon(&self, pixels: &mut [u32], bound_w: i32, bound_h: i32, r: Rect, color: u32) {
        let bar_w = self.px(4.0).max(1);
        let tri_w = self.px(14.0).max(1);
        let tri_h = self.px(20.0).max(1);
        let gap = self.px(2.0).max(0);
        let sx = r.x + (r.w - (bar_w + gap + tri_w)) / 2;
        let cy = r.y + r.h / 2;
        let ty = cy - tri_h / 2;
        fill_rect(
            pixels,
            bound_w,
            bound_h,
            Rect {
                x: sx,
                y: ty,
                w: bar_w,
                h: tri_h,
            },
            color,
        );
        let tip = sx + bar_w + gap;
        fill_triangle(
            pixels,
            bound_w,
            bound_h,
            [(tip, cy), (tip + tri_w, ty), (tip + tri_w, ty + tri_h)],
            color,
        );
    }

    fn paint_next_icon(&self, pixels: &mut [u32], bound_w: i32, bound_h: i32, r: Rect, color: u32) {
        let bar_w = self.px(4.0).max(1);
        let tri_w = self.px(14.0).max(1);
        let tri_h = self.px(20.0).max(1);
        let gap = self.px(2.0).max(0);
        let sx = r.x + (r.w - (bar_w + gap + tri_w)) / 2;
        let cy = r.y + r.h / 2;
        let ty = cy - tri_h / 2;
        fill_triangle(
            pixels,
            bound_w,
            bound_h,
            [(sx + tri_w, cy), (sx, ty), (sx, ty + tri_h)],
            color,
        );
        fill_rect(
            pixels,
            bound_w,
            bound_h,
            Rect {
                x: sx + tri_w + gap,
                y: ty,
                w: bar_w,
                h: tri_h,
            },
            color,
        );
    }

    fn paint_play_icon(
        &self,
        pixels: &mut [u32],
        bound_w: i32,
        bound_h: i32,
        r: Rect,
        color: u32,
        playing: bool,
    ) {
        let cy = r.y + r.h / 2;
        if playing {
            let bw = self.px(5.0).max(1);
            let ph = self.px(22.0).max(1);
            let gap = self.px(6.0).max(0);
            let sx = r.x + (r.w - (2 * bw + gap)) / 2;
            let ty = cy - ph / 2;
            fill_rect(
                pixels,
                bound_w,
                bound_h,
                Rect {
                    x: sx,
                    y: ty,
                    w: bw,
                    h: ph,
                },
                color,
            );
            fill_rect(
                pixels,
                bound_w,
                bound_h,
                Rect {
                    x: sx + bw + gap,
                    y: ty,
                    w: bw,
                    h: ph,
                },
                color,
            );
        } else {
            let tw = self.px(13.0).max(1);
            let th = self.px(22.0).max(1);
            let sx = r.x + (r.w - tw) / 2;
            let ty = cy - th / 2;
            fill_triangle(
                pixels,
                bound_w,
                bound_h,
                [(sx + tw, cy), (sx, ty), (sx, ty + th)],
                color,
            );
        }
    }

    /// Previous / Play-Pause / Next boxes: a 40 px row centered 12 above the
    /// bottom edge; 32 / 40 / 32 wide boxes, 16 apart. Shared by paint and hit.
    fn button_rects(&self) -> (Rect, Rect, Rect) {
        let (w, h) = self.size_px();
        let bh = self.px(40.0);
        let y = h - self.px(12.0) - bh;
        let side = self.px(32.0);
        let mid = self.px(40.0);
        let gap = self.px(16.0);
        let x0 = (w - (side + gap + mid + gap + side)) / 2;
        (
            Rect {
                x: x0,
                y,
                w: side,
                h: bh,
            },
            Rect {
                x: x0 + side + gap,
                y,
                w: mid,
                h: bh,
            },
            Rect {
                x: x0 + side + gap + mid + gap,
                y,
                w: side,
                h: bh,
            },
        )
    }

    fn row_metrics(&self) -> Metrics {
        Metrics::new(FONT_SIZE * self.scale, LINE_HEIGHT * self.scale)
    }

    fn label_metrics(&self) -> Metrics {
        Metrics::new(LABEL_SIZE * self.scale, LABEL_LINE * self.scale)
    }

    fn size_px(&self) -> (i32, i32) {
        (
            self.width.min(i32::MAX as u32) as i32,
            self.height.min(i32::MAX as u32) as i32,
        )
    }

    fn px(&self, logical: f32) -> i32 {
        (logical * self.scale).round() as i32
    }

    fn wrap_width_px(&self) -> f32 {
        (self.width as f32 - 2.0 * (PAD + INNER) * self.scale).max(1.0)
    }

    fn list_height_px(&self) -> i32 {
        (self.size_px().1 - self.px(FOOTER_H)).max(0)
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - self.list_height_px()).max(0) as f32
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_chunks() -> Vec<String> {
        [
            "First row.",
            "Second row is a little longer than the first.",
            "سلام! این یک متن فارسی است.",
        ]
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    /// `None` (with a note) when the machine has no fonts, e.g. thin CI images.
    fn painter() -> Option<Painter> {
        match Painter::new() {
            Ok(painter) => Some(painter),
            Err(err) => {
                eprintln!("skipping painter test: {err}");
                None
            }
        }
    }

    fn test_frame<'a>(theme: &'a Theme) -> Frame<'a> {
        Frame {
            label: "Sentence 1 of 3",
            progress: 1.0 / 3.0,
            can_previous: false,
            can_next: true,
            playing: true,
            can_play_pause: true,
            theme,
        }
    }

    #[test]
    fn blend_midpoint_at_half_alpha() {
        assert_eq!(blend_pixel(0x000000, 0xFFFFFF, 128), 0x808080);
        assert_eq!(blend_pixel(0x123456, 0xABCDEF, 0), 0x123456);
        assert_eq!(blend_pixel(0x123456, 0xABCDEF, 255), 0xABCDEF);
        assert_eq!(blend_pixel(0x112233, 0x112233, 200), 0x112233);
    }

    #[test]
    fn stamp_pixel_matches_stamp_rect() {
        // Inside, on every edge, and just outside on a 10x6 surface.
        let positions = [
            (0, 0),
            (5, 3),
            (9, 5),
            (0, 5),
            (9, 0),
            (-1, 0),
            (0, -1),
            (10, 0),
            (0, 6),
            (10, 6),
            (-5, -5),
            (20, 20),
        ];
        for alpha in [0u8, 128, 255] {
            for (x, y) in positions {
                let mut fast = vec![0x112233u32; 10 * 6];
                let mut slow = fast.clone();
                let color = Color::rgba(0xAA, 0xBB, 0xCC, alpha);
                stamp_pixel(&mut fast, 10, 6, x, y, color);
                stamp_rect(&mut slow, 10, 6, Rect { x, y, w: 1, h: 1 }, color);
                assert_eq!(fast, slow, "mismatch at ({x}, {y}) alpha {alpha}");
            }
        }
        // Zero-size surface: no panic, no write.
        let mut empty: Vec<u32> = Vec::new();
        stamp_pixel(&mut empty, 0, 0, 0, 0, Color::rgb(1, 2, 3));
        assert!(empty.is_empty());
    }

    #[test]
    fn pixel_helpers_clip_at_every_edge() {
        // 10x6 surface.
        let mut pixels = vec![0u32; 10 * 6];
        // Rect hanging off the top-left: only (0..5, 0..3) lands.
        fill_rect(
            &mut pixels,
            10,
            6,
            Rect {
                x: -3,
                y: -2,
                w: 8,
                h: 5,
            },
            0x111111,
        );
        assert_eq!(pixels[0], 0x111111);
        assert_eq!(pixels[2 * 10 + 4], 0x111111);
        assert_eq!(pixels[2 * 10 + 5], 0);
        assert_eq!(pixels[3 * 10], 0);
        // Rect hanging off the bottom-right.
        fill_rect(
            &mut pixels,
            10,
            6,
            Rect {
                x: 8,
                y: 4,
                w: 6,
                h: 6,
            },
            0x222222,
        );
        assert_eq!(pixels[4 * 10 + 8], 0x222222);
        assert_eq!(pixels[5 * 10 + 9], 0x222222);
        assert_eq!(pixels[4 * 10 + 7], 0);
        // Fully outside: no write, no panic.
        fill_rect(
            &mut pixels,
            10,
            6,
            Rect {
                x: 20,
                y: 20,
                w: 4,
                h: 4,
            },
            0x333333,
        );
        fill_rect(
            &mut pixels,
            10,
            6,
            Rect {
                x: -8,
                y: 1,
                w: 4,
                h: 2,
            },
            0x333333,
        );
        assert!(!pixels.contains(&0x333333));
        // Zero and negative sizes: no-ops.
        for rect in [
            Rect {
                x: 1,
                y: 1,
                w: 0,
                h: 3,
            },
            Rect {
                x: 1,
                y: 1,
                w: 3,
                h: 0,
            },
            Rect {
                x: 1,
                y: 1,
                w: -4,
                h: 3,
            },
        ] {
            fill_rect(&mut pixels, 10, 6, rect, 0x444444);
            fill_rounded_rect(&mut pixels, 10, 6, rect, 3, 0x444444);
        }
        assert!(!pixels.contains(&0x444444));
        // Zero-size surface: no panic.
        let mut empty: Vec<u32> = Vec::new();
        fill_rect(
            &mut empty,
            0,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            1,
        );
        fill_rounded_rect(
            &mut empty,
            0,
            0,
            Rect {
                x: -2,
                y: -2,
                w: 8,
                h: 8,
            },
            3,
            1,
        );
        fill_triangle(&mut empty, 0, 0, [(0, 0), (5, 0), (0, 5)], 1);
        stamp_rect(
            &mut empty,
            0,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            Color::rgb(1, 2, 3),
        );
    }

    #[test]
    fn rounded_rect_and_triangle_clip_and_degenerate() {
        let mut pixels = vec![0u32; 12 * 12];
        // Rounded rect half off the left edge, radius larger than half height.
        fill_rounded_rect(
            &mut pixels,
            12,
            12,
            Rect {
                x: -4,
                y: 2,
                w: 10,
                h: 4,
            },
            8,
            0x555555,
        );
        assert_eq!(pixels[3 * 12], 0x555555);
        assert_eq!(pixels[2 * 12 + 3], 0x555555);
        // Edge row is corner-trimmed: x=5 stays empty.
        assert_eq!(pixels[2 * 12 + 5], 0);
        // Corner is actually rounded: the rect's own corner pixel stays empty.
        let mut corner = vec![0u32; 20 * 20];
        fill_rounded_rect(
            &mut corner,
            20,
            20,
            Rect {
                x: 2,
                y: 2,
                w: 16,
                h: 16,
            },
            8,
            7,
        );
        assert_eq!(corner[2 * 20 + 2], 0);
        assert_eq!(corner[10 * 20 + 10], 7);
        // Triangle hanging off every edge, plus degenerate ones.
        fill_triangle(&mut pixels, 12, 12, [(-5, -5), (20, 6), (6, 20)], 9);
        fill_triangle(&mut pixels, 12, 12, [(3, 3), (3, 3), (3, 3)], 9);
        fill_triangle(&mut pixels, 12, 12, [(1, 5), (9, 5), (4, 5)], 9);
        fill_triangle(&mut pixels, 12, 12, [(100, 100), (120, 100), (100, 120)], 9);
        assert!(pixels.contains(&9));
        // Transparent glyph stamp leaves the surface alone.
        let before = pixels.clone();
        stamp_rect(
            &mut pixels,
            12,
            12,
            Rect {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
            },
            Color::rgba(255, 255, 255, 0),
        );
        assert_eq!(pixels, before);
    }

    #[test]
    fn hit_buttons_rows_and_gaps() {
        let Some(mut painter) = painter() else {
            return;
        };
        // Short viewport so the rows scroll.
        painter.resize(440, 160, 1.0);
        painter.set_playlist(&test_chunks());
        let theme = Theme::light();
        let mut pixels = vec![0u32; 440 * 160];
        painter.paint(&test_frame(&theme), &mut pixels);

        let (prev, play, next) = painter.button_rects();
        let center = |r: Rect| (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        // Previous is disabled in this frame.
        let (x, y) = center(prev);
        assert_eq!(painter.hit(x, y), Hit::None);
        let (x, y) = center(play);
        assert_eq!(painter.hit(x, y), Hit::PlayPause);
        let (x, y) = center(next);
        assert_eq!(painter.hit(x, y), Hit::Next);
        // Enabled frame: previous hits.
        let enabled = Frame {
            can_previous: true,
            ..test_frame(&theme)
        };
        painter.paint(&enabled, &mut pixels);
        let (x, y) = center(prev);
        assert_eq!(painter.hit(x, y), Hit::Previous);

        // Rows: middle of row 0's box.
        let top0 = painter.row_tops[0];
        let h0 = painter.row_heights[0];
        assert_eq!(
            painter.hit(220.0, top0 as f32 + h0 as f32 / 2.0),
            Hit::Row(0)
        );
        // After scrolling, the same screen point hits a later row.
        painter.scroll_by(h0 as f32);
        assert_eq!(
            painter.hit(220.0, top0 as f32 + h0 as f32 / 2.0),
            Hit::Row(1)
        );
        // Footer gaps: bottom-left corner and the label's right end.
        assert_eq!(painter.hit(5.0, 155.0), Hit::None);
        assert_eq!(painter.hit(430.0, 75.0), Hit::None);
        // Between-rows gap.
        let gap_y = top0 + h0 + 3;
        painter.scroll_by(-(h0 as f32));
        assert_eq!(painter.hit(220.0, gap_y as f32), Hit::None);
        // Outside and non-finite.
        assert_eq!(painter.hit(-1.0, 10.0), Hit::None);
        assert_eq!(painter.hit(10.0, 500.0), Hit::None);
        assert_eq!(painter.hit(f32::NAN, 10.0), Hit::None);
    }

    #[test]
    fn highlight_same_pair_is_noop() {
        let Some(mut painter) = painter() else {
            return;
        };
        painter.resize(440, 300, 1.0);
        painter.set_playlist(&test_chunks());
        painter.set_highlight(Some(0), Some(0));
        let base = painter.reshape_count();
        assert!(base > 0);
        painter.set_highlight(Some(0), Some(0));
        assert_eq!(painter.reshape_count(), base);
        // Same row, new word: one row re-shaped.
        painter.set_highlight(Some(0), Some(1));
        assert_eq!(painter.reshape_count(), base + 1);
        // New row: old and new re-shaped.
        painter.set_highlight(Some(1), Some(0));
        assert_eq!(painter.reshape_count(), base + 3);
    }

    #[test]
    fn paint_empty_playlist_does_not_panic() {
        let Some(mut painter) = painter() else {
            return;
        };
        painter.resize(440, 300, 1.0);
        painter.set_playlist(&[]);
        let theme = Theme::light();
        let mut pixels = vec![0u32; 440 * 300];
        painter.paint(&test_frame(&theme), &mut pixels);
        // List area is plain background.
        assert_eq!(pixels[5 * 440 + 5], theme.bg);
        assert_eq!(painter.hit(220.0, 50.0), Hit::None);
    }

    #[test]
    fn text_stays_out_of_right_padding() {
        let Some(mut painter) = painter() else {
            return;
        };
        // A long English row and a long Persian row, both wrapping onto
        // several lines so the wrap width really binds.
        let scale = 2.0;
        painter.resize(440 * scale as u32, 300 * scale as u32, scale);
        painter.set_playlist(&[
            "This is a deliberately long English row that must wrap onto several lines so the wrap width really binds and every line ends before the right padding begins."
                .to_string(),
            "سلام! این یک متن فارسی نسبتا طولانی برای آزمایش است که باید راست‌چین شود.".to_string(),
        ]);
        let theme = Theme::light();
        let frame = Frame {
            label: "Sentence 1 of 2",
            progress: 0.5,
            can_previous: false,
            can_next: true,
            playing: true,
            can_play_pause: true,
            theme: &theme,
        };
        let (w, h) = (painter.width as usize, painter.height as usize);
        let list_h = painter.list_height_px() as usize;
        let margin = ((PAD + INNER) * scale).round() as usize;
        let mut pixels = vec![0u32; w * h];
        // Both rows highlighted in turn, so the margin is checked against the
        // current-row background as well as the plain background.
        for current in [Some(0), Some(1)] {
            painter.set_highlight(current, Some(1));
            painter.paint(&frame, &mut pixels);
            for y in 0..list_h {
                for x in (w - margin)..w {
                    let pixel = pixels[y * w + x];
                    assert!(
                        pixel == theme.bg || pixel == theme.current_row,
                        "glyph ink in the right margin at ({x}, {y}): {pixel:#08X}"
                    );
                }
            }
        }
    }

    #[test]
    fn scroll_clamps_and_centers_current() {
        let Some(mut painter) = painter() else {
            return;
        };
        painter.resize(440, 160, 1.0);
        painter.set_playlist(&test_chunks());
        painter.scroll_by(1e9);
        assert_eq!(painter.scroll, painter.max_scroll());
        painter.scroll_by(-1e9);
        assert_eq!(painter.scroll, 0.0);
        painter.scroll_by(f32::NAN);
        assert_eq!(painter.scroll, 0.0);
        painter.set_highlight(Some(2), Some(0));
        painter.scroll_to_current();
        assert!(painter.scroll >= 0.0);
        assert!(painter.scroll <= painter.max_scroll());
    }
}
