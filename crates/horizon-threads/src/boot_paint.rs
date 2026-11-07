//! The game window's boot screen before wgpu's first frame: the renderer's
//! device and pipelines take seconds to build, and winit's window sits white
//! until a frame is presented. The game's boot design (`ui_theme::draw_boot`:
//! pure black, a starfield and diamond outline, the ONYX wordmark cut by a
//! slit of light with HORIZON CLIENT inside it, a white progress bar near
//! the bottom), painted with GDI in the same layout proportions and replaced
//! by the wgpu frames when the renderer starts drawing. The painted version
//! is static where the wgpu one animates (no comets or sheen), and carries
//! no console text, like the design.

use std::sync::Once;
use std::time::Duration;

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DT_CALCRECT,
    DT_CENTER, DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    FF_DONTCARE, FillRect, GetDC, IntersectClipRect, LineTo, MoveToEx, PS_SOLID, ReleaseDC,
    RestoreDC, SRCCOPY, SaveDC, SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

/// GDI's font table is global to the process: the bundled faces register once
/// across every paint.
static FACES: Once = Once::new();

/// A `0x00bbggrr` colour from rgb components scaled by `a` (GDI colours are
/// opaque, so translucency is carried by darkening toward the black field).
fn shade(r: f32, g: f32, b: f32, a: f32) -> COLORREF {
    let ch = |v: f32| (v * a).round().clamp(0.0, 255.0) as u32;
    ch(r) | ch(g) << 8 | ch(b) << 16
}

/// The deterministic star hash, shared with `ui_theme`'s starfield so the
/// painted stars sit where the wgpu ones will.
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

/// The design's twinkle curve, as in `ui_theme`.
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
    let u = u.rem_euclid(1.0);
    if u <= KEYS[0].0 {
        return KEYS[0].1;
    }
    for w in KEYS.windows(2) {
        if u <= w[1].0 {
            return w[0].1 + (w[1].1 - w[0].1) * ((u - w[0].0) / (w[1].0 - w[0].0));
        }
    }
    KEYS[KEYS.len() - 1].1
}

/// The wordmark's gradient colour at height fraction `f`: the design's
/// soft stops (`ui_theme::boot_onyx_colour`, without the stone banding).
fn onyx_shade(f: f32) -> COLORREF {
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
    shade(rgb[0], rgb[1], rgb[2], 1.0)
}

/// Paints the client area of the window at `hwnd` with the boot design:
/// black, the starfield and diamond, the wordmark in the slit arrangement
/// and the loader's progress bar near the bottom — the same proportions
/// as the game's `ui_theme::draw_boot`, which takes over when the
/// renderer presents its first frame. `fraction` is the bar's displayed
/// fraction (the caller eases it toward the loader's real fraction) and
/// `elapsed` drives the stars' twinkle.
///
/// `semibold` and `mono` are the faces' TTF bytes (`Geist SemiBold`,
/// `Geist Mono`), which must outlive the process: `AddFontMemResourceEx`
/// keeps referencing them. Repainting is safe and cheap (a background
/// thread repaints at a few Hz while the renderer builds).
pub fn paint(hwnd: isize, semibold: &[u8], mono: &[u8], fraction: f32, elapsed: Duration) {
    let hwnd = hwnd as HWND;
    FACES.call_once(|| {
        for font in [semibold, mono] {
            let count = 0u32;
            unsafe {
                AddFontMemResourceEx(
                    font.as_ptr().cast(),
                    font.len() as u32,
                    std::ptr::null(),
                    &count,
                );
            }
        }
    });
    unsafe {
        let window_dc = GetDC(hwnd);
        if window_dc.is_null() {
            return;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetClientRect(hwnd, &mut rect);
        // Draw into a memory bitmap and copy it to the window in one blit:
        // clearing and drawing straight to the window's DC shows the blank
        // frame in between, so the screen flickers at every repaint.
        let mem = CreateCompatibleDC(window_dc);
        let bitmap = CreateCompatibleBitmap(window_dc, rect.right, rect.bottom);
        if mem.is_null() || bitmap.is_null() {
            if !bitmap.is_null() {
                let _ = DeleteObject(bitmap);
            }
            if !mem.is_null() {
                let _ = DeleteDC(mem);
            }
            let _ = ReleaseDC(hwnd, window_dc);
            return;
        }
        let old_bitmap = SelectObject(mem, bitmap);
        let hdc = mem;
        let (w, h) = (
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
        );
        // The pure black clear.
        let brush = CreateSolidBrush(0);
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(brush);
        if w <= 0.0 || h <= 0.0 {
            BitBlt(window_dc, 0, 0, rect.right, rect.bottom, hdc, 0, 0, SRCCOPY);
            SelectObject(hdc, old_bitmap);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(hdc);
            let _ = ReleaseDC(hwnd, window_dc);
            return;
        }
        // The design space mapped onto the window, as in `draw_boot`.
        let (dx, dy) = (w / 1920.0, h / 1080.0);
        let m = dx.min(dy);
        let et = elapsed.as_secs_f32();
        let face = |px: i32, weight: i32, name: &str| {
            let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            CreateFontW(
                -px,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                FF_DONTCARE as u32,
                name.as_ptr(),
            )
        };
        // A solid fill between two corners.
        let fill = |x0: f32, y0: f32, x1: f32, y1: f32, color: COLORREF| {
            if x1 > x0 && y1 > y0 {
                let rect = RECT {
                    left: x0 as i32,
                    top: y0 as i32,
                    right: x1 as i32,
                    bottom: y1 as i32,
                };
                unsafe {
                    let brush = CreateSolidBrush(color);
                    FillRect(hdc, &rect, brush);
                    let _ = DeleteObject(brush);
                }
            }
        };
        // The wordmark's font size: measured once at 100px so ONYX spans
        // the design's ~61.5% of the width at any window size.
        let probe = face(100, 600, "Geist SemiBold");
        let old_font = SelectObject(hdc, probe);
        SetBkMode(hdc, TRANSPARENT as i32);
        let text: Vec<u16> = "ONYX".encode_utf16().collect();
        let mut measure = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        DrawTextW(
            hdc,
            text.as_ptr(),
            text.len() as i32,
            &mut measure,
            DT_SINGLELINE | DT_CALCRECT,
        );
        let at_100 = (measure.right - measure.left).max(1) as f32;
        SelectObject(hdc, old_font);
        let _ = DeleteObject(probe);
        let font_px = 0.615 * w * 100.0 / at_100;
        let mark = face(font_px.round() as i32, 600, "Geist SemiBold");
        let old_font = SelectObject(hdc, mark);
        let cx = w / 2.0 - 8.0 * dx;
        let cy = 480.0 * dy;
        // The slit: the wordmark's middle band, cut from every layer.
        let slit = (cy - font_px * 0.0912, cy + font_px * 0.0912);

        // The starfield: the same six groups as the wgpu field, as small
        // grey squares (their twinkle carried by the shade).
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
                let x = boot_unit(seed * 4) * w;
                let y = boot_unit(seed * 4 + 1) * h;
                let base = ALPHAS[(boot_hash(seed * 4 + 2) % 6) as usize];
                let u = (et + off) / period + boot_unit(seed * 4 + 3);
                let a = base * boot_twinkle(u);
                let s = (size * m).max(1.0);
                fill(
                    x - s / 2.0,
                    y - s / 2.0,
                    x + s / 2.0,
                    y + s / 2.0,
                    shade(255.0, 255.0, 255.0, a),
                );
            }
        }
        for _ in 0..16 {
            seed += 1;
        }

        // The diamond outlines, both sets, with the slit cut out of the
        // segments that cross it.
        let p = |x: f32, y: f32| (x * dx, y * dy);
        let seg = |a: (f32, f32), b: (f32, f32)| {
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
            for win in cuts.windows(2) {
                let pa = (a.0 + (b.0 - a.0) * win[0], a.1 + (b.1 - a.1) * win[0]);
                let pb = (a.0 + (b.0 - a.0) * win[1], a.1 + (b.1 - a.1) * win[1]);
                let mid = (pa.1 + pb.1) / 2.0;
                if mid > slit.0 && mid < slit.1 {
                    continue;
                }
                unsafe {
                    MoveToEx(hdc, pa.0 as i32, pa.1 as i32, std::ptr::null_mut());
                    LineTo(hdc, pb.0 as i32, pb.1 as i32);
                }
            }
        };
        let poly = |pts: &[(f32, f32)], closed: bool| {
            for pair in pts.windows(2) {
                seg(pair[0], pair[1]);
            }
            if closed && pts.len() > 2 {
                seg(*pts.last().unwrap(), pts[0]);
            }
        };
        let lines = |pts_list: &[&[(f32, f32)]], width: f32, color: COLORREF| {
            let pen = CreatePen(PS_SOLID, (width as i32).max(1), color);
            let old_pen = SelectObject(hdc, pen);
            for pts in pts_list {
                poly(pts, true);
            }
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen);
        };
        let const_outer = [
            p(960.0, 118.0),
            p(645.0, 391.0),
            p(960.0, 872.0),
            p(1275.0, 391.0),
        ];
        let const_inner = [
            p(960.0, 118.0),
            p(822.0, 391.0),
            p(960.0, 872.0),
            p(1098.0, 391.0),
        ];
        lines(
            &[&const_outer, &const_inner],
            1.4 * m,
            shade(255.0, 255.0, 255.0, 0.28),
        );
        {
            let pen = CreatePen(PS_SOLID, (1.4 * m) as i32, shade(255.0, 255.0, 255.0, 0.28));
            let old_pen = SelectObject(hdc, pen);
            seg(p(645.0, 391.0), p(1275.0, 391.0));
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen);
        }
        let gem_outer = [
            p(960.0, 139.0),
            p(634.0, 400.0),
            p(960.0, 899.0),
            p(1286.0, 400.0),
        ];
        let gem_inner = [
            p(960.0, 139.0),
            p(818.0, 400.0),
            p(960.0, 899.0),
            p(1102.0, 400.0),
        ];
        lines(
            &[&gem_outer, &gem_inner],
            1.6 * m,
            shade(201.0, 209.0, 245.0, 0.5),
        );
        {
            let pen = CreatePen(PS_SOLID, (1.6 * m) as i32, shade(201.0, 209.0, 245.0, 0.5));
            let old_pen = SelectObject(hdc, pen);
            seg(p(634.0, 400.0), p(1286.0, 400.0));
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen);
        }
        // The nodes: a dim halo square and a bright dot at each vertex.
        for (x, y) in [
            (960.0, 118.0),
            (645.0, 391.0),
            (1275.0, 391.0),
            (960.0, 872.0),
            (960.0, 139.0),
            (634.0, 400.0),
            (1286.0, 400.0),
            (960.0, 899.0),
        ] {
            let (x, y) = p(x, y);
            let hr = 11.0 * m;
            fill(
                x - hr,
                y - hr,
                x + hr,
                y + hr,
                shade(255.0, 255.0, 255.0, 0.10),
            );
            let r = 3.4 * m;
            fill(x - r, y - r, x + r, y + r, shade(255.0, 255.0, 255.0, 0.85));
        }

        // The wordmark: a dim periwinkle bloom offset, then the gradient
        // in six horizontal bands — every draw clipped out of the slit.
        let mark_rect = |dxo: f32, dyo: f32| RECT {
            left: (cx - w / 2.0 + dxo) as i32,
            top: (cy - font_px * 0.725 + dyo) as i32,
            right: (cx + w / 2.0 + dxo) as i32,
            bottom: (cy + font_px * 0.725 + dyo) as i32,
        };
        let draw_mark = |dxo: f32, dyo: f32, y0: f32, y1: f32| {
            // Clip to the band [y0, y1) minus the slit: up to two clips.
            for (c0, c1) in [(y0, slit.0), (slit.1, y1)] {
                if c1 <= c0 {
                    continue;
                }
                unsafe {
                    let saved = SaveDC(hdc);
                    IntersectClipRect(hdc, 0, c0 as i32, w as i32, c1 as i32);
                    let mut r = mark_rect(dxo, dyo);
                    DrawTextW(
                        hdc,
                        text.as_ptr(),
                        text.len() as i32,
                        &mut r,
                        DT_CENTER | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
                    );
                    RestoreDC(hdc, saved);
                }
            }
        };
        SetTextColor(hdc, shade(213.0, 220.0, 255.0, 0.30));
        draw_mark(2.2 * dx, 2.0 * dy, 0.0, h);
        const BANDS: usize = 6;
        let (top, bot) = (cy - font_px / 2.0, cy + font_px / 2.0);
        for i in 0..BANDS {
            let f0 = i as f32 / BANDS as f32;
            let f1 = (i + 1) as f32 / BANDS as f32;
            SetTextColor(hdc, onyx_shade((f0 + f1) / 2.0));
            draw_mark(0.0, 0.0, top + (bot - top) * f0, top + (bot - top) * f1);
        }
        SelectObject(hdc, old_font);
        let _ = DeleteObject(mark);

        // The seam of light in the slit.
        let pulse = 0.70 + 0.25 * (std::f32::consts::TAU * et / 5.0).sin();
        let (sx0, sx1) = (300.0 * dx, 1620.0 * dx);
        let thick = (2.0 * dy).max(1.0);
        for i in 0..16 {
            let (f0, f1) = (i as f32 / 16.0, (i + 1) as f32 / 16.0);
            let fm = (f0 + f1) / 2.0;
            let prof = if fm < 0.3 {
                0.4 * (fm / 0.3)
            } else if fm < 0.5 {
                0.4 + 0.25 * ((fm - 0.3) / 0.2)
            } else if fm < 0.7 {
                0.65 - 0.25 * ((fm - 0.5) / 0.2)
            } else {
                0.4 * (1.0 - (fm - 0.7) / 0.3)
            };
            let t = 1.0 - (2.0 * (fm - 0.5)).abs();
            let mix = |e: f32, c: f32| e + (c - e) * t;
            fill(
                sx0 + (sx1 - sx0) * f0,
                cy - thick / 2.0,
                sx0 + (sx1 - sx0) * f1 + 1.0,
                cy + thick / 2.0,
                shade(mix(178.0, 235.0), mix(193.0, 240.0), 255.0, prof * pulse),
            );
        }

        // HORIZON CLIENT in the slit, letterspaced, centred ~70 design px
        // left of centre, with a dark periwinkle glow pass under it.
        let tag_px = (26.0 * m).round().max(8.0);
        let tag = face(tag_px as i32, 400, "Geist Mono");
        let old_tag = SelectObject(hdc, tag);
        SetTextCharacterExtra(hdc, (0.6 * tag_px).round() as i32);
        let tag_text: Vec<u16> = "HORIZON CLIENT".encode_utf16().collect();
        let tag_cx = 890.0 * dx;
        let tag_rect = |dxo: f32, dyo: f32| RECT {
            left: (tag_cx - 600.0 + dxo) as i32,
            top: (cy - tag_px + dyo) as i32,
            right: (tag_cx + 600.0 + dxo) as i32,
            bottom: (cy + tag_px + dyo) as i32,
        };
        SetTextColor(hdc, shade(164.0, 182.0, 255.0, 0.35));
        for (dxo, dyo) in [(-1.5 * dx, 0.0), (1.5 * dx, 0.0), (0.0, 1.5 * dy)] {
            let mut r = tag_rect(dxo, dyo);
            DrawTextW(
                hdc,
                tag_text.as_ptr(),
                tag_text.len() as i32,
                &mut r,
                DT_CENTER | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
            );
        }
        SetTextColor(hdc, shade(195.0, 204.0, 247.0, 0.85));
        let mut r = tag_rect(0.0, 0.0);
        DrawTextW(
            hdc,
            tag_text.as_ptr(),
            tag_text.len() as i32,
            &mut r,
            DT_CENTER | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
        );
        SetTextCharacterExtra(hdc, 0);
        SelectObject(hdc, old_tag);
        let _ = DeleteObject(tag);

        // The bar near the bottom: white border, white fill at the
        // loader's fraction.
        let fraction = fraction.clamp(0.0, 1.0);
        let (bx0, bx1) = (0.05 * w, 0.95 * w);
        let bh = 26.0 * dy;
        let by = h - 88.0 * dy - bh;
        let bw = (2.0 * m).max(1.0);
        let pad = 3.0 * m;
        let white = shade(255.0, 255.0, 255.0, 0.95);
        fill(bx0, by, bx1, by + bw, white);
        fill(bx0, by + bh - bw, bx1, by + bh, white);
        fill(bx0, by, bx0 + bw, by + bh, white);
        fill(bx1 - bw, by, bx1, by + bh, white);
        let fill_w = (bx1 - bx0 - 2.0 * pad).max(0.0) * fraction;
        if fill_w > 0.0 {
            fill(
                bx0 + pad,
                by + pad,
                bx0 + pad + fill_w,
                by + bh - pad,
                white,
            );
        }

        // The finished frame reaches the window as one copy.
        BitBlt(
            window_dc,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            hdc,
            rect.left,
            rect.top,
            SRCCOPY,
        );
        SelectObject(hdc, old_bitmap);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(hdc);
        let _ = ReleaseDC(hwnd, window_dc);
    }
}
