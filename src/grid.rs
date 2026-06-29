//! A minimal terminal screen model plus an ANSI/VT escape-sequence parser.
//!
//! `Grid` holds a rectangular buffer of `Cell`s and implements [`vte::Perform`]
//! so it can be fed raw PTY output directly. It is intentionally a *useful
//! subset* of a full terminal: enough to drive interactive shells, render text
//! with colour, and produce a recognisable live preview.

use std::sync::{Mutex, MutexGuard};

use vte::{Params, Perform};

/// Lock a grid mutex, recovering the guard even if a previous holder panicked.
/// A poisoned lock just means the grid may hold stale data, which is never a
/// reason to crash the UI, so we take the inner value and carry on.
pub fn lock_grid(m: &Mutex<Grid>) -> MutexGuard<'_, Grid> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 8-bit RGB colour.
pub type Rgb = [u8; 3];

pub const DEFAULT_FG: Rgb = [0xCC, 0xCC, 0xCC];
pub const DEFAULT_BG: Rgb = [0x10, 0x10, 0x14];

/// The classic 16-colour ANSI palette (normal 0-7, bright 8-15).
const PALETTE: [Rgb; 16] = [
    [0x00, 0x00, 0x00], // black
    [0xCD, 0x31, 0x31], // red
    [0x0D, 0xBC, 0x79], // green
    [0xE5, 0xE5, 0x10], // yellow
    [0x24, 0x72, 0xC8], // blue
    [0xBC, 0x3F, 0xBC], // magenta
    [0x11, 0xA8, 0xCD], // cyan
    [0xE5, 0xE5, 0xE5], // white
    [0x66, 0x66, 0x66], // bright black
    [0xF1, 0x4C, 0x4C], // bright red
    [0x23, 0xD1, 0x8B], // bright green
    [0xF5, 0xF5, 0x43], // bright yellow
    [0x3B, 0x8E, 0xEA], // bright blue
    [0xD6, 0x70, 0xD6], // bright magenta
    [0x29, 0xB8, 0xDB], // bright cyan
    [0xFF, 0xFF, 0xFF], // bright white
];

#[derive(Clone, Copy)]
pub struct Cell {
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
        }
    }
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    pub cx: usize,
    pub cy: usize,
    cur_fg: Rgb,
    cur_bg: Rgb,
    bold: bool,
    /// Bumped whenever the grid changes, so the UI can cheaply detect activity.
    pub dirty: u64,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Grid {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cx: 0,
            cy: 0,
            cur_fg: DEFAULT_FG,
            cur_bg: DEFAULT_BG,
            bold: false,
            dirty: 0,
        }
    }

    #[inline]
    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }

    fn touch(&mut self) {
        self.dirty = self.dirty.wrapping_add(1);
    }

    /// Resize the visible area, preserving as much content as possible.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut next = vec![Cell::default(); cols * rows];
        for y in 0..rows.min(self.rows) {
            for x in 0..cols.min(self.cols) {
                next[y * cols + x] = self.cells[y * self.cols + x];
            }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.touch();
    }

    fn blank_cell(&self) -> Cell {
        Cell {
            c: ' ',
            fg: self.cur_fg,
            bg: self.cur_bg,
            bold: false,
        }
    }

    fn scroll_up(&mut self) {
        // Drop the top row and append a blank one at the bottom.
        self.cells.drain(0..self.cols);
        let blank = self.blank_cell();
        self.cells.extend(std::iter::repeat_n(blank, self.cols));
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cy += 1;
        }
    }

    fn put_char(&mut self, c: char) {
        if self.cx >= self.cols {
            self.cx = 0;
            self.newline();
        }
        let idx = self.cy * self.cols + self.cx;
        self.cells[idx] = Cell {
            c,
            fg: self.cur_fg,
            bg: self.cur_bg,
            bold: self.bold,
        };
        self.cx += 1;
    }

    fn erase_line_from_cursor(&mut self) {
        let blank = self.blank_cell();
        for x in self.cx..self.cols {
            self.cells[self.cy * self.cols + x] = blank;
        }
    }

    fn erase_line_to_cursor(&mut self) {
        let blank = self.blank_cell();
        for x in 0..=self.cx.min(self.cols - 1) {
            self.cells[self.cy * self.cols + x] = blank;
        }
    }

    fn erase_whole_line(&mut self) {
        let blank = self.blank_cell();
        for x in 0..self.cols {
            self.cells[self.cy * self.cols + x] = blank;
        }
    }

    fn erase_below(&mut self) {
        self.erase_line_from_cursor();
        let blank = self.blank_cell();
        for y in (self.cy + 1)..self.rows {
            for x in 0..self.cols {
                self.cells[y * self.cols + x] = blank;
            }
        }
    }

    fn erase_above(&mut self) {
        self.erase_line_to_cursor();
        let blank = self.blank_cell();
        for y in 0..self.cy {
            for x in 0..self.cols {
                self.cells[y * self.cols + x] = blank;
            }
        }
    }

    fn erase_all(&mut self) {
        let blank = self.blank_cell();
        for cell in self.cells.iter_mut() {
            *cell = blank;
        }
    }

    fn apply_sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        if params.is_empty() {
            self.reset_sgr();
            return;
        }
        while i < params.len() {
            let p = params[i];
            match p {
                0 => self.reset_sgr(),
                1 => self.bold = true,
                22 => self.bold = false,
                30..=37 => self.cur_fg = PALETTE[(p - 30) as usize],
                90..=97 => self.cur_fg = PALETTE[(p - 90 + 8) as usize],
                40..=47 => self.cur_bg = PALETTE[(p - 40) as usize],
                100..=107 => self.cur_bg = PALETTE[(p - 100 + 8) as usize],
                39 => self.cur_fg = DEFAULT_FG,
                49 => self.cur_bg = DEFAULT_BG,
                38 | 48 => {
                    // Extended colour: 38;5;n / 38;2;r;g;b (and 48;* for bg).
                    let target_fg = p == 38;
                    if let Some(&mode) = params.get(i + 1) {
                        match mode {
                            5 => {
                                if let Some(&idx) = params.get(i + 2) {
                                    let rgb = color256(idx as u8);
                                    if target_fg {
                                        self.cur_fg = rgb;
                                    } else {
                                        self.cur_bg = rgb;
                                    }
                                    i += 2;
                                }
                            }
                            2 => {
                                if let (Some(&r), Some(&g), Some(&b)) =
                                    (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                                {
                                    let rgb = [r as u8, g as u8, b as u8];
                                    if target_fg {
                                        self.cur_fg = rgb;
                                    } else {
                                        self.cur_bg = rgb;
                                    }
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn reset_sgr(&mut self) {
        self.cur_fg = DEFAULT_FG;
        self.cur_bg = DEFAULT_BG;
        self.bold = false;
    }
}

/// Map an xterm 256-colour index to RGB.
fn color256(idx: u8) -> Rgb {
    match idx {
        0..=15 => PALETTE[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [conv(r), conv(g), conv(b)]
        }
        _ => {
            let v = 8 + (idx - 232) * 10;
            [v, v, v]
        }
    }
}

impl Perform for Grid {
    fn print(&mut self, c: char) {
        self.put_char(c);
        self.touch();
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cx = 0,
            b'\t' => {
                let next = ((self.cx / 8) + 1) * 8;
                self.cx = next.min(self.cols - 1);
            }
            0x08 => {
                if self.cx > 0 {
                    self.cx -= 1;
                }
            }
            _ => {}
        }
        self.touch();
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Flatten params (handles both `;` and `:` separated sub-parameters).
        let flat: Vec<u16> = params.iter().flatten().copied().collect();
        let first = flat.first().copied().unwrap_or(0);
        let arg = |n: u16| if first == 0 { n } else { first };

        match action {
            'm' => self.apply_sgr(&flat),
            'H' | 'f' => {
                let row = flat.first().copied().unwrap_or(1).max(1) as usize - 1;
                let col = flat.get(1).copied().unwrap_or(1).max(1) as usize - 1;
                self.cy = row.min(self.rows - 1);
                self.cx = col.min(self.cols - 1);
            }
            'A' => self.cy = self.cy.saturating_sub(arg(1) as usize),
            'B' => self.cy = (self.cy + arg(1) as usize).min(self.rows - 1),
            'C' => self.cx = (self.cx + arg(1) as usize).min(self.cols - 1),
            'D' => self.cx = self.cx.saturating_sub(arg(1) as usize),
            'G' => self.cx = (first.max(1) as usize - 1).min(self.cols - 1),
            'd' => self.cy = (first.max(1) as usize - 1).min(self.rows - 1),
            'J' => match first {
                0 => self.erase_below(),
                1 => self.erase_above(),
                2 | 3 => {
                    self.erase_all();
                    self.cx = 0;
                    self.cy = 0;
                }
                _ => {}
            },
            'K' => match first {
                0 => self.erase_line_from_cursor(),
                1 => self.erase_line_to_cursor(),
                2 => self.erase_whole_line(),
                _ => {}
            },
            _ => {}
        }
        self.touch();
    }

    // Unused hooks for the DCS / OSC paths — we ignore them.
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed raw bytes through a fresh VT parser into the grid, exactly as the
    /// reader thread does in `terminal.rs`.
    fn feed(grid: &mut Grid, bytes: &[u8]) {
        let mut parser = vte::Parser::new();
        for &b in bytes {
            parser.advance(grid, b);
        }
    }

    /// Collect a row's characters into a `String`, trailing blanks trimmed.
    fn row(grid: &Grid, y: usize) -> String {
        let line: String = (0..grid.cols).map(|x| grid.cell(x, y).c).collect();
        line.trim_end().to_string()
    }

    #[test]
    fn new_grid_is_blank_with_given_dimensions() {
        let g = Grid::new(8, 4);
        assert_eq!(g.cols, 8);
        assert_eq!(g.rows, 4);
        assert_eq!(g.cx, 0);
        assert_eq!(g.cy, 0);
        for y in 0..g.rows {
            for x in 0..g.cols {
                let cell = g.cell(x, y);
                assert_eq!(cell.c, ' ');
                assert_eq!(cell.fg, DEFAULT_FG);
                assert_eq!(cell.bg, DEFAULT_BG);
                assert!(!cell.bold);
            }
        }
    }

    #[test]
    fn printing_text_fills_cells_and_advances_cursor() {
        let mut g = Grid::new(20, 3);
        feed(&mut g, b"hello");
        assert_eq!(row(&g, 0), "hello");
        assert_eq!(g.cx, 5);
        assert_eq!(g.cy, 0);
    }

    #[test]
    fn printing_past_the_right_edge_wraps_to_next_line() {
        let mut g = Grid::new(5, 3);
        feed(&mut g, b"abcdefg");
        assert_eq!(row(&g, 0), "abcde");
        assert_eq!(row(&g, 1), "fg");
        assert_eq!(g.cy, 1);
        assert_eq!(g.cx, 2);
    }

    #[test]
    fn carriage_return_returns_to_column_zero() {
        let mut g = Grid::new(20, 3);
        feed(&mut g, b"hello\rworld");
        assert_eq!(row(&g, 0), "world");
        assert_eq!(g.cy, 0);
        assert_eq!(g.cx, 5);
    }

    #[test]
    fn newline_moves_down_keeping_column() {
        let mut g = Grid::new(20, 3);
        feed(&mut g, b"ab\ncd");
        // \n is a bare line feed: down a row, column preserved (no CR).
        assert_eq!(g.cy, 1);
        assert_eq!(g.cx, 4);
        assert_eq!(g.cell(2, 1).c, 'c');
        assert_eq!(g.cell(3, 1).c, 'd');
    }

    #[test]
    fn crlf_starts_a_fresh_line() {
        let mut g = Grid::new(20, 3);
        feed(&mut g, b"ab\r\ncd");
        assert_eq!(row(&g, 0), "ab");
        assert_eq!(row(&g, 1), "cd");
    }

    #[test]
    fn tab_advances_to_next_eight_column_stop() {
        let mut g = Grid::new(40, 2);
        feed(&mut g, b"a\tb");
        assert_eq!(g.cell(0, 0).c, 'a');
        assert_eq!(g.cell(8, 0).c, 'b');
        assert_eq!(g.cx, 9);
    }

    #[test]
    fn tab_is_clamped_to_the_last_column() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\t\t");
        assert_eq!(g.cx, g.cols - 1);
    }

    #[test]
    fn backspace_moves_left_but_not_past_column_zero() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"ab\x08");
        assert_eq!(g.cx, 1);
        feed(&mut g, b"\x08\x08\x08");
        assert_eq!(g.cx, 0);
    }

    #[test]
    fn writing_past_the_bottom_scrolls_the_screen_up() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"top\r\nbottom\r\nnext");
        // First line scrolled off; "bottom" moved up, "next" on the last row.
        assert_eq!(row(&g, 0), "bottom");
        assert_eq!(row(&g, 1), "next");
        assert_eq!(g.cy, 1);
    }

    #[test]
    fn cursor_position_csi_is_one_based() {
        let mut g = Grid::new(20, 10);
        feed(&mut g, b"\x1b[3;5H");
        assert_eq!(g.cy, 2);
        assert_eq!(g.cx, 4);
        // Bare \x1b[H homes the cursor.
        feed(&mut g, b"\x1b[H");
        assert_eq!(g.cy, 0);
        assert_eq!(g.cx, 0);
    }

    #[test]
    fn cursor_position_is_clamped_to_the_grid() {
        let mut g = Grid::new(20, 10);
        feed(&mut g, b"\x1b[999;999H");
        assert_eq!(g.cy, 9);
        assert_eq!(g.cx, 19);
    }

    #[test]
    fn relative_cursor_movement_obeys_bounds() {
        let mut g = Grid::new(20, 10);
        feed(&mut g, b"\x1b[5;5H");
        feed(&mut g, b"\x1b[2A"); // up 2
        assert_eq!(g.cy, 2);
        feed(&mut g, b"\x1b[3B"); // down 3
        assert_eq!(g.cy, 5);
        feed(&mut g, b"\x1b[4C"); // right 4
        assert_eq!(g.cx, 8);
        feed(&mut g, b"\x1b[2D"); // left 2
        assert_eq!(g.cx, 6);
        // Clamp at the edges.
        feed(&mut g, b"\x1b[99A");
        assert_eq!(g.cy, 0);
        feed(&mut g, b"\x1b[99D");
        assert_eq!(g.cx, 0);
    }

    #[test]
    fn default_movement_argument_is_one() {
        let mut g = Grid::new(20, 10);
        feed(&mut g, b"\x1b[5;5H\x1b[B"); // no arg -> down 1
        assert_eq!(g.cy, 5);
    }

    #[test]
    fn column_and_row_absolute_moves() {
        let mut g = Grid::new(20, 10);
        feed(&mut g, b"\x1b[7G"); // column 7 (1-based)
        assert_eq!(g.cx, 6);
        feed(&mut g, b"\x1b[4d"); // row 4 (1-based)
        assert_eq!(g.cy, 3);
    }

    #[test]
    fn erase_line_from_cursor() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"abcdef");
        feed(&mut g, b"\x1b[3G"); // back to column 3
        feed(&mut g, b"\x1b[0K");
        assert_eq!(row(&g, 0), "ab");
    }

    #[test]
    fn erase_line_to_cursor_replaces_with_spaces() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"abcdef");
        feed(&mut g, b"\x1b[3G"); // column 3 (index 2)
        feed(&mut g, b"\x1b[1K");
        // Columns 0..=2 blanked, the rest intact.
        assert_eq!(g.cell(0, 0).c, ' ');
        assert_eq!(g.cell(2, 0).c, ' ');
        assert_eq!(g.cell(3, 0).c, 'd');
    }

    #[test]
    fn erase_whole_line() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"abcdef");
        feed(&mut g, b"\x1b[2K");
        assert_eq!(row(&g, 0), "");
    }

    #[test]
    fn erase_all_clears_screen_and_homes_cursor() {
        let mut g = Grid::new(10, 3);
        feed(&mut g, b"line1\r\nline2\r\nline3");
        feed(&mut g, b"\x1b[2J");
        for y in 0..g.rows {
            assert_eq!(row(&g, y), "");
        }
        assert_eq!(g.cx, 0);
        assert_eq!(g.cy, 0);
    }

    #[test]
    fn erase_below_keeps_earlier_rows() {
        let mut g = Grid::new(10, 3);
        feed(&mut g, b"aaa\r\nbbb\r\nccc");
        feed(&mut g, b"\x1b[2;1H"); // row 2, col 1
        feed(&mut g, b"\x1b[0J"); // erase from cursor to end
        assert_eq!(row(&g, 0), "aaa");
        assert_eq!(row(&g, 1), "");
        assert_eq!(row(&g, 2), "");
    }

    #[test]
    fn erase_above_keeps_later_rows() {
        let mut g = Grid::new(10, 3);
        feed(&mut g, b"aaa\r\nbbb\r\nccc");
        feed(&mut g, b"\x1b[2;3H"); // row 2, col 3
        feed(&mut g, b"\x1b[1J"); // erase from start to cursor
        assert_eq!(row(&g, 0), "");
        assert_eq!(row(&g, 1), ""); // cols up to cursor cleared; rest were blank
        assert_eq!(row(&g, 2), "ccc");
    }

    #[test]
    fn sgr_sets_basic_foreground_and_background() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[31;42mX");
        let cell = g.cell(0, 0);
        assert_eq!(cell.c, 'X');
        assert_eq!(cell.fg, PALETTE[1]); // red
        assert_eq!(cell.bg, PALETTE[2]); // green
    }

    #[test]
    fn sgr_bright_colors_use_upper_palette() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[91;102mX");
        let cell = g.cell(0, 0);
        assert_eq!(cell.fg, PALETTE[1 + 8]); // bright red
        assert_eq!(cell.bg, PALETTE[2 + 8]); // bright green
    }

    #[test]
    fn sgr_bold_and_reset() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[1mB");
        assert!(g.cell(0, 0).bold);
        feed(&mut g, b"\x1b[0mN");
        let n = g.cell(1, 0);
        assert!(!n.bold);
        assert_eq!(n.fg, DEFAULT_FG);
        assert_eq!(n.bg, DEFAULT_BG);
    }

    #[test]
    fn sgr_empty_params_reset() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[31m\x1b[mX"); // set red, then bare reset
        assert_eq!(g.cell(0, 0).fg, DEFAULT_FG);
    }

    #[test]
    fn sgr_default_fg_bg_codes() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[31;42m\x1b[39;49mX");
        let cell = g.cell(0, 0);
        assert_eq!(cell.fg, DEFAULT_FG);
        assert_eq!(cell.bg, DEFAULT_BG);
    }

    #[test]
    fn sgr_256_color_foreground() {
        let mut g = Grid::new(10, 2);
        // 38;5;1 -> palette red.
        feed(&mut g, b"\x1b[38;5;1mX");
        assert_eq!(g.cell(0, 0).fg, PALETTE[1]);
    }

    #[test]
    fn sgr_truecolor_foreground_and_background() {
        let mut g = Grid::new(10, 2);
        feed(&mut g, b"\x1b[38;2;10;20;30;48;2;40;50;60mX");
        let cell = g.cell(0, 0);
        assert_eq!(cell.fg, [10, 20, 30]);
        assert_eq!(cell.bg, [40, 50, 60]);
    }

    #[test]
    fn color256_palette_cube_and_grayscale() {
        // 0..=15 map straight to the 16-colour palette.
        assert_eq!(color256(5), PALETTE[5]);
        // 16 is the bottom of the cube: pure black.
        assert_eq!(color256(16), [0, 0, 0]);
        // 231 is the top of the cube: pure white.
        assert_eq!(color256(231), [255, 255, 255]);
        // Grayscale ramp start.
        assert_eq!(color256(232), [8, 8, 8]);
        // Grayscale ramp end.
        assert_eq!(color256(255), [238, 238, 238]);
    }

    #[test]
    fn resize_preserves_overlapping_content() {
        let mut g = Grid::new(10, 4);
        feed(&mut g, b"hello");
        g.resize(20, 6);
        assert_eq!(g.cols, 20);
        assert_eq!(g.rows, 6);
        assert_eq!(row(&g, 0), "hello");
    }

    #[test]
    fn resize_clamps_cursor_into_new_bounds() {
        let mut g = Grid::new(10, 4);
        feed(&mut g, b"\x1b[4;9H"); // near bottom-right
        g.resize(5, 2);
        assert!(g.cx <= g.cols - 1);
        assert!(g.cy <= g.rows - 1);
        assert_eq!(g.cx, 4);
        assert_eq!(g.cy, 1);
    }

    #[test]
    fn resize_enforces_minimum_of_one() {
        let mut g = Grid::new(10, 4);
        g.resize(0, 0);
        assert_eq!(g.cols, 1);
        assert_eq!(g.rows, 1);
    }

    #[test]
    fn resize_to_same_dimensions_is_a_noop() {
        let mut g = Grid::new(10, 4);
        let before = g.dirty;
        g.resize(10, 4);
        assert_eq!(g.dirty, before);
    }

    #[test]
    fn dirty_counter_advances_on_output() {
        let mut g = Grid::new(10, 2);
        let before = g.dirty;
        feed(&mut g, b"x");
        assert_ne!(g.dirty, before);
    }

    #[test]
    fn lock_grid_recovers_from_a_poisoned_mutex() {
        use std::sync::{Arc, Mutex};
        let m = Arc::new(Mutex::new(Grid::new(4, 2)));
        let m2 = Arc::clone(&m);
        // Poison the mutex by panicking while the lock is held.
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        // lock_grid must still hand back a usable guard.
        let mut g = lock_grid(&m);
        g.resize(6, 3);
        assert_eq!(g.cols, 6);
    }
}
