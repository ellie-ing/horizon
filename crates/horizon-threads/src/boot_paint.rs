//! The game window's boot screen before wgpu's first frame: the renderer's
//! device and pipelines take seconds to build, and winit's window sits white
//! until a frame is presented. The game's boot design (`ui_theme::draw_boot`:
//! pure black, the white ONYX wordmark, the loader's progress bar under it,
//! the console's last lines at the bottom left, wrapping word-wise), painted
//! with GDI and replaced by the wgpu frames when the renderer starts drawing.

use std::sync::Once;
use std::time::Duration;

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_CALCRECT, DT_CENTER,
    DT_LEFT, DT_NOCLIP, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject,
    DrawTextW, FF_DONTCARE, FillRect, GetDC, OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY, SelectObject,
    SetBkMode, SetTextCharacterExtra, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

/// The wordmark on the pure black.
const WORDMARK: COLORREF = 0x00ff_ffff;
/// The console's lines, `0x9BA1A6` in GDI's `0x00bbggrr`.
const CONSOLE: COLORREF = 0x00a6_a19b;
// ONYX fork: the rest of the boot screen's palette, mirroring
// `ui_theme::draw_boot`'s — GDI colours are opaque, so the wgpu screen's
// translucent ice glow is a dark ice blue here and the translucent border
// its blend over the black.
/// The sub-line, `0x9FB4D8`.
const SUB: COLORREF = 0x00d8_b49f;
/// The bar's fill, `0xBCD6FF`.
const ICE: COLORREF = 0x00ff_d6bc;
/// The bar's track, near the black.
const TRACK: COLORREF = 0x0019_1310;
/// The bar's thin border.
const BORDER: COLORREF = 0x008c_6c5a;
/// The meta row under the bar, dimmed white.
const META: COLORREF = 0x00a0_9b96;

/// The wordmark glow's colour, pulsing gently with the elapsed time: the
/// ice blue at a brightness GDI's opaque text can manage on the black.
fn glow(elapsed: Duration) -> COLORREF {
    let pulse = 0.5 + 0.5 * (elapsed.as_secs_f32() * 2.2).sin();
    let scale = 0.7 + 0.5 * pulse;
    let ch = |base: f32| (base * scale).round().clamp(0.0, 255.0) as u32;
    // `0x38506E` at full brightness, in `0x00bbggrr`.
    ch(0x6e as f32) << 16 | ch(0x50 as f32) << 8 | ch(0x38 as f32)
}

/// GDI's font table is global to the process: the bundled faces register once
/// across every paint.
static FACES: Once = Once::new();

/// Paints the client area of the window at `hwnd` black, the wordmark centred,
/// the loader's progress under it and the console's last lines at the bottom
/// left — the same look as the game's `ui_theme::draw_boot`, which takes over
/// when the renderer presents its first frame. `fraction` is the bar's
/// displayed fraction (the caller eases it toward the loader's stage
/// fraction), `stage` the loader's current stage line and `elapsed` drives
/// the glow's gentle pulse.
///
/// `semibold` and `mono` are the faces' TTF bytes (`Geist SemiBold`,
/// `Geist Mono`), which must outlive the process: `AddFontMemResourceEx`
/// keeps referencing them. Repainting is safe and cheap (a background thread
/// may call this as the console's lines arrive).
pub fn paint(
    hwnd: isize,
    semibold: &[u8],
    mono: &[u8],
    logs: &[String],
    fraction: f32,
    stage: &str,
    elapsed: Duration,
) {
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
        // The type scale of the game's `Ui::new`: physical pixels per dp,
        // then the sizes in dp (the wordmark at `DISPLAY`, the console at
        // `MICRO` with 1.5 line heights).
        let density = (w / 1040.0).min(h / 700.0).clamp(1.0, 4.0);
        let size = ((w / density / 16.0).clamp(40.0, 96.0) * density).round();
        let line = 11.0 * density * 1.5;
        let pad = 16.0 * density;
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
        // A solid fill between two corners, the bar's whole vocabulary.
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
        // The centred column — wordmark, sub-line, bar and meta row — laid
        // out with the same formulas as the game's `boot_layout`, in
        // physical pixels: every piece hangs off the wordmark's edges.
        let small = (size * 0.25).clamp(11.0 * density, 18.0 * density);
        let bar_h = (6.0 * density).round();
        let meta_px = 11.0 * density;
        let block = size * 1.45 + small * 0.6 + small * 1.5;
        let total = block + size * 0.55 + bar_h + 9.0 * density + meta_px * 1.5;
        let top = (h - total) / 2.0;
        let sub_top = top + size * 1.45 + small * 0.6;
        let bar_y = sub_top + small * 1.5 + size * 0.55;
        let meta_y = bar_y + bar_h + 9.0 * density;
        let mark = face(size as i32, 600, "Geist SemiBold");
        let old_mark = SelectObject(hdc, mark);
        SetBkMode(hdc, TRANSPARENT as i32);
        let text: Vec<u16> = "ONYX".encode_utf16().collect();
        // The wordmark's width, measured without drawing: the column's
        // left and right edges.
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
        let mark_w = (measure.right - measure.left) as f32;
        let (left, right) = ((w - mark_w) / 2.0, (w + mark_w) / 2.0);
        let mark_rect = |dx: f32, dy: f32| RECT {
            left: dx as i32,
            top: (top + dy) as i32,
            right: (w + dx) as i32,
            bottom: (top + dy + size * 1.45) as i32,
        };
        // ONYX fork: the glow is the wordmark drawn again at small offsets
        // in dark ice blue, as in `ui_theme::wordmark` — GDI text has no
        // alpha, so the colour itself carries the translucency.
        SetTextColor(hdc, glow(elapsed));
        let d = (size * 0.022).max(1.5);
        for (dx, dy) in [(-d, 0.0), (d, 0.0), (0.0, d)] {
            let mut rect = mark_rect(dx, dy);
            DrawTextW(
                hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut rect,
                DT_CENTER | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
            );
        }
        SetTextColor(hdc, WORDMARK);
        let mut rect = mark_rect(0.0, 0.0);
        DrawTextW(
            hdc,
            text.as_ptr(),
            text.len() as i32,
            &mut rect,
            DT_CENTER | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
        );
        // The sub-line, letterspaced mono ending at the wordmark's right
        // edge (GDI's character extra is the letterspacing).
        let tagline = face(small as i32, 400, "Geist Mono");
        let old_tagline = SelectObject(hdc, tagline);
        SetTextColor(hdc, SUB);
        SetTextCharacterExtra(hdc, (small * 0.16).round() as i32);
        let tagline_text: Vec<u16> = "HORIZON CLIENT".encode_utf16().collect();
        let mut tagline_rect = RECT {
            left: left as i32,
            top: sub_top as i32,
            right: right as i32,
            bottom: (sub_top + small * 1.5) as i32,
        };
        DrawTextW(
            hdc,
            tagline_text.as_ptr(),
            tagline_text.len() as i32,
            &mut tagline_rect,
            DT_RIGHT | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
        );
        SetTextCharacterExtra(hdc, 0);
        // The bar across the wordmark's width: track, the ice fill running
        // to white at its leading edge, the thin border over it all.
        let fraction = fraction.clamp(0.0, 1.0);
        let fill_w = (mark_w - 2.0).max(0.0) * fraction;
        if fill_w > 0.0 {
            // The fill's soft leading edge, just past the fill.
            fill(
                left - 0.5,
                bar_y - 1.5,
                left + 1.0 + fill_w + 1.5,
                bar_y + bar_h + 1.5,
                glow(elapsed),
            );
        }
        fill(left, bar_y, right, bar_y + bar_h, TRACK);
        if fill_w > 0.0 {
            fill(left + 1.0, bar_y + 1.0, left + 1.0 + fill_w, bar_y + bar_h - 1.0, ICE);
            let seg = fill_w.min(16.0 * density);
            fill(
                left + 1.0 + fill_w - seg,
                bar_y + 1.0,
                left + 1.0 + fill_w,
                bar_y + bar_h - 1.0,
                WORDMARK,
            );
        }
        fill(left, bar_y, right, bar_y + 1.0, BORDER);
        fill(left, bar_y + bar_h - 1.0, right, bar_y + bar_h, BORDER);
        fill(left, bar_y, left + 1.0, bar_y + bar_h, BORDER);
        fill(right - 1.0, bar_y, right, bar_y + bar_h, BORDER);
        // The meta row: the stage on the left, the percentage on the right.
        let meta = face(meta_px as i32, 400, "Geist Mono");
        let old_meta = SelectObject(hdc, meta);
        SetTextColor(hdc, META);
        let stage_text: Vec<u16> = stage.to_uppercase().encode_utf16().collect();
        let mut meta_rect = RECT {
            left: left as i32,
            top: meta_y as i32,
            right: right as i32,
            bottom: (meta_y + meta_px * 1.5) as i32,
        };
        DrawTextW(
            hdc,
            stage_text.as_ptr(),
            stage_text.len() as i32,
            &mut meta_rect,
            DT_LEFT | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
        );
        let pct = format!("{:.0}%", fraction * 100.0);
        let pct_text: Vec<u16> = pct.encode_utf16().collect();
        DrawTextW(
            hdc,
            pct_text.as_ptr(),
            pct_text.len() as i32,
            &mut meta_rect,
            DT_RIGHT | DT_SINGLELINE | DT_NOCLIP | DT_VCENTER,
        );
        // The console's last lines at the bottom left, the newest at the
        // bottom: long lines wrap word-wise and the block grows upwards, as
        // the game's `draw_boot_log` draws it (capped at 24 sub-lines).
        let console = face(line as i32, 400, "Geist Mono");
        let old_console = SelectObject(hdc, console);
        SetTextColor(hdc, CONSOLE);
        let max_rows = (((h - pad) / line) as usize).clamp(1, 24);
        let mut y = h - pad;
        let mut drawn = 0;
        'lines: for text in logs.iter().rev() {
            if text.is_empty() {
                continue;
            }
            let wide: Vec<u16> = text.encode_utf16().collect();
            // The wrapped height, measured without drawing (DT_CALCRECT);
            // every row is the face's cell height, `line`.
            let mut rect = RECT {
                left: pad as i32,
                top: 0,
                right: (pad + w / 2.0) as i32,
                bottom: 0,
            };
            DrawTextW(
                hdc,
                wide.as_ptr(),
                wide.len() as i32,
                &mut rect,
                DT_LEFT | DT_WORDBREAK | DT_CALCRECT,
            );
            let height = rect.bottom.max(line as i32);
            let rows = ((height as f32 / line).ceil() as usize).max(1);
            // The block may only grow past the top before anything is drawn
            // (a newest line wider than the whole screen on a tiny window);
            // once something is there, older lines stop.
            if drawn + rows > max_rows && drawn > 0 {
                break 'lines;
            }
            rect.top = ((y - height as f32).max(0.0)) as i32;
            rect.bottom = y as i32;
            DrawTextW(
                hdc,
                wide.as_ptr(),
                wide.len() as i32,
                &mut rect,
                DT_LEFT | DT_WORDBREAK,
            );
            y -= height as f32;
            drawn += rows;
        }
        SelectObject(hdc, old_mark);
        SelectObject(hdc, old_tagline);
        SelectObject(hdc, old_meta);
        SelectObject(hdc, old_console);
        let _ = DeleteObject(mark);
        let _ = DeleteObject(tagline);
        let _ = DeleteObject(meta);
        let _ = DeleteObject(console);
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
