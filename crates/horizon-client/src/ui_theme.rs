//! The design system's widgets drawn in the game (`crates/horizon-theme`:
//! palette, type scale, radii, control sizes, motion and the bundled fonts;
//! `docs/design-system.md`). Every menu is vanilla's (`ui`); what is left
//! here draws Horizon's own overlays that vanilla has no screen for: the
//! on-screen keyboard a controller types with (`osk`), plus the
//! `horizon.txt` preferences.
//!
//! Build a [`Ui`] over the frame's `HudBatch` with [`Ui::new`], lay out in dp
//! (the launcher's logical pixels) and give every interactive widget a stable
//! `id`. Clicks run the same layout with [`Ui::dry`] and read [`Ui::hit`]: one
//! function draws and hit-tests, so they never disagree.
use crate::hud::{HudAtlas, HudBatch, Rect};
use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _};
use std::{cell::RefCell, collections::HashMap, path::Path, time::Duration};

/// A Horizon-only preference from `horizon.txt` beside `options.txt`
/// (`key:value` lines), so vanilla's options file stays exactly as vanilla
/// writes it.
pub fn preference(dir: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("horizon.txt")).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_owned())
    })
}
/// Set (or with None remove) one preference, keeping the others.
pub(crate) fn set_preference(dir: &Path, key: &str, value: Option<&str>) {
    let text = std::fs::read_to_string(dir.join("horizon.txt")).unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.split_once(':').is_none_or(|(k, _)| k.trim() != key))
        .map(str::to_owned)
        .collect();
    if let Some(v) = value {
        lines.push(format!("{key}:{v}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let _ = std::fs::write(dir.join("horizon.txt"), out);
}

/// The dark palette (`horizon_theme::DARK`) in the HUD's colour encoding, plus the
/// translucent plates the overlays draw over the game.
pub(crate) mod color {
    use horizon_theme::DARK as P;
    pub const fn hex(c: u32, a: u8) -> u32 {
        (a as u32) << 24 | (c & 0xff) << 16 | (c >> 8 & 0xff) << 8 | (c >> 16 & 0xff)
    }
    /// The page, under every veil.
    pub const BG: u32 = hex(P.page, 255);
    /// Hover plates, secondary buttons, fields, the segmented track: the
    /// dark palette's soft plate, as ink over the veil so the plates read the
    /// same over any backdrop (over the page colour they are `plate_soft`).
    pub const RAISED: u32 = hex(P.ink, 20);
    /// Pressed plates and a switch that is off (`plate`, the same way).
    pub const HOVER: u32 = hex(P.ink, 36);
    /// One-pixel rules between rows, slider tracks.
    pub const BORDER: u32 = hex(P.rule, 255);
    /// Text and selection (`ink`).
    pub const TEXT: u32 = hex(P.ink, 255);
    /// Values and secondary text (`ink_soft`).
    pub const MUTED: u32 = hex(P.ink_soft, 255);
    pub const ACCENT: u32 = hex(P.accent, 255);
    pub const ON_ACCENT: u32 = hex(P.on_accent, 255);
    pub(crate) fn with_alpha(c: u32, a: f32) -> u32 {
        let base = (c >> 24) as f32;
        (c & 0x00ff_ffff) | ((base * a.clamp(0.0, 1.0)).round() as u32) << 24
    }
}

/// The launcher's type scale and shapes (`horizon_theme`), in dp.
pub(crate) const SMALL: f32 = horizon_theme::CAPTION.px;
pub(crate) const BODY: f32 = horizon_theme::LABEL.px;
/// Fields; plates (hover rows, selection); pills (buttons, segments).
pub(crate) const RADIUS: f32 = horizon_theme::RADIUS;
pub(crate) const RADIUS_PLATE: f32 = horizon_theme::RADIUS_PLATE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Face {
    Regular,
    Medium,
    /// The wordmark and page titles (the design system's 500).
    SemiBold,
    /// Counts, timestamps and the boot screen's console lines.
    Mono,
}
struct Fonts {
    faces: [FontRef<'static>; 4],
}
impl Fonts {
    fn load() -> Self {
        let f = |b: &'static [u8]| FontRef::try_from_slice(b).expect("bundled font");
        Self {
            faces: [
                f(horizon_theme::fonts::GEIST_REGULAR),
                f(horizon_theme::fonts::GEIST_MEDIUM),
                f(horizon_theme::fonts::GEIST_SEMIBOLD),
                f(horizon_theme::fonts::GEIST_MONO),
            ],
        }
    }
    fn face(&self, f: Face) -> &FontRef<'static> {
        &self.faces[f as usize]
    }
    /// The ab_glyph scale for an em size in pixels.
    fn scale(&self, f: Face, px: f32) -> PxScale {
        let font = self.face(f);
        let upem = font.units_per_em().unwrap_or(1000.0);
        PxScale::from(px * font.height_unscaled() / upem)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Glyph(Face, u16, u16),
    /// A quarter disc (`false`) or a 1px quarter ring of this radius in pixels.
    Corner(u16, bool),
}
#[derive(Clone, Copy)]
struct Cached {
    rect: Option<Rect>,
    /// Offset of the bitmap from the pen (glyphs) in pixels.
    left: f32,
    top: f32,
}
/// Glyphs and corner masks rasterized into the atlas region `horizon/ui-text`.
struct Cache {
    fonts: Fonts,
    entries: HashMap<Key, Cached>,
    shelf: (u32, u32, u32),
    pending: Vec<crate::font::Patch>,
    /// The atlas the entries were uploaded to (a new atlas starts over).
    atlas: usize,
}
thread_local! {
    static CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) };
}
/// Reloaded atlases can occupy the same address with empty run-time slots.
pub fn reset_atlas() {
    CACHE.with(|cache| *cache.borrow_mut() = None);
}
fn with_cache<R>(atlas: &HudAtlas, f: impl FnOnce(&mut Cache) -> R) -> R {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let id = atlas as *const HudAtlas as usize;
        let cache = c.get_or_insert_with(|| Cache {
            fonts: Fonts::load(),
            entries: HashMap::new(),
            shelf: (0, 0, 0),
            pending: Vec::new(),
            atlas: id,
        });
        if cache.atlas != id {
            cache.atlas = id;
            cache.entries.clear();
            cache.shelf = (0, 0, 0);
            cache.pending.clear();
        }
        f(cache)
    })
}
impl Cache {
    fn region(atlas: &HudAtlas) -> Option<Rect> {
        atlas.sprites.get("horizon/ui-text").copied()
    }
    /// Reserve `w x h` pixels; on overflow forget everything and start over.
    fn place(&mut self, atlas: &HudAtlas, w: u32, h: u32) -> Option<(u32, u32, Rect)> {
        let region = Self::region(atlas)?;
        let (mut x, mut y, mut row) = self.shelf;
        if x + w + 1 > region.w {
            x = 0;
            y += row + 1;
            row = 0;
        }
        if y + h > region.h || w > region.w {
            self.entries.clear();
            self.shelf = (0, 0, 0);
            return None;
        }
        self.shelf = (x + w + 1, y, row.max(h));
        Some((x, y, region))
    }
    fn upload(&mut self, atlas: &HudAtlas, w: u32, h: u32, alpha: &[f32]) -> Option<Rect> {
        let (x, y, region) = self.place(atlas, w, h)?;
        let (ax, ay) = (
            (region.u0 * atlas.width as f32).round() as u32 + x,
            (region.v0 * atlas.height as f32).round() as u32 + y,
        );
        let rgba = alpha
            .iter()
            .flat_map(|a| [255, 255, 255, (a.clamp(0.0, 1.0) * 255.0).round() as u8])
            .collect();
        self.pending.push(crate::font::Patch {
            x: ax,
            y: ay,
            width: w,
            height: h,
            rgba,
        });
        let (aw, ah) = (atlas.width as f32, atlas.height as f32);
        Some(Rect {
            u0: ax as f32 / aw,
            v0: ay as f32 / ah,
            u1: (ax + w) as f32 / aw,
            v1: (ay + h) as f32 / ah,
            w,
            h,
        })
    }
    fn glyph(&mut self, atlas: &HudAtlas, face: Face, id: ab_glyph::GlyphId, px: f32) -> Cached {
        let size = (px * 4.0).round() as u16;
        let key = Key::Glyph(face, id.0, size);
        if let Some(c) = self.entries.get(&key) {
            return *c;
        }
        let scale = self.fonts.scale(face, f32::from(size) / 4.0);
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
        let made = match self.fonts.face(face).outline_glyph(glyph) {
            Some(outline) => {
                let b = outline.px_bounds();
                let (w, h) = (b.width() as u32, b.height() as u32);
                let mut alpha = vec![0.0; (w * h) as usize];
                outline.draw(|x, y, c| {
                    if x < w && y < h {
                        alpha[(y * w + x) as usize] = c;
                    }
                });
                // Light text on dark backgrounds reads thin: a gentle boost.
                for a in &mut alpha {
                    *a = a.powf(0.85);
                }
                Cached {
                    rect: self.upload(atlas, w, h, &alpha),
                    left: b.min.x,
                    top: b.min.y,
                }
            }
            None => Cached {
                rect: None,
                left: 0.0,
                top: 0.0,
            },
        };
        if made.rect.is_some() || made.left == 0.0 {
            self.entries.insert(key, made);
        }
        made
    }
    fn corner(&mut self, atlas: &HudAtlas, r: u16, ring: bool) -> Option<Rect> {
        let key = Key::Corner(r, ring);
        if let Some(c) = self.entries.get(&key) {
            return c.rect;
        }
        // The top-left quadrant of a circle centred at (r, r), 4x4 supersampled.
        let n = u32::from(r);
        let rf = f32::from(r);
        let mut alpha = vec![0.0; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        let d = ((px - rf).powi(2) + (py - rf).powi(2)).sqrt();
                        if d <= rf && (!ring || d >= rf - 1.0) {
                            hits += 1;
                        }
                    }
                }
                alpha[(y * n + x) as usize] = hits as f32 / 16.0;
            }
        }
        let rect = self.upload(atlas, n, n, &alpha);
        if rect.is_some() {
            self.entries.insert(
                key,
                Cached {
                    rect,
                    left: 0.0,
                    top: 0.0,
                },
            );
        }
        rect
    }
}

/// One frame of design-system drawing, in dp: the
/// launcher's logical pixels, scaled so the screen reads like the launcher's
/// 1040x700 window at any size.
pub struct Ui<'a> {
    pub atlas: &'a HudAtlas,
    pub batch: &'a mut HudBatch,
    /// The screen in dp.
    pub w: f32,
    pub h: f32,
    /// The cursor in dp.
    pub cursor: (f32, f32),
    /// Physical pixels per dp.
    pub density: f32,
    scale: f32,
}
impl<'a> Ui<'a> {
    /// `cursor` is in GUI pixels, like every screen's input.
    pub fn new(atlas: &'a HudAtlas, batch: &'a mut HudBatch, cursor: (f32, f32)) -> Self {
        let scale = batch.scale;
        let (pw, ph) = (batch.width * scale, batch.height * scale);
        let density = (pw / 1040.0).min(ph / 700.0).clamp(1.0, 4.0);
        Self {
            atlas,
            w: pw / density,
            h: ph / density,
            cursor: (cursor.0 * scale / density, cursor.1 * scale / density),
            density,
            scale,
            batch,
        }
    }
    /// Move rasterized glyphs into the batch's atlas uploads (real frames only).
    pub fn finish(self) {
        let pending = with_cache(self.atlas, |c| std::mem::take(&mut c.pending));
        self.batch.patches.extend(pending);
    }

    // ---- geometry, snapped to physical pixels ----
    fn px_x(&self, v: f32) -> f32 {
        (v * self.density).round()
    }
    fn px_y(&self, v: f32) -> f32 {
        (v * self.density).round()
    }
    fn raw_fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        if x1 > x0 && y1 > y0 {
            let s = self.scale;
            self.batch.fill(
                self.atlas,
                x0 / s,
                y0 / s,
                (x1 - x0) / s,
                (y1 - y0) / s,
                color,
            );
        }
    }
    fn corner_quad(&mut self, rect: Rect, x: f32, y: f32, r: f32, flip: (bool, bool), color: u32) {
        let (u0, u1) = if flip.0 {
            (rect.u1, rect.u0)
        } else {
            (rect.u0, rect.u1)
        };
        let (v0, v1) = if flip.1 {
            (rect.v1, rect.v0)
        } else {
            (rect.v0, rect.v1)
        };
        let s = self.scale;
        let (gx, gy, gr) = (x / s, y / s, r / s);
        self.batch.quad(
            [[gx, gy], [gx + gr, gy], [gx + gr, gy + gr], [gx, gy + gr]],
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
            color,
        );
    }
    fn corners(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, ring: bool, color: u32) {
        if r < 1.0 {
            return;
        }
        let atlas = self.atlas;
        if let Some(rect) = with_cache(atlas, |c| c.corner(atlas, r as u16, ring)) {
            self.corner_quad(rect, x0, y0, r, (false, false), color);
            self.corner_quad(rect, x1 - r, y0, r, (true, false), color);
            self.corner_quad(rect, x1 - r, y1 - r, r, (true, true), color);
            self.corner_quad(rect, x0, y1 - r, r, (false, true), color);
        }
    }
    /// A filled rectangle with rounded corners.
    pub(crate) fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        if color >> 24 == 0 {
            return;
        }
        let (x0, y0, x1, y1) = (
            self.px_x(x),
            self.px_y(y),
            self.px_x(x + w),
            self.px_y(y + h),
        );
        let r = (radius * self.density)
            .round()
            .min(((x1 - x0) / 2.0).floor())
            .min(((y1 - y0) / 2.0).floor())
            .max(0.0);
        if r < 1.0 {
            self.raw_fill(x0, y0, x1, y1, color);
            return;
        }
        self.raw_fill(x0 + r, y0, x1 - r, y1, color);
        self.raw_fill(x0, y0 + r, x0 + r, y1 - r, color);
        self.raw_fill(x1 - r, y0 + r, x1, y1 - r, color);
        self.corners(x0, y0, x1, y1, r, false, color);
    }
    /// A 1px rounded border.
    pub(crate) fn border(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        if color >> 24 == 0 {
            return;
        }
        let (x0, y0, x1, y1) = (
            self.px_x(x),
            self.px_y(y),
            self.px_x(x + w),
            self.px_y(y + h),
        );
        let r = (radius * self.density)
            .round()
            .min(((x1 - x0) / 2.0).floor())
            .min(((y1 - y0) / 2.0).floor())
            .max(0.0);
        self.raw_fill(x0 + r, y0, x1 - r, y0 + 1.0, color);
        self.raw_fill(x0 + r, y1 - 1.0, x1 - r, y1, color);
        self.raw_fill(x0, y0 + r, x0 + 1.0, y1 - r, color);
        self.raw_fill(x1 - 1.0, y0 + r, x1, y1 - r, color);
        self.corners(x0, y0, x1, y1, r, true, color);
    }

    // ---- text ----
    /// The advance of `text` in dp.
    pub(crate) fn measure(&self, face: Face, size: f32, text: &str) -> f32 {
        let px = (size * self.density * 4.0).round() / 4.0;
        with_cache(self.atlas, |c| {
            let font = c.fonts.face(face);
            let scaled = font.as_scaled(c.fonts.scale(face, px));
            let mut w = 0.0;
            let mut prev = None;
            for ch in text.chars() {
                let id = font.glyph_id(ch);
                if let Some(p) = prev {
                    w += scaled.kern(p, id);
                }
                w += scaled.h_advance(id);
                prev = Some(id);
            }
            w / self.density
        })
    }
    /// Draw a line of text in a `line`-tall box whose top is `y` (the
    /// launcher's line heights: about 1.45 × size for the interface);
    /// returns its width in dp.
    pub(crate) fn text_line(
        &mut self,
        face: Face,
        size: f32,
        line: f32,
        x: f32,
        y: f32,
        color: u32,
        text: &str,
    ) -> f32 {
        let px = (size * self.density * 4.0).round() / 4.0;
        let real = color >> 24 != 0;
        let (x0, y0) = (self.px_x(x), self.px_y(y));
        let box_px = line * self.density;
        let atlas = self.atlas;
        let density = self.density;
        let mut quads = Vec::new();
        let width = with_cache(atlas, |c| {
            let scale = c.fonts.scale(face, px);
            let (ascent, descent) = {
                let f = c.fonts.face(face).as_scaled(scale);
                (f.ascent(), f.descent())
            };
            let baseline = y0 + ((box_px - (ascent - descent)) / 2.0 + ascent).round();
            let mut pen = x0;
            let mut prev = None;
            for ch in text.chars() {
                let (id, advance, kern) = {
                    let font = c.fonts.face(face);
                    let id = font.glyph_id(ch);
                    let f = font.as_scaled(scale);
                    (id, f.h_advance(id), prev.map_or(0.0, |p| f.kern(p, id)))
                };
                pen += kern;
                if real && !ch.is_whitespace() {
                    let g = c.glyph(atlas, face, id, px);
                    if let Some(rect) = g.rect {
                        quads.push(((pen + g.left).round(), baseline + g.top, rect));
                    }
                }
                pen += advance;
                prev = Some(id);
            }
            (pen - x0) / density
        });
        let s = self.scale;
        for (gx, gy, rect) in quads {
            self.batch.rect_uv(
                gx / s,
                gy / s,
                rect.w as f32 / s,
                rect.h as f32 / s,
                rect,
                color,
            );
        }
        width
    }
    /// Text in a `1.45 × size` line box whose top is `y`.
    pub(crate) fn text(
        &mut self,
        face: Face,
        size: f32,
        x: f32,
        y: f32,
        color: u32,
        text: &str,
    ) -> f32 {
        self.text_line(face, size, (size * 1.45).round(), x, y, color, text)
    }
    pub(crate) fn text_centered(
        &mut self,
        face: Face,
        size: f32,
        cx: f32,
        y: f32,
        color: u32,
        text: &str,
    ) {
        let w = self.measure(face, size, text);
        self.text(face, size, cx - w / 2.0, y, color, text);
    }

    // ---- the launcher's components ----
}

// ---- the boot screen ----
//
// ONYX fork: the boot screen is the v2 ONYX loading design, drawn in
// code: a pure black field, a shimmering starfield, a diamond outline
// with light travelling its edges, the ONYX wordmark as polished black
// onyx cut by a slit of light with HORIZON CLIENT inside it, and a plain
// white progress bar near the bottom. No console text, no stage line, no
// percentage: the design has none. The layout is the mock's 1920x1080
// design space mapped onto the window (`Ui` dp), so every window size
// keeps the design's proportions. The GDI boot paint
// (`horizon_threads::boot_paint`) mirrors the same proportions
// statically, so the painted frame and the first wgpu frame agree.
//
// The finish sequence (see `boot_phase`) starts at the loader's last
// checkpoints, as the tail of loading itself: the bar flashes and fades
// out completely, only then the lockup bounces with its afterimages,
// then the whole screen thins out over the title already drawn beneath
// it — the fade reveals the homescreen, it never fades to black.

/// The finish sequence's reference length, and its ceiling: the design
/// timeline below is authored against this many seconds, and a slow
/// boot plays the sequence at exactly this speed, never slower.
pub const BOOT_SEQ_TOTAL: Duration = Duration::from_millis(3600);
/// The finish sequence's floor: however fast the boot, the bounce and
/// its afterimages need this long to stay readable.
pub const BOOT_SEQ_MIN: Duration = Duration::from_millis(1800);

/// How long the finish sequence runs for a load that took
/// `load_elapsed` to reach its last checkpoints: a quarter of the load's
/// own time, clamped into [BOOT_SEQ_MIN, BOOT_SEQ_TOTAL] — quick boots
/// finish quick, slow boots get the design's full speed, nothing ever
/// runs slower than the design.
pub fn boot_seq_duration(load_elapsed: Duration) -> Duration {
    (load_elapsed / 4).clamp(BOOT_SEQ_MIN, BOOT_SEQ_TOTAL)
}
/// The bar is fully faded out at this sequence time...
const BOOT_BAR_END: f32 = 0.80;
/// ...and only here does the bounce begin: the two never overlap.
const BOOT_BOUNCE_AT: f32 = 0.95;
const BOOT_BOUNCE_LEN: f32 = 1.15;
const BOOT_FADE_AT: f32 = BOOT_BOUNCE_AT + 1.30;
const BOOT_STAR_FADE_AT: f32 = BOOT_FADE_AT + 0.15;
/// The slit's half height as a fraction of the wordmark's font size (the
/// design cuts 139..201 of its 340px wordmark box, about its centre).
const BOOT_SLIT_HALF: f32 = 0.0912;

fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

/// A CSS fade-in: 0 before `at`, eased to 1 over `dur`.
fn ramp_in(et: f32, at: f32, dur: f32) -> f32 {
    ease_out((et - at) / dur)
}

/// Piecewise-linear keyframes over `(time, value)` pairs, clamped at
/// both ends.
fn keys_at(keys: &[(f32, f32)], t: f32) -> f32 {
    if t <= keys[0].0 {
        return keys[0].1;
    }
    for w in keys.windows(2) {
        if t <= w[1].0 {
            return w[0].1 + (w[1].1 - w[0].1) * ((t - w[0].0) / (w[1].0 - w[0].0));
        }
    }
    keys[keys.len() - 1].1
}

/// One element's bounce at sequence time `t`: the design's keyframe
/// shape as `(u, scale, dy)` triples (dy in design px), run over `len`
/// from `start`. Before the bounce the element rests;
/// after it, it keeps its final keyframe until the fade takes it.
fn bounce_at(keys: &[(f32, f32, f32)], start: f32, len: f32, t: f32) -> (f32, f32) {
    let u = (t - start) / len;
    if u <= 0.0 {
        return (1.0, 0.0);
    }
    if u >= 1.0 {
        let k = keys[keys.len() - 1];
        return (k.1, k.2);
    }
    for w in keys.windows(2) {
        if u <= w[1].0 {
            let f = (u - w[0].0) / (w[1].0 - w[0].0);
            return (
                w[0].1 + (w[1].1 - w[0].1) * f,
                w[0].2 + (w[1].2 - w[0].2) * f,
            );
        }
    }
    let k = keys[keys.len() - 1];
    (k.1, k.2)
}

const LOGO_BOUNCE: [(f32, f32, f32); 5] = [
    (0.0, 1.0, 0.0),
    (0.28, 1.065, -7.0),
    (0.52, 0.982, 1.0),
    (0.74, 1.022, -2.0),
    (1.0, 1.008, 0.0),
];
const GEM_BOUNCE: [(f32, f32, f32); 5] = [
    (0.0, 1.0, 0.0),
    (0.30, 1.028, 0.0),
    (0.55, 0.994, 0.0),
    (0.78, 1.010, 0.0),
    (1.0, 1.003, 0.0),
];
const TAG_BOUNCE: [(f32, f32, f32); 5] = [
    (0.0, 1.0, 0.0),
    (0.30, 1.045, 0.0),
    (0.55, 0.990, 0.0),
    (0.78, 1.012, 0.0),
    (1.0, 1.004, 0.0),
];
/// The afterimage ghosts: (delay after the bounce starts, duration,
/// peak alpha, final scale). Their colours are white, periwinkle and
/// blue, in this order.
const BOOT_GHOSTS: [(f32, f32, f32, f32); 3] = [
    (0.0, 1.0, 0.5, 1.13),
    (0.1, 1.1, 0.3, 1.25),
    (0.2, 1.2, 0.16, 1.39),
];

/// Where every animated piece of the boot screen is at one instant: the
/// finish sequence's state, computed purely from its elapsed time so the
/// ordering (bar gone before the bounce, ghosts inside the bounce,
/// black at the end) is testable without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BootPhase {
    /// The bar's opacity: 1 while loading and while the bar holds at
    /// 100%, 0 from `BOOT_BAR_END` on.
    pub bar_alpha: f32,
    /// The bar's one brightness flash, peaking at 0.15s.
    pub bar_flash: f32,
    /// The bar's downward drift (design px) as it fades.
    pub bar_drop: f32,
    pub logo_scale: f32,
    /// Vertical offsets in design px.
    pub logo_dy: f32,
    pub gem_scale: f32,
    pub gem_dy: f32,
    pub tag_scale: f32,
    pub tag_dy: f32,
    /// The three ghosts as (alpha, scale); alpha 0 = not present.
    pub ghosts: [(f32, f32); 3],
    /// The whole lockup's fade: 1 = fully shown.
    pub fade: f32,
    /// The starfield's own, later fade.
    pub star_fade: f32,
    pub halo_fade: f32,
    /// The sequence has played out: the boot screen draws nothing.
    pub done: bool,
}

impl BootPhase {
    /// The state while loading (no sequence running).
    const NEUTRAL: Self = Self {
        bar_alpha: 1.0,
        bar_flash: 0.0,
        bar_drop: 0.0,
        logo_scale: 1.0,
        logo_dy: 0.0,
        gem_scale: 1.0,
        gem_dy: 0.0,
        tag_scale: 1.0,
        tag_dy: 0.0,
        ghosts: [(0.0, 1.0); 3],
        fade: 1.0,
        star_fade: 1.0,
        halo_fade: 1.0,
        done: false,
    };
}

/// The finish sequence at `finish` seconds from its trigger (or the
/// neutral loading state for `None`), on a timeline of `seq_total`
/// seconds: the design's keys are authored against `BOOT_SEQ_TOTAL`
/// and scale proportionally, so the ordering (bar gone before the
/// bounce, ghosts inside the bounce, stars fading last) holds at any
/// total in [BOOT_SEQ_MIN, BOOT_SEQ_TOTAL].
pub(crate) fn boot_phase(finish: Option<f32>, seq_total: f32) -> BootPhase {
    let Some(t) = finish else {
        return BootPhase::NEUTRAL;
    };
    let s = seq_total / BOOT_SEQ_TOTAL.as_secs_f32();
    let bar_end = BOOT_BAR_END * s;
    let bounce_start = BOOT_BOUNCE_AT * s;
    let bounce_len = BOOT_BOUNCE_LEN * s;
    let flash_at = 0.15 * s;
    // The bar: holds, flashes bright once (peak at the flash key), then
    // fades out while dropping; by the bar key it is gone, before
    // anything moves.
    let fade_u = ease_out((t - flash_at) / (bar_end - flash_at));
    let bar_alpha = 1.0 - fade_u;
    let bar_flash = if t <= flash_at {
        t / flash_at
    } else {
        1.0 - fade_u
    };
    let (logo_scale, logo_dy) = bounce_at(&LOGO_BOUNCE, bounce_start, bounce_len, t);
    let (gem_scale, gem_dy) = bounce_at(&GEM_BOUNCE, bounce_start + 0.02 * s, bounce_len, t);
    let (tag_scale, tag_dy) = bounce_at(&TAG_BOUNCE, bounce_start + 0.04 * s, bounce_len, t);
    // The fade-out: everything rises slightly and scales a touch as it
    // goes, the stars starting later and ending last.
    let out_u = ease_out((t - BOOT_FADE_AT * s) / s);
    let mut ghosts = [(0.0f32, 1.0f32); 3];
    for (g, &(delay, dur, peak, target)) in ghosts.iter_mut().zip(&BOOT_GHOSTS) {
        let u = (t - (bounce_start + delay * s)) / (dur * s);
        if (0.0..=1.0).contains(&u) {
            let a = if u < 0.14 {
                peak * (u / 0.14)
            } else {
                peak * (1.0 - ease_out((u - 0.14) / 0.86))
            };
            *g = (a, 1.0 + (target - 1.0) * ease_out(u));
        }
    }
    BootPhase {
        bar_alpha,
        bar_flash,
        bar_drop: 10.0 * fade_u,
        logo_scale: logo_scale * (1.0 + 0.014 * out_u),
        logo_dy: logo_dy - 10.0 * out_u,
        gem_scale: gem_scale * (1.0 + 0.006 * out_u),
        gem_dy: gem_dy - 6.0 * out_u,
        tag_scale: tag_scale * (1.0 + 0.006 * out_u),
        tag_dy: tag_dy - 6.0 * out_u,
        ghosts,
        fade: 1.0 - out_u,
        star_fade: 1.0 - ease_out((t - BOOT_STAR_FADE_AT * s) / (1.2 * s)),
        halo_fade: 1.0 - ease_out((t - (BOOT_FADE_AT + 0.05) * s) / (1.3 * s)),
        done: t >= seq_total,
    }
}

/// A colour in the batch's encoding at a fractional alpha.
fn boot_rgba(rgb: u32, a: f32) -> u32 {
    color::hex(rgb, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Deterministic hash noise for the starfield: fixed positions every
/// frame and every run, no RNG state.
fn boot_hash(n: u32) -> u32 {
    let mut x = n.wrapping_mul(0x9E37_79B1);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x
}
fn boot_unit(n: u32) -> f32 {
    (boot_hash(n) >> 8) as f32 / 16_777_216.0
}

/// The design's twinkle curve (its opacity x brightness keyframes),
/// normalised so the brightest point is 1: an uneven flicker, not a
/// smooth pulse.
fn boot_twinkle(u: f32) -> f32 {
    const KEYS: [(f32, f32); 8] = [
        (0.0, 0.874),
        (0.12, 0.286),
        (0.26, 1.0),
        (0.40, 0.204),
        (0.56, 0.941),
        (0.72, 0.351),
        (0.86, 0.698),
        (1.0, 0.874),
    ];
    keys_at(&KEYS, u.rem_euclid(1.0))
}

/// The wordmark's placement, shared by the pieces that hang off it (the
/// diamond's slit, the seam, the tag): its centre and font size in dp,
/// and the slit's y range in dp.
struct BootMark {
    cx: f32,
    cy: f32,
    size: f32,
    slit: (f32, f32),
}

fn boot_mark(ui: &Ui) -> BootMark {
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    // The design's wordmark spans ~61.5% of the width; size the font by
    // its measured advance so the span holds at any window size.
    let size = 0.615 * ui.w * 100.0 / ui.measure(Face::SemiBold, 100.0, "ONYX").max(1.0);
    let cy = 480.0 * sy;
    BootMark {
        cx: ui.w / 2.0 - 8.0 * sx,
        cy,
        size,
        slit: (cy - size * BOOT_SLIT_HALF, cy + size * BOOT_SLIT_HALF),
    }
}

/// Scale the vertices drawn since `start` about a centre given in dp,
/// with an extra vertical offset in dp: the batch-space form of one
/// element's transform.
fn boot_xf(ui: &mut Ui, start: usize, centre: (f32, f32), scale: f32, dy: f32) {
    let k = ui.density / ui.scale;
    ui.batch
        .scale_since(start, centre.0 * k, centre.1 * k, scale, scale);
    if dy != 0.0 {
        for v in &mut ui.batch.vertices[start..] {
            v.pos[1] += dy * k;
        }
    }
}

/// Rewrite the colours of the vertices in `range` from their position
/// (in dp): the wordmark's gradient is a function of height, its sheen
/// of x.
fn boot_recolour(ui: &mut Ui, range: std::ops::Range<usize>, f: impl Fn(f32, f32) -> u32) {
    let k = ui.density / ui.scale;
    for v in &mut ui.batch.vertices[range] {
        v.color = f(v.pos[0] / k, v.pos[1] / k);
    }
}

/// A filled ellipse as a fan of thin quads (the batch has no disc
/// primitive); 24 segments read smooth at these sizes.
fn boot_disc(ui: &mut Ui, cx: f32, cy: f32, rx: f32, ry: f32, color: u32) {
    if color >> 24 == 0 || rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let k = ui.density / ui.scale;
    let (cx, cy, rx, ry) = (cx * k, cy * k, rx * k, ry * k);
    let mut prev = [cx + rx, cy];
    for i in 1..=24 {
        let a = std::f32::consts::TAU * i as f32 / 24.0;
        let p = [cx + rx * a.cos(), cy + ry * a.sin()];
        ui.batch
            .fill_quad(ui.atlas, [[cx, cy], prev, p, [cx, cy]], color);
        prev = p;
    }
}

/// A thick line segment as one quad, endpoints and width in dp.
fn boot_line(ui: &mut Ui, a: (f32, f32), b: (f32, f32), width: f32, color: u32) {
    if color >> 24 == 0 {
        return;
    }
    let k = ui.density / ui.scale;
    let (ax, ay) = (a.0 * k, a.1 * k);
    let (bx, by) = (b.0 * k, b.1 * k);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.01 {
        return;
    }
    let (nx, ny) = (-dy / len * width * k / 2.0, dx / len * width * k / 2.0);
    ui.batch.fill_quad(
        ui.atlas,
        [
            [ax + nx, ay + ny],
            [bx + nx, by + ny],
            [bx - nx, by - ny],
            [ax - nx, ay - ny],
        ],
        color,
    );
}

/// A design-space (1920x1080) point in dp.
fn boot_point(ui: &Ui, x: f32, y: f32) -> (f32, f32) {
    (x * ui.w / 1920.0, y * ui.h / 1080.0)
}

/// One line segment with the slit band cut out of it: the diamond's
/// lines pass behind the slit, like the wordmark does.
fn boot_seg(ui: &mut Ui, a: (f32, f32), b: (f32, f32), slit: (f32, f32), width: f32, color: u32) {
    let mut cuts = [0.0f32, 1.0, 0.0, 0.0];
    let mut n = 2;
    if (b.1 - a.1).abs() > f32::EPSILON {
        for bound in [slit.0, slit.1] {
            let t = (bound - a.1) / (b.1 - a.1);
            if t > 0.0 && t < 1.0 {
                cuts[n] = t;
                n += 1;
            }
        }
    }
    let mut cuts = cuts[..n].to_vec();
    cuts.sort_by(f32::total_cmp);
    for w in cuts.windows(2) {
        let pa = (a.0 + (b.0 - a.0) * w[0], a.1 + (b.1 - a.1) * w[0]);
        let pb = (a.0 + (b.0 - a.0) * w[1], a.1 + (b.1 - a.1) * w[1]);
        let mid = (pa.1 + pb.1) / 2.0;
        if mid > slit.0 && mid < slit.1 {
            continue;
        }
        boot_line(ui, pa, pb, width, color);
    }
}

fn boot_poly(
    ui: &mut Ui,
    pts: &[(f32, f32)],
    closed: bool,
    slit: (f32, f32),
    width: f32,
    color: u32,
) {
    for pair in pts.windows(2) {
        boot_seg(ui, pair[0], pair[1], slit, width, color);
    }
    if closed && pts.len() > 2 {
        boot_seg(ui, *pts.last().unwrap(), pts[0], slit, width, color);
    }
}

/// A comet: light travelling a polyline. `head` is the distance the
/// brightest point has reached along the path; each layer is a trailing
/// segment (length in thousandths of the path, width in dp, colour,
/// alpha). The slit cuts the comet like it cuts the lines.
fn boot_comet(
    ui: &mut Ui,
    pts: &[(f32, f32)],
    closed: bool,
    head: f32,
    layers: &[(f32, f32, u32, f32)],
    slit: (f32, f32),
) {
    let mut segs: Vec<((f32, f32), (f32, f32), f32, f32)> = Vec::new();
    let mut total = 0.0f32;
    let count = if closed { pts.len() } else { pts.len() - 1 };
    for i in 0..count {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        segs.push((a, b, total, len));
        total += len;
    }
    if total <= 0.0 {
        return;
    }
    for &(len_u, width, rgb, alpha) in layers {
        if alpha <= 0.0 {
            continue;
        }
        let len = len_u / 1000.0 * total;
        let mut spans = [(0.0f32, 0.0f32); 2];
        let nspans = if closed {
            let h = head.rem_euclid(total);
            let lo = h - len;
            if lo < 0.0 {
                spans = [(0.0, h), (total + lo, total)];
                2
            } else {
                spans[0] = (lo, h);
                1
            }
        } else {
            spans[0] = ((head - len).max(0.0), head.min(total));
            1
        };
        for &(s0, s1) in &spans[..nspans] {
            if s1 <= s0 {
                continue;
            }
            for &(a, b, start, seg_len) in &segs {
                let (o0, o1) = (s0.max(start), s1.min(start + seg_len));
                if o1 <= o0 || seg_len <= 0.0 {
                    continue;
                }
                let pa = (
                    a.0 + (b.0 - a.0) * (o0 - start) / seg_len,
                    a.1 + (b.1 - a.1) * (o0 - start) / seg_len,
                );
                let pb = (
                    a.0 + (b.0 - a.0) * (o1 - start) / seg_len,
                    a.1 + (b.1 - a.1) * (o1 - start) / seg_len,
                );
                boot_seg(ui, pa, pb, slit, width, boot_rgba(rgb, alpha));
            }
        }
    }
}

/// How far a comet's head has travelled: it sweeps the first `travel`
/// seconds of its cycle (plus 30% past the end, so the tail clears),
/// starting after `delay`.
fn comet_head(et: f32, cycle: f32, delay: f32, travel: f32, total: f32) -> f32 {
    let lt = et.rem_euclid(cycle);
    ((lt - delay) / travel).clamp(0.0, 1.0) * 1.3 * total
}

/// The gem node's flash cycle: (alpha, scale) — a quick flare as a comet
/// reaches the node, then rest.
fn gem_flash(et: f32, delay: f32) -> (f32, f32) {
    let lt = et.rem_euclid(6.5);
    if lt < delay {
        return (0.55, 1.0);
    }
    let u = (lt - delay) / 6.5;
    if u < 0.03 {
        let f = u / 0.03;
        (0.55 + 0.45 * f, 1.0 + 1.3 * f)
    } else if u < 0.15 {
        let f = (u - 0.03) / 0.12;
        (1.0 - 0.45 * f, 2.3 - 1.3 * f)
    } else {
        (0.55, 1.0)
    }
}

/// The halo behind the lockup: a soft radial glow, approximated with
/// layered translucent discs (the HUD has no blur), gently pulsing.
fn boot_halo(ui: &mut Ui, et: f32, phase: &BootPhase) {
    let a = phase.fade * phase.halo_fade;
    if a <= 0.0 {
        return;
    }
    let pulse = 0.825 - 0.175 * (std::f32::consts::TAU * et / 6.0).cos();
    let (cx, cy) = boot_point(ui, 960.0, 480.0);
    let (rx, ry) = (ui.w * 840.0 / 1920.0, ui.h * 330.0 / 1080.0);
    // Eight layers of ~0.013 stack to the design's ~0.10 at the centre.
    for i in 0..8 {
        let f = (i as f32 + 1.0) / 8.0;
        boot_disc(
            ui,
            cx,
            cy,
            rx * f,
            ry * f,
            boot_rgba(0xCDD8FF, 0.0131 * pulse * a),
        );
    }
}

/// The starfield: ~200 stars in six groups of different sizes and
/// twinkle periods, plus 16 sparkles with cross flares.
fn boot_stars(ui: &mut Ui, et: f32, phase: &BootPhase) {
    let ramp = ramp_in(et, 0.0, 1.6) * phase.fade * phase.star_fade;
    if ramp <= 0.0 {
        return;
    }
    let m = (ui.w / 1920.0).min(ui.h / 1080.0);
    // (count, size in design px, twinkle period, phase offset)
    const GROUPS: [(u32, f32, f32, f32); 6] = [
        (72, 2.0, 5.2, 0.0),
        (62, 1.0, 5.3, 2.0),
        (28, 2.0, 3.1, 1.2),
        (22, 2.0, 4.6, 3.5),
        (10, 3.0, 2.7, 0.6),
        (8, 3.0, 4.1, 2.8),
    ];
    const ALPHAS: [f32; 6] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
    let mut seed = 0u32;
    for (count, size, period, off) in GROUPS {
        for _ in 0..count {
            seed += 1;
            let x = boot_unit(seed * 4) * ui.w;
            let y = boot_unit(seed * 4 + 1) * ui.h;
            let base = ALPHAS[(boot_hash(seed * 4 + 2) % 6) as usize];
            let u = (et + off) / period + boot_unit(seed * 4 + 3);
            let a = base * boot_twinkle(u) * ramp;
            if a < 0.02 {
                continue;
            }
            let s = (size * m).max(0.6);
            ui.rounded(
                x - s / 2.0,
                y - s / 2.0,
                s,
                s,
                s / 2.0,
                boot_rgba(0xFFFFFF, a),
            );
        }
    }
    // The sparkles pulse on the design's sparkle curve: a glow disc, a
    // bright dot and thin cross arms.
    const SPARK_A: [(f32, f32); 7] = [
        (0.0, 0.15),
        (0.18, 0.9),
        (0.32, 0.3),
        (0.55, 1.0),
        (0.74, 0.35),
        (0.88, 0.75),
        (1.0, 0.15),
    ];
    const SPARK_S: [(f32, f32); 7] = [
        (0.0, 0.55),
        (0.18, 1.05),
        (0.32, 0.7),
        (0.55, 1.2),
        (0.74, 0.75),
        (0.88, 1.0),
        (1.0, 0.55),
    ];
    for _ in 0..16 {
        seed += 1;
        let x = boot_unit(seed * 4) * ui.w;
        let y = boot_unit(seed * 4 + 1) * ui.h;
        let size = 3.0 + 3.0 * boot_unit(seed * 4 + 2);
        let period = 3.2 + 2.0 * boot_unit(seed * 4 + 3);
        let u = (et / period + boot_unit(seed * 7 + 5)).fract();
        let a = keys_at(&SPARK_A, u) * ramp;
        if a < 0.02 {
            continue;
        }
        let sc = keys_at(&SPARK_S, u) * m;
        boot_disc(
            ui,
            x,
            y,
            size * 1.1 * sc,
            size * 1.1 * sc,
            boot_rgba(0xBECDFF, 0.20 * a),
        );
        boot_disc(
            ui,
            x,
            y,
            size * 0.5 * sc,
            size * 0.5 * sc,
            boot_rgba(0xFFFFFF, a),
        );
        let arm = 15.0 * sc;
        let w = (1.5 * m).max(0.4);
        boot_line(
            ui,
            (x - arm, y),
            (x + arm, y),
            w,
            boot_rgba(0xFFFFFF, 0.55 * a),
        );
        boot_line(
            ui,
            (x, y - arm),
            (x, y + arm),
            w,
            boot_rgba(0xFFFFFF, 0.55 * a),
        );
    }
}

/// Both diamond outline sets (the faint constellation and the brighter
/// gem), their comets and their vertex nodes, bounced as one group
/// about the diamond's centre.
fn boot_diamond(ui: &mut Ui, et: f32, phase: &BootPhase, mark: &BootMark) {
    if phase.fade <= 0.0 {
        return;
    }
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    let m = sx.min(sy);
    let slit = mark.slit;
    let start = ui.batch.vertices.len();
    let const_ramp = ramp_in(et, 0.5, 1.8) * phase.fade;
    let gem_ramp = ramp_in(et, 0.4, 1.6) * phase.fade;
    let const_outer = [
        boot_point(ui, 960.0, 118.0),
        boot_point(ui, 645.0, 391.0),
        boot_point(ui, 960.0, 872.0),
        boot_point(ui, 1275.0, 391.0),
    ];
    let const_inner = [
        boot_point(ui, 960.0, 118.0),
        boot_point(ui, 822.0, 391.0),
        boot_point(ui, 960.0, 872.0),
        boot_point(ui, 1098.0, 391.0),
    ];
    let gem_outer = [
        boot_point(ui, 960.0, 139.0),
        boot_point(ui, 634.0, 400.0),
        boot_point(ui, 960.0, 899.0),
        boot_point(ui, 1286.0, 400.0),
    ];
    let gem_inner = [
        boot_point(ui, 960.0, 139.0),
        boot_point(ui, 818.0, 400.0),
        boot_point(ui, 960.0, 899.0),
        boot_point(ui, 1102.0, 400.0),
    ];
    // The constellation: faintest white lines, its own slower comets.
    let const_col = boot_rgba(0xFFFFFF, 0.28 * const_ramp);
    boot_poly(ui, &const_outer, true, slit, 1.4 * m, const_col);
    boot_poly(ui, &const_inner, true, slit, 1.4 * m, const_col);
    boot_seg(
        ui,
        boot_point(ui, 645.0, 391.0),
        boot_point(ui, 1275.0, 391.0),
        slit,
        1.4 * m,
        const_col,
    );
    let total = |pts: &[(f32, f32)], closed: bool| {
        let n = if closed { pts.len() } else { pts.len() - 1 };
        (0..n).fold(0.0f32, |acc, i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            acc + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
        })
    };
    let const_layers = [
        (240.0, 5.0 * m, 0xFFFFFFu32, 0.15 * const_ramp),
        (240.0, 1.6 * m, 0xFFFFFF, 0.35 * const_ramp),
        (110.0, 2.4 * m, 0xFFFFFF, 0.6 * const_ramp),
        (28.0, 3.4 * m, 0xFFFFFF, 1.0 * const_ramp),
    ];
    boot_comet(
        ui,
        &const_outer,
        true,
        comet_head(et, 9.0, 2.2, 4.05, total(&const_outer, true)),
        &const_layers,
        slit,
    );
    boot_comet(
        ui,
        &const_inner,
        true,
        comet_head(et, 9.0, 3.6, 4.05, total(&const_inner, true)),
        &const_layers,
        slit,
    );
    let const_mid = [boot_point(ui, 645.0, 391.0), boot_point(ui, 1275.0, 391.0)];
    boot_comet(
        ui,
        &const_mid,
        false,
        comet_head(et, 9.0, 5.0, 4.05, total(&const_mid, false)),
        &const_layers,
        slit,
    );
    // The gem: brighter periwinkle-white lines, faster comets on every
    // edge, flashing nodes where the comets land.
    let gem_col = boot_rgba(0xC9D1F5, 0.5 * gem_ramp);
    boot_poly(ui, &gem_outer, true, slit, 1.6 * m, gem_col);
    boot_poly(ui, &gem_inner, true, slit, 1.6 * m, gem_col);
    boot_seg(
        ui,
        boot_point(ui, 634.0, 400.0),
        boot_point(ui, 1286.0, 400.0),
        slit,
        1.6 * m,
        gem_col,
    );
    let gem_layers = [
        (300.0, 6.0 * m, 0xD6DEFFu32, 0.12 * gem_ramp),
        (300.0, 2.2 * m, 0xCFD8FF, 0.28 * gem_ramp),
        (190.0, 2.4 * m, 0xDBE1FF, 0.45 * gem_ramp),
        (100.0, 2.8 * m, 0xEEF1FF, 0.70 * gem_ramp),
        (28.0, 3.2 * m, 0xFFFFFF, 1.0 * gem_ramp),
    ];
    let (top, left, right, bottom) = (gem_outer[0], gem_outer[1], gem_outer[3], gem_outer[2]);
    let (in_l, in_r) = (gem_inner[1], gem_inner[3]);
    for path in [
        [top, left, bottom],
        [top, in_l, bottom],
        [top, right, bottom],
        [top, in_r, bottom],
    ] {
        boot_comet(
            ui,
            &path,
            false,
            comet_head(et, 6.5, 1.8, 2.99, total(&path, false)),
            &gem_layers,
            slit,
        );
    }
    let gem_mid = [left, right];
    boot_comet(
        ui,
        &gem_mid,
        false,
        comet_head(et, 6.5, 2.75, 2.99, total(&gem_mid, false)),
        &gem_layers,
        slit,
    );
    // Constellation nodes: a halo disc and a twinkling dot.
    for (x, y, off) in [
        (960.0, 118.0, -0.5),
        (645.0, 391.0, -1.7),
        (1275.0, 391.0, -2.6),
        (960.0, 872.0, -3.4),
        (822.0, 391.0, -1.1),
        (1098.0, 391.0, -2.2),
    ] {
        let (x, y) = boot_point(ui, x, y);
        let tw = 0.35 + 0.65 * boot_twinkle(et / 4.2 + off);
        boot_disc(
            ui,
            x,
            y,
            11.0 * sx,
            11.0 * sy,
            boot_rgba(0xFFFFFF, 0.10 * tw * const_ramp),
        );
        boot_disc(
            ui,
            x,
            y,
            3.4 * m,
            3.4 * m,
            boot_rgba(0xFFFFFF, tw * const_ramp),
        );
    }
    // Gem nodes: a dot with a cross flare, flashing as the comets pass.
    for (x, y, delay, cross) in [
        (960.0, 139.0, 1.8, true),
        (634.0, 400.0, 2.75, true),
        (1286.0, 400.0, 2.75, true),
        (960.0, 899.0, 4.1, true),
        (818.0, 400.0, 2.64, false),
        (1102.0, 400.0, 2.64, false),
    ] {
        let (x, y) = boot_point(ui, x, y);
        let (fa, fs) = gem_flash(et, delay);
        let a = fa * gem_ramp;
        boot_disc(ui, x, y, 3.6 * m, 3.6 * m, boot_rgba(0xFFFFFF, a));
        if cross {
            let arm = 16.0 * fs * m;
            let w = (1.2 * m).max(0.4);
            boot_line(ui, (x - arm, y), (x + arm, y), w, boot_rgba(0xFFFFFF, a));
            boot_line(ui, (x, y - arm), (x, y + arm), w, boot_rgba(0xFFFFFF, a));
        }
    }
    let centre = boot_point(ui, 960.0, 519.0);
    boot_xf(ui, start, centre, phase.gem_scale, phase.gem_dy * sy);
}

/// The wordmark drawn once, clipped to outside the slit; returns the
/// vertex range the draw produced.
fn boot_text_clipped(
    ui: &mut Ui,
    size: f32,
    x: f32,
    y: f32,
    color: u32,
    clip: (f32, f32, f32, f32),
) -> std::ops::Range<usize> {
    let start = ui.batch.vertices.len();
    ui.text_line(Face::SemiBold, size, size, x, y, color, "ONYX");
    ui.batch.clip_since(start, clip.0, clip.1, clip.2, clip.3);
    start..ui.batch.vertices.len()
}

/// The ONYX wordmark as polished black onyx: a soft vertical gradient
/// with faint stone banding (painted into the glyph vertices by height),
/// a blurred periwinkle bloom behind it, a light sweep clipped to the
/// letters, and the slit cut through all of it.
fn boot_wordmark(ui: &mut Ui, et: f32, phase: &BootPhase, mark: &BootMark) {
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    let k = ui.density / ui.scale;
    let (cx, cy, size) = (mark.cx, mark.cy, mark.size);
    // The glyph cache's shelf packing cannot hold four wordmark glyphs
    // rasterised at full size in the boot atlas (its text region is half
    // the GUI atlas'), so rasterise under a cap and let the group
    // transform below scale the quads up to the true size.
    let cap = if ui.atlas.width <= 1024 { 340.0 } else { 470.0 };
    let draw = size.min(cap / ui.density);
    let upscale = size / draw;
    let w_draw = ui.measure(Face::SemiBold, draw, "ONYX");
    let (x, y) = (cx - w_draw / 2.0, cy - draw / 2.0);
    let rise = ramp_in(et, 0.25, 1.3);
    let alpha = rise * phase.fade;
    let above = (0.0, 0.0, ui.batch.width, mark.slit.0 * k);
    let below = (0.0, mark.slit.1 * k, ui.batch.width, ui.batch.height);
    let group = ui.batch.vertices.len();
    // The bloom: offset copies in periwinkle, clipped out of the slit,
    // then scaled a touch larger about the centre.
    let bloom = boot_rgba(0xD5DCFF, 0.11 * alpha);
    for (dx, dy) in [(-2.2 * sx, 2.0 * sy), (2.2 * sx, 2.0 * sy), (0.0, 4.2 * sy)] {
        for clip in [above, below] {
            boot_text_clipped(ui, draw, x + dx, y + dy, bloom, clip);
        }
    }
    ui.batch.scale_since(group, cx * k, cy * k, 1.012, 1.012);
    // The ghosts (finish sequence only): flat copies expanding about
    // the centre, slit-masked like the wordmark.
    for (gi, &(ga, gs)) in phase.ghosts.iter().enumerate() {
        if ga <= 0.0 {
            continue;
        }
        const GHOST_COLS: [u32; 3] = [0xFFFFFF, 0xBCC9FF, 0x8EA2FF];
        let start = ui.batch.vertices.len();
        let col = boot_rgba(GHOST_COLS[gi], ga * alpha);
        for clip in [above, below] {
            boot_text_clipped(ui, draw, x, y, col, clip);
        }
        ui.batch.scale_since(start, cx * k, cy * k, gs, gs);
    }
    // The wordmark itself, drawn white; the gradient is painted over
    // its vertices by height below.
    let main = boot_text_clipped(ui, draw, x, y, color::hex(0xFFFFFF, 255), above);
    let main2 = boot_text_clipped(ui, draw, x, y, color::hex(0xFFFFFF, 255), below);
    // The sheen: a bright band sweeping the letters once per 7.5s cycle,
    // its falloff painted by x.
    let sheen = boot_text_clipped(ui, draw, x, y, color::hex(0xFFFFFF, 255), above);
    let sheen2 = boot_text_clipped(ui, draw, x, y, color::hex(0xFFFFFF, 255), below);
    let top = cy - size / 2.0;
    let paint = |_px: f32, py: f32| {
        let fy = cy + (py - cy) * upscale;
        boot_onyx_colour((fy - top) / size, alpha)
    };
    boot_recolour(ui, main, paint);
    boot_recolour(ui, main2, paint);
    let w_full = w_draw * upscale;
    let cycle = et - 2.2;
    let sheen_u = (cycle.rem_euclid(7.5)) / 2.85;
    let sheen_on = cycle >= 0.0 && cycle.rem_euclid(7.5) < 2.85;
    let paint_sheen = |px: f32, _py: f32| {
        if !sheen_on {
            return color::hex(0xFFFFFF, 0);
        }
        let frac = (px - (cx - w_full / 2.0)) / w_full.max(1.0);
        let centre = -0.78 + 4.16 * sheen_u;
        let a = 0.34 * 0.9 * (1.0 - (frac - centre).abs() / 0.312).clamp(0.0, 1.0) * alpha;
        boot_rgba(0xFFFFFF, a)
    };
    boot_recolour(ui, sheen, paint_sheen);
    boot_recolour(ui, sheen2, paint_sheen);
    // The whole lockup group: up to true size, the intro rise, the
    // bounce and the fade's rise.
    let intro_dy = 16.0 * sy * (1.0 - rise);
    boot_xf(
        ui,
        group,
        (cx, cy),
        upscale * phase.logo_scale,
        phase.logo_dy * sy + intro_dy,
    );
}

/// The polished-onyx colour at height fraction `f` of the wordmark: the
/// design's soft gradient stops with its faint stone banding composited
/// over, at the lockup's current alpha.
fn boot_onyx_colour(f: f32, alpha: f32) -> u32 {
    const STOPS: [(f32, [f32; 3]); 8] = [
        (0.0, [253.0, 253.0, 255.0]),
        (0.188, [238.0, 240.0, 248.0]),
        (0.388, [216.0, 219.0, 231.0]),
        (0.494, [194.0, 197.0, 210.0]),
        (0.629, [166.0, 168.0, 182.0]),
        (0.788, [131.0, 133.0, 143.0]),
        (0.947, [91.0, 92.0, 102.0]),
        (1.0, [74.0, 75.0, 84.0]),
    ];
    let f = f.clamp(0.0, 1.0);
    let mut rgb = STOPS[STOPS.len() - 1].1;
    for w in STOPS.windows(2) {
        if f <= w[1].0 {
            let t = (f - w[0].0) / (w[1].0 - w[0].0);
            rgb = std::array::from_fn(|i| w[0].1[i] + (w[1].1[i] - w[0].1[i]) * t);
            break;
        }
    }
    let (over, a): ([f32; 3], f32) = if f < 0.0706 {
        ([255.0, 255.0, 255.0], 0.05)
    } else if f < 0.147 {
        ([10.0, 10.0, 18.0], 0.05)
    } else if f < 0.229 {
        ([255.0, 255.0, 255.0], 0.04)
    } else {
        ([10.0, 10.0, 18.0], 0.04)
    };
    let mix = |c: f32, o: f32| (c * (1.0 - a) + o * a).round() as u32;
    let packed = mix(rgb[0], over[0]) << 16 | mix(rgb[1], over[1]) << 8 | mix(rgb[2], over[2]);
    boot_rgba(packed, alpha)
}

/// The seam of light inside the slit: a thin horizontal gradient line,
/// pulsing slowly.
fn boot_seam(ui: &mut Ui, et: f32, phase: &BootPhase, mark: &BootMark) {
    let ramp = ramp_in(et, 1.1, 1.2) * phase.fade;
    if ramp <= 0.0 {
        return;
    }
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    let pulse = 0.70 + 0.25 * (std::f32::consts::TAU * et / 5.0).sin();
    let (x0, x1) = (300.0 * sx, 1620.0 * sx);
    let thick = (2.0 * sy).max(0.8);
    const PROFILE: [(f32, f32); 5] = [(0.0, 0.0), (0.3, 0.4), (0.5, 0.65), (0.7, 0.4), (1.0, 0.0)];
    for i in 0..24 {
        let (f0, f1) = (i as f32 / 24.0, (i + 1) as f32 / 24.0);
        let fm = (f0 + f1) / 2.0;
        let t = 1.0 - (2.0 * (fm - 0.5)).abs();
        let mix = |e: f32, c: f32| (e + (c - e) * t).round() as u32;
        let packed = mix(178.0, 235.0) << 16 | mix(193.0, 240.0) << 8 | mix(255.0, 255.0);
        let a = keys_at(&PROFILE, fm) * pulse * ramp;
        ui.rounded(
            x0 + (x1 - x0) * f0,
            mark.cy - thick / 2.0,
            (x1 - x0) / 24.0 + 0.4,
            thick,
            thick / 2.0,
            boot_rgba(packed, a),
        );
    }
}

/// HORIZON CLIENT in the slit: letterspaced mono, periwinkle, with a
/// soft glow of offset copies. The design centres it ~70 design px left
/// of centre.
fn boot_tag(ui: &mut Ui, et: f32, phase: &BootPhase, mark: &BootMark) {
    let ramp = ramp_in(et, 1.1, 1.2) * phase.fade;
    if ramp <= 0.0 {
        return;
    }
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    let size = 26.0 * sx.min(sy);
    let text = "HORIZON CLIENT";
    let tracking = 0.6 * size;
    let plain: f32 = text
        .chars()
        .map(|c| ui.measure(Face::Mono, size, &c.to_string()))
        .sum();
    let total = plain + tracking * (text.len() - 1) as f32;
    let cx = 890.0 * sx;
    let y = mark.cy - size * 1.45 / 2.0;
    let start = ui.batch.vertices.len();
    let draw_tracked = |ui: &mut Ui, dx: f32, dy: f32, color: u32| {
        let mut x = cx - total / 2.0 + dx;
        for c in text.chars() {
            let mut buf = [0u8; 4];
            let w = ui.measure(Face::Mono, size, &c.to_string());
            ui.text(Face::Mono, size, x, y + dy, color, c.encode_utf8(&mut buf));
            x += w + tracking;
        }
    };
    let glow = boot_rgba(0xA4B6FF, 0.16 * ramp);
    for (dx, dy) in [
        (-1.5 * sx, 0.0),
        (1.5 * sx, 0.0),
        (0.0, -1.5 * sy),
        (0.0, 1.5 * sy),
    ] {
        draw_tracked(ui, dx, dy, glow);
    }
    draw_tracked(ui, 0.0, 0.0, boot_rgba(0xC3CCF7, 0.95 * ramp));
    boot_xf(ui, start, (cx, mark.cy), phase.tag_scale, phase.tag_dy * sy);
}

/// The progress bar: a thin white border, an inner white fill at the
/// loader's fraction, a soft glow under it. During the finish sequence
/// it flashes and fades away (`phase`).
fn boot_bar(ui: &mut Ui, fraction: f32, et: f32, phase: &BootPhase) {
    if phase.bar_alpha <= 0.0 {
        return;
    }
    let a = phase.bar_alpha * ramp_in(et, 1.2, 1.0);
    if a <= 0.0 {
        return;
    }
    let sx = ui.w / 1920.0;
    let sy = ui.h / 1080.0;
    let m = sx.min(sy);
    let (x0, x1) = (0.05 * ui.w, 0.95 * ui.w);
    let h = 26.0 * sy;
    let y = ui.h - 88.0 * sy - h + phase.bar_drop * sy;
    let border = (2.0 * m).max(1.0);
    let pad = 3.0 * m;
    let white = boot_rgba(0xFFFFFF, a);
    if phase.bar_flash > 0.0 {
        ui.rounded(
            x0 - 10.0 * sx,
            y - 10.0 * sy,
            (x1 - x0) + 20.0 * sx,
            h + 20.0 * sy,
            8.0,
            boot_rgba(0xFFFFFF, 0.25 * phase.bar_flash * a),
        );
    }
    ui.rounded(
        x0 - 4.0 * sx,
        y - 4.0 * sy,
        (x1 - x0) + 8.0 * sx,
        h + 8.0 * sy,
        4.0,
        boot_rgba(0xFFFFFF, (0.10 + 0.5 * phase.bar_flash) * a),
    );
    ui.rounded(x0, y, x1 - x0, border, 0.0, white);
    ui.rounded(x0, y + h - border, x1 - x0, border, 0.0, white);
    ui.rounded(x0, y, border, h, 0.0, white);
    ui.rounded(x1 - border, y, border, h, 0.0, white);
    let fill_w = (x1 - x0 - 2.0 * pad).max(0.0) * fraction.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        ui.rounded(
            x0 + pad,
            y + pad,
            fill_w,
            (h - 2.0 * pad).max(0.0),
            0.0,
            white,
        );
    }
}

/// The boot screen while the menu is not ready yet, and again while the
/// finish sequence plays: the v2 ONYX design on pure black. `fraction`
/// is the bar's displayed fraction (the caller eases it toward the
/// loader's real fraction, so it never claims more than has loaded);
/// `elapsed` drives the ambient animation; `finish`, when set, is the
/// finish sequence's elapsed time, played over a `seq_total` timeline.
///
/// With `overlay` set the caller has already drawn the title into the
/// same batch: the black field then carries the sequence's own fade —
/// opaque until the fade window, thinning to nothing at its end — so
/// the sequence dissolves into the title beneath instead of covering
/// it with black. Once the sequence is done this draws nothing at all,
/// in either mode: no fill, no elements.
pub fn draw_boot(
    atlas: &HudAtlas,
    batch: &mut HudBatch,
    fraction: f32,
    elapsed: Duration,
    finish: Option<Duration>,
    seq_total: Duration,
    overlay: bool,
) {
    let phase = boot_phase(finish.map(|f| f.as_secs_f32()), seq_total.as_secs_f32());
    if phase.done {
        return;
    }
    // Pure black under everything: the frame's panorama (the title
    // artwork once the bundle is in) must not leak through. In overlay
    // mode the field itself fades with the sequence, revealing the
    // title the caller drew underneath.
    let fill_alpha = if overlay { phase.fade } else { 1.0 };
    batch.fill(
        atlas,
        0.0,
        0.0,
        batch.width,
        batch.height,
        boot_rgba(0x000000, fill_alpha),
    );
    let et = elapsed.as_secs_f32();
    let mut ui = Ui::new(atlas, batch, (0.0, 0.0));
    let mark = boot_mark(&ui);
    boot_halo(&mut ui, et, &phase);
    boot_stars(&mut ui, et, &phase);
    boot_diamond(&mut ui, et, &phase, &mark);
    boot_wordmark(&mut ui, et, &phase, &mark);
    boot_seam(&mut ui, et, &phase, &mark);
    boot_tag(&mut ui, et, &phase, &mark);
    boot_bar(&mut ui, fraction, et, &phase);
    ui.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_reload_regenerates_text_and_corners_at_the_same_address() {
        let atlas = HudAtlas::build(Path::new("/nonexistent/horizon-test-assets")).unwrap();
        reset_atlas();
        let draw = |cache: &mut Cache| {
            let glyph = cache.fonts.face(Face::Regular).glyph_id('A');
            assert!(
                cache
                    .glyph(&atlas, Face::Regular, glyph, 16.0)
                    .rect
                    .is_some()
            );
            assert!(cache.corner(&atlas, 8, false).is_some());
            std::mem::take(&mut cache.pending)
        };
        let before = with_cache(&atlas, draw);
        assert_eq!(before.len(), 2);
        assert!(with_cache(&atlas, draw).is_empty());
        reset_atlas();
        let after = with_cache(&atlas, draw);
        assert_eq!(after.len(), 2);
        for (a, b) in before.iter().zip(&after) {
            assert_eq!((a.x, a.y, a.width, a.height), (b.x, b.y, b.width, b.height));
            assert_eq!(a.rgba, b.rgba);
        }
        reset_atlas();
    }

    #[test]
    fn the_boot_atlas_holds_the_run_time_glyph_region_without_a_pack() {
        let atlas = HudAtlas::boot();
        assert_eq!((atlas.width, atlas.height), (1024, 1024));
        assert_eq!(
            atlas.sprites["horizon/ui-text"].texels((atlas.width, atlas.height)),
            (1024, 512)
        );
        assert_eq!(
            atlas.sprites["horizon/white"].texels((atlas.width, atlas.height)),
            (4, 4)
        );
    }

    #[test]
    fn the_boot_screen_draws_the_design_on_pure_black() {
        reset_atlas();
        let atlas = HudAtlas::boot();
        let mut batch = HudBatch::new([1280, 720]);
        draw_boot(
            &atlas,
            &mut batch,
            0.5,
            Duration::from_secs(2),
            None,
            BOOT_SEQ_TOTAL,
            false,
        );
        // The first quad is the pure black backdrop over the frame's panorama.
        let (w, h) = (batch.width, batch.height);
        let bg = &batch.vertices[..6];
        assert!(bg.iter().all(|v| v.color == 0xFF00_0000));
        assert!(bg.iter().all(|v| v.pos[0] == 0.0 || v.pos[0] == w));
        assert!(bg.iter().all(|v| v.pos[1] == 0.0 || v.pos[1] == h));
        assert!(
            !batch.patches.is_empty(),
            "the glyphs are queued for the atlas"
        );
        // The wordmark straddles the centre.
        let (min, max) = batch
            .vertices
            .iter()
            .map(|v| v.pos[0])
            .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
        assert!(
            min < w / 2.0 && max > w / 2.0,
            "the design spans the centre"
        );
        // Where the old console block lived (the bottom rows) there is
        // now only the odd star: a handful of small quads, never the
        // console's rows of glyph quads running up from the bottom edge.
        let bottom = batch
            .vertices
            .iter()
            .filter(|v| v.pos[1] > h * 0.97)
            .count();
        assert!(
            bottom < 300,
            "no console text at the bottom: {bottom} vertices"
        );
        // At full fraction the bar's fill reaches toward the right margin.
        let mut batch = HudBatch::new([1280, 720]);
        draw_boot(
            &atlas,
            &mut batch,
            1.0,
            Duration::from_secs(2),
            None,
            BOOT_SEQ_TOTAL,
            false,
        );
        let bar_max_x = batch
            .vertices
            .iter()
            .filter(|v| v.pos[1] > h * 0.85)
            .map(|v| v.pos[0])
            .fold(f32::MIN, f32::max);
        assert!(
            bar_max_x > w * 0.9,
            "the full bar reaches the right margin: {bar_max_x} of {w}"
        );
        reset_atlas();
    }

    #[test]
    fn the_finish_sequence_keeps_its_order() {
        // Loading: the neutral phase.
        let p = boot_phase(None, BOOT_SEQ_TOTAL.as_secs_f32());
        assert_eq!(p.bar_alpha, 1.0);
        assert_eq!(p.logo_scale, 1.0);
        assert!(p.ghosts.iter().all(|g| g.0 == 0.0));
        // The bar holds and flashes first.
        let p = boot_phase(Some(0.15), BOOT_SEQ_TOTAL.as_secs_f32());
        assert_eq!(p.bar_alpha, 1.0);
        assert!((p.bar_flash - 1.0).abs() < 1e-6);
        // It is fully gone before the bounce begins, and stays gone.
        for t in [BOOT_BAR_END, 0.85, 0.90, BOOT_BOUNCE_AT - 0.01] {
            let p = boot_phase(Some(t), BOOT_SEQ_TOTAL.as_secs_f32());
            assert_eq!(p.bar_alpha, 0.0, "bar gone at {t}");
            assert_eq!(p.logo_scale, 1.0, "no bounce yet at {t}");
            assert!(p.ghosts.iter().all(|g| g.0 == 0.0), "no ghosts at {t}");
        }
        // The bounce overshoots on the design's keyframe (28% of 1.15s).
        let p = boot_phase(
            Some(BOOT_BOUNCE_AT + 0.28 * BOOT_BOUNCE_LEN),
            BOOT_SEQ_TOTAL.as_secs_f32(),
        );
        assert!(
            (p.logo_scale - 1.065).abs() < 0.01,
            "logo overshoot: {}",
            p.logo_scale
        );
        assert!(p.gem_scale > 1.0 && p.gem_scale < p.logo_scale);
        // Ghosts exist only inside the bounce window.
        let p = boot_phase(Some(BOOT_BOUNCE_AT + 0.25), BOOT_SEQ_TOTAL.as_secs_f32());
        assert!(p.ghosts[0].0 > 0.0 && p.ghosts[0].1 > 1.0);
        let p = boot_phase(Some(3.0), BOOT_SEQ_TOTAL.as_secs_f32());
        assert!(
            p.ghosts.iter().all(|g| g.0 == 0.0),
            "ghosts spent by the fade"
        );
        // The fade runs after the bounce, the stars last, and the
        // sequence ends on black within its total length.
        assert_eq!(
            boot_phase(Some(2.0), BOOT_SEQ_TOTAL.as_secs_f32()).fade,
            1.0
        );
        let p = boot_phase(Some(3.0), BOOT_SEQ_TOTAL.as_secs_f32());
        assert!(p.fade < 0.2 && p.star_fade > p.fade, "stars fade last");
        let p = boot_phase(
            Some(BOOT_SEQ_TOTAL.as_secs_f32()),
            BOOT_SEQ_TOTAL.as_secs_f32(),
        );
        assert!(p.done);
        assert_eq!(p.star_fade, 0.0);
        assert_eq!(p.fade, 0.0);
    }

    #[test]
    fn the_finish_sequence_keeps_its_order_at_any_total() {
        // The timeline scales proportionally: whatever the adaptive
        // total, the bar is gone before the bounce, the ghosts live
        // inside the bounce, the fade runs after it and the stars end
        // last, exactly at the total.
        for total in [1.8f32, 2.5, 3.6] {
            let s = total / BOOT_SEQ_TOTAL.as_secs_f32();
            // Bar fully gone at its key, and nothing has bounced yet.
            for t in [BOOT_BAR_END * s, BOOT_BOUNCE_AT * s - 0.01 * s] {
                let p = boot_phase(Some(t), total);
                assert_eq!(p.bar_alpha, 0.0, "bar gone at {t} of {total}");
                assert_eq!(p.logo_scale, 1.0, "no bounce yet at {t} of {total}");
                assert!(
                    p.ghosts.iter().all(|g| g.0 == 0.0),
                    "no ghosts at {t} of {total}"
                );
            }
            // The bounce overshoots on the same keyframe shape.
            let p = boot_phase(Some((BOOT_BOUNCE_AT + 0.28 * BOOT_BOUNCE_LEN) * s), total);
            assert!(
                (p.logo_scale - 1.065).abs() < 0.01,
                "logo overshoot at total {total}: {}",
                p.logo_scale
            );
            // Ghosts exist only inside the bounce window.
            let p = boot_phase(Some((BOOT_BOUNCE_AT + 0.25) * s), total);
            assert!(p.ghosts[0].0 > 0.0 && p.ghosts[0].1 > 1.0);
            // No fade before its key; mid-fade the stars linger longest.
            assert_eq!(boot_phase(Some(2.0 * s), total).fade, 1.0);
            let p = boot_phase(Some(2.75 * s), total);
            assert!(
                p.fade < 0.2 && p.star_fade > p.fade,
                "stars fade last at total {total}"
            );
            assert!(p.ghosts.iter().all(|g| g.0 == 0.0), "ghosts spent");
            // The sequence ends exactly at its total, not the reference.
            assert!(!boot_phase(Some(total - 0.01), total).done);
            let p = boot_phase(Some(total), total);
            assert!(p.done, "done at {total}");
            assert_eq!(p.star_fade, 0.0);
            assert_eq!(p.fade, 0.0);
        }
    }

    #[test]
    fn the_sequence_length_follows_the_load_and_never_exceeds_the_design() {
        // A quarter of the load's time, floored so the bounce stays
        // readable and capped at the design's own length.
        assert_eq!(boot_seq_duration(Duration::from_secs(0)), BOOT_SEQ_MIN);
        assert_eq!(boot_seq_duration(Duration::from_secs(4)), BOOT_SEQ_MIN);
        assert_eq!(boot_seq_duration(Duration::from_millis(7200)), BOOT_SEQ_MIN);
        assert_eq!(
            boot_seq_duration(Duration::from_secs(10)),
            Duration::from_millis(2500)
        );
        assert_eq!(
            boot_seq_duration(Duration::from_millis(14_400)),
            BOOT_SEQ_TOTAL
        );
        assert_eq!(boot_seq_duration(Duration::from_secs(600)), BOOT_SEQ_TOTAL);
    }

    #[test]
    fn the_overlay_fill_rides_the_sequence_fade_and_done_draws_nothing() {
        reset_atlas();
        let atlas = HudAtlas::boot();
        // Mid-fade the overlay's black field is exactly the sequence's
        // fade, not an opaque cover: the title underneath shows through
        // by precisely that much.
        let t = 2.75;
        let phase = boot_phase(Some(t), BOOT_SEQ_TOTAL.as_secs_f32());
        assert!(phase.fade > 0.0 && phase.fade < 1.0);
        let mut batch = HudBatch::new([1280, 720]);
        draw_boot(
            &atlas,
            &mut batch,
            1.0,
            Duration::from_secs(2),
            Some(Duration::from_secs_f32(t)),
            BOOT_SEQ_TOTAL,
            true,
        );
        let bg = &batch.vertices[..6];
        assert!(
            bg.iter()
                .all(|v| v.color == boot_rgba(0x000000, phase.fade))
        );
        // Before the fade window the field is still fully opaque.
        let mut batch = HudBatch::new([1280, 720]);
        draw_boot(
            &atlas,
            &mut batch,
            1.0,
            Duration::from_secs(2),
            Some(Duration::from_secs(1)),
            BOOT_SEQ_TOTAL,
            true,
        );
        assert!(batch.vertices[..6].iter().all(|v| v.color == 0xFF00_0000));
        // At the end of the sequence the boot screen contributes
        // nothing at all — no fill lands on the title's frame.
        let mut batch = HudBatch::new([1280, 720]);
        draw_boot(
            &atlas,
            &mut batch,
            1.0,
            Duration::from_secs(2),
            Some(BOOT_SEQ_TOTAL),
            BOOT_SEQ_TOTAL,
            true,
        );
        assert!(batch.vertices.is_empty(), "done draws nothing");
        reset_atlas();
    }
}
