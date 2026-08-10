//! Paint a vt100 screen into an egui rect. Adapted from tmuxmux's
//! `render_terminal`, minus mouse-selection (touch selection is future work).

use egui::{Color32, FontId, Pos2, Rect, Ui, Vec2};

use crate::colors::{convert_bg, convert_fg, DEFAULT_BG, DEFAULT_FG};

/// Grid geometry chosen for a given rect + font, so the caller can resize the
/// remote PTY to match.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
}

/// Paint `screen` filling `rect`. Returns the grid dimensions that fit.
pub fn paint_terminal(
    ui: &Ui,
    rect: Rect,
    screen: &vt100_ctt::Screen,
    focused: bool,
    font_size: f32,
) -> Grid {
    let font_id = FontId::monospace(font_size);
    let (glyph_w, line_h) = ui
        .ctx()
        .fonts_mut(|f| (f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));

    let pad = 3.0;
    let inner = rect.shrink(pad);
    let cols = ((inner.width() / glyph_w).floor() as usize).clamp(2, 500);
    let rows = ((inner.height() / line_h).floor() as usize).clamp(2, 200);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DEFAULT_BG);

    let origin = inner.min;
    let (srows, scols) = screen.size();
    let draw_rows = (srows as usize).min(rows);
    let draw_cols = (scols as usize).min(cols);

    // Pass 1: background rects, merged into runs.
    for row in 0..draw_rows {
        let y = origin.y + row as f32 * line_h;
        let mut col = 0;
        while col < draw_cols {
            let bg = cell_bg(screen, row, col);
            let mut run = col + 1;
            while run < draw_cols && cell_bg(screen, row, run) == bg {
                run += 1;
            }
            if bg != DEFAULT_BG {
                let x = origin.x + col as f32 * glyph_w;
                let w = (run - col) as f32 * glyph_w;
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, line_h)),
                    0.0,
                    bg,
                );
            }
            col = run;
        }
    }

    // Pass 2: text runs grouped by style.
    for row in 0..draw_rows {
        let y = origin.y + row as f32 * line_h;
        let mut col = 0;
        while col < draw_cols {
            let cell = match screen.cell(row as u16, col as u16) {
                Some(c) => c,
                None => {
                    col += 1;
                    continue;
                }
            };
            if cell.is_wide_continuation() || !cell.has_contents() {
                col += 1;
                continue;
            }
            let style = cell_style(cell);
            if cell.is_wide() {
                let x = origin.x + col as f32 * glyph_w;
                painter.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    cell.contents(),
                    font_id.clone(),
                    style.0,
                );
                col += 2;
                continue;
            }
            let mut text = String::from(cell.contents());
            let mut run = col + 1;
            while run < draw_cols {
                let next = match screen.cell(row as u16, run as u16) {
                    Some(c) => c,
                    None => break,
                };
                if next.is_wide() || next.is_wide_continuation() || cell_style(next) != style {
                    break;
                }
                if next.has_contents() {
                    text.push_str(next.contents());
                } else {
                    text.push(' ');
                }
                run += 1;
            }
            let x = origin.x + col as f32 * glyph_w;
            if !text.chars().all(|c| c == ' ') {
                painter.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    text.trim_end(),
                    font_id.clone(),
                    style.0,
                );
            }
            if style.1 {
                let w = (run - col) as f32 * glyph_w;
                painter.line_segment(
                    [
                        Pos2::new(x, y + line_h - 1.5),
                        Pos2::new(x + w, y + line_h - 1.5),
                    ],
                    (1.0, style.0),
                );
            }
            col = run;
        }
    }

    // Cursor.
    if focused && !screen.hide_cursor() {
        let (crow, ccol) = screen.cursor_position();
        if (crow as usize) < draw_rows && (ccol as usize) < draw_cols {
            let x = origin.x + ccol as f32 * glyph_w;
            let y = origin.y + crow as f32 * line_h;
            let cur = Rect::from_min_size(Pos2::new(x, y), Vec2::new(glyph_w, line_h));
            painter.rect_filled(cur, 0.0, DEFAULT_FG);
            if let Some(cell) = screen.cell(crow, ccol) {
                if cell.has_contents() {
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        cell.contents(),
                        font_id.clone(),
                        DEFAULT_BG,
                    );
                }
            }
        }
    }

    Grid { cols, rows }
}

fn cell_bg(screen: &vt100_ctt::Screen, row: usize, col: usize) -> Color32 {
    match screen.cell(row as u16, col as u16) {
        Some(cell) => resolve_colors(cell).1,
        None => DEFAULT_BG,
    }
}

fn resolve_colors(cell: &vt100_ctt::Cell) -> (Color32, Color32) {
    let mut fg = convert_fg(cell.fgcolor(), cell.bold());
    let mut bg = convert_bg(cell.bgcolor());
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
        if cell.fgcolor() == vt100_ctt::Color::Default {
            bg = DEFAULT_FG;
        }
        if cell.bgcolor() == vt100_ctt::Color::Default {
            fg = DEFAULT_BG;
        }
    }
    if cell.dim() {
        let [r, g, b, _] = fg.to_array();
        fg = Color32::from_rgb(
            (r as f32 * 0.6) as u8,
            (g as f32 * 0.6) as u8,
            (b as f32 * 0.6) as u8,
        );
    }
    (fg, bg)
}

fn cell_style(cell: &vt100_ctt::Cell) -> (Color32, bool) {
    let (fg, _) = resolve_colors(cell);
    (fg, cell.underline())
}
