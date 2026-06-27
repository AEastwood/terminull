//! The Terminull GUI: a manager that launches terminals like lightweight VMs,
//! lets you close them, minimise them to live-preview thumbnails, and spawn any
//! number of instances on request.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};

use crate::grid::{lock_grid, Grid};
use crate::terminal::{TermState, Terminal};

const DEFAULT_COLS: usize = 100;
const DEFAULT_ROWS: usize = 30;
const FONT_SIZE: f32 = 14.0;

pub struct TerminullApp {
    terminals: Vec<Terminal>,
    next_id: u64,
    /// id of the terminal currently shown full-size, if any.
    active: Option<u64>,
    spawn_count: i32,
    /// Command typed into the broadcast box (sent to every terminal at once).
    broadcast: String,
    /// Previously sent broadcast commands, oldest first.
    broadcast_history: Vec<String>,
    /// Cursor into `broadcast_history` while scrolling with Up/Down. `None`
    /// means we're editing a fresh line rather than browsing history.
    broadcast_history_pos: Option<usize>,
    /// When true, the central area tiles terminals in a grid instead of showing
    /// a single full-screen terminal.
    grid_view: bool,
    grid_rows: i32,
    grid_cols: i32,
    /// Sum of every grid's change counter last frame, used to skip repaints
    /// while nothing is happening.
    last_dirty: u64,
}

impl Default for TerminullApp {
    fn default() -> Self {
        let mut app = TerminullApp {
            terminals: Vec::new(),
            next_id: 1,
            active: None,
            spawn_count: 3,
            broadcast: String::new(),
            broadcast_history: Vec::new(),
            broadcast_history_pos: None,
            grid_view: false,
            grid_rows: 2,
            grid_cols: 2,
            last_dirty: 0,
        };
        // Start with one terminal so the window isn't empty.
        app.spawn_terminals(1);
        app
    }
}

impl TerminullApp {
    fn spawn_terminals(&mut self, count: usize) {
        for _ in 0..count {
            let id = self.next_id;
            match Terminal::spawn(id, DEFAULT_COLS, DEFAULT_ROWS) {
                Ok(mut term) => {
                    // Only the newest stays Active/full; older ones minimise.
                    for t in self.terminals.iter_mut() {
                        if t.state == TermState::Active {
                            t.state = TermState::Minimized;
                        }
                    }
                    term.state = TermState::Active;
                    self.active = Some(id);
                    self.terminals.push(term);
                    self.next_id += 1;
                }
                Err(e) => {
                    eprintln!("failed to spawn terminal: {e}");
                }
            }
        }
    }

    fn close(&mut self, id: u64) {
        self.terminals.retain(|t| t.id != id);
        if self.active == Some(id) {
            self.active = self.terminals.first().map(|t| t.id);
            if let Some(active_id) = self.active {
                for t in self.terminals.iter_mut() {
                    if t.id == active_id {
                        t.state = TermState::Active;
                    }
                }
            }
        }
    }

    fn restore(&mut self, id: u64) {
        for t in self.terminals.iter_mut() {
            if t.id == id {
                t.state = TermState::Active;
            } else if t.state == TermState::Active {
                t.state = TermState::Minimized;
            }
        }
        self.active = Some(id);
    }

    fn minimize(&mut self, id: u64) {
        for t in self.terminals.iter_mut() {
            if t.id == id {
                t.state = TermState::Minimized;
            }
        }
        if self.active == Some(id) {
            self.active = None;
        }
    }

    /// Send raw bytes to every terminal at once.
    fn broadcast_bytes(&mut self, bytes: &[u8]) {
        for t in self.terminals.iter_mut() {
            t.send(bytes);
        }
    }

    /// Send a command line (followed by Enter) to every terminal at once.
    fn broadcast_command(&mut self, cmd: &str) {
        for t in self.terminals.iter_mut() {
            t.send(cmd.as_bytes());
            t.send(b"\r");
        }
    }

    /// Record a sent broadcast in the history, skipping blanks and immediate
    /// duplicates, and reset the browse cursor.
    fn record_broadcast(&mut self, cmd: String) {
        if !cmd.is_empty() && self.broadcast_history.last() != Some(&cmd) {
            self.broadcast_history.push(cmd);
        }
        self.broadcast_history_pos = None;
    }

    /// Step back to an older broadcast command (Up arrow).
    fn broadcast_history_prev(&mut self) {
        if self.broadcast_history.is_empty() {
            return;
        }
        let pos = match self.broadcast_history_pos {
            Some(0) => 0,
            Some(p) => p - 1,
            None => self.broadcast_history.len() - 1,
        };
        self.broadcast_history_pos = Some(pos);
        self.broadcast = self.broadcast_history[pos].clone();
    }

    /// Step forward to a newer broadcast command, or back to a blank line once
    /// past the newest entry (Down arrow).
    fn broadcast_history_next(&mut self) {
        let Some(pos) = self.broadcast_history_pos else {
            return;
        };
        if pos + 1 < self.broadcast_history.len() {
            self.broadcast_history_pos = Some(pos + 1);
            self.broadcast = self.broadcast_history[pos + 1].clone();
        } else {
            self.broadcast_history_pos = None;
            self.broadcast.clear();
        }
    }
}

impl eframe::App for TerminullApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Reap dead shells (process exited) so they don't linger as ghosts.
        let dead: Vec<u64> = self
            .terminals
            .iter()
            .filter(|t| !t.is_alive())
            .map(|t| t.id)
            .collect();
        for id in dead {
            self.close(id);
        }

        // Repaint promptly when any terminal produced output, otherwise tick
        // slowly so an idle window doesn't burn CPU.
        let dirty: u64 = self
            .terminals
            .iter()
            .fold(0u64, |acc, t| acc.wrapping_add(lock_grid(&t.grid).dirty));
        if dirty != self.last_dirty {
            self.last_dirty = dirty;
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        self.top_bar(ctx);
        self.dock(ctx);
        self.central(ctx);
    }
}

impl TerminullApp {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Terminull");
                ui.separator();
                if ui.button("➕ New terminal").clicked() {
                    self.spawn_terminals(1);
                }
                ui.separator();
                ui.label("Spawn");
                ui.add(
                    egui::DragValue::new(&mut self.spawn_count)
                        .range(1..=256)
                        .speed(0.2),
                );
                if ui.button("Spawn instances").clicked() {
                    let n = self.spawn_count.max(1) as usize;
                    self.spawn_terminals(n);
                }
                ui.separator();
                ui.checkbox(&mut self.grid_view, "▦ Grid view");
                ui.add_enabled_ui(self.grid_view, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.grid_rows)
                            .range(1..=8)
                            .prefix("rows "),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut self.grid_cols)
                            .range(1..=8)
                            .prefix("cols "),
                    );
                });
                ui.separator();
                ui.label(format!("{} running", self.terminals.len()));
            });

            // Second row: broadcast a command to every terminal at once.
            ui.horizontal(|ui| {
                ui.label("📡 Broadcast:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.broadcast)
                        .desired_width(320.0)
                        .hint_text("command to send to ALL terminals"),
                );

                // Up/Down scrolls through previously sent commands, shell-style.
                if resp.has_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        self.broadcast_history_prev();
                    } else if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        self.broadcast_history_next();
                    }
                }

                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Send to all").clicked() || enter {
                    let cmd = self.broadcast.clone();
                    self.broadcast_command(&cmd);
                    self.record_broadcast(cmd);
                    self.broadcast.clear();
                    resp.request_focus();
                }
                ui.separator();
                if ui
                    .button("⛔ Ctrl+C all")
                    .on_hover_text("Send SIGINT (Ctrl+C) to every terminal")
                    .clicked()
                {
                    self.broadcast_bytes(&[0x03]);
                }
            });
        });
    }

    /// Bottom dock: a live-preview thumbnail of *every* terminal (taskbar
    /// style). The currently selected terminal's preview is outlined in blue.
    fn dock(&mut self, ctx: &egui::Context) {
        let ids: Vec<u64> = self.terminals.iter().map(|t| t.id).collect();

        if ids.is_empty() {
            return;
        }

        egui::TopBottomPanel::bottom("dock")
            .resizable(false)
            .min_height(120.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label("Terminals (live preview)");
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for id in ids {
                            self.thumbnail(ui, id);
                        }
                    });
                });
                ui.add_space(4.0);
            });
    }

    fn thumbnail(&mut self, ui: &mut egui::Ui, id: u64) {
        // Pull out everything we need *before* the closure so we don't hold a
        // borrow of `self` across UI calls that also mutate `self`.
        let (title, grid) = {
            let Some(term) = self.terminals.iter().find(|t| t.id == id) else {
                return;
            };
            (term.title.clone(), Arc::clone(&term.grid))
        };
        let is_active = self.active == Some(id);

        let mut do_restore = false;
        let mut do_close = false;

        let thumb_size = Vec2::new(180.0, 96.0);
        ui.vertical(|ui| {
            let (rect, resp) = ui.allocate_exact_size(thumb_size, Sense::click());

            // Frame. The selected/active terminal gets a bold blue outline.
            ui.painter()
                .rect_filled(rect, 4.0, Color32::from_rgb(0x10, 0x10, 0x14));
            let stroke = if is_active {
                Stroke::new(3.0, Color32::from_rgb(0x3B, 0x8E, 0xEA))
            } else if resp.hovered() {
                Stroke::new(2.0, Color32::from_rgb(0x5A, 0x6A, 0x80))
            } else {
                Stroke::new(1.0, Color32::from_gray(60))
            };
            ui.painter().rect_stroke(rect, 4.0, stroke);

            // Live, scaled-down render of the grid (cheap: coloured blocks).
            paint_grid_preview(ui, rect.shrink(3.0), &lock_grid(&grid));

            ui.horizontal(|ui| {
                if ui.small_button("▢").on_hover_text("Restore").clicked() {
                    do_restore = true;
                }
                if ui.small_button("✖").on_hover_text("Close").clicked() {
                    do_close = true;
                }
                ui.label(egui::RichText::new(title).small());
            });

            if resp.clicked() {
                do_restore = true;
            }
        });
        ui.add_space(6.0);

        if do_restore {
            self.restore(id);
        }
        if do_close {
            self.close(id);
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.grid_view {
                self.grid_central(ui);
                return;
            }

            let Some(active_id) = self.active else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "No active terminal.\nClick a thumbnail to restore, or spawn a new one.",
                        )
                        .size(16.0),
                    );
                });
                return;
            };

            // Title row with an editable name and window-style controls.
            let mut do_close = false;
            let mut do_minimize = false;
            ui.horizontal(|ui| {
                ui.label("Name:");
                if let Some(term) = self.terminals.iter_mut().find(|t| t.id == active_id) {
                    ui.add(
                        egui::TextEdit::singleline(&mut term.title)
                            .desired_width(240.0)
                            .hint_text("terminal name"),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✖ Close").clicked() {
                        do_close = true;
                    }
                    if ui.button("➖ Minimize").clicked() {
                        do_minimize = true;
                    }
                });
            });
            ui.separator();

            if do_close {
                self.close(active_id);
            }
            if do_minimize {
                self.minimize(active_id);
            }

            let avail = ui.available_size();
            self.render_terminal(ui, active_id, avail, true, false);
        });
    }

    /// Render one terminal into the available space, handling resize, focus,
    /// painting and keyboard routing.
    ///
    /// * `auto_focus` — grab keyboard focus when nothing else holds it (used by
    ///   the single full-screen view so typing works immediately).
    /// * `draw_outline` — outline this terminal in blue when it is the selected
    ///   one (used by the tiled grid to mark the active cell).
    fn render_terminal(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        avail: Vec2,
        auto_focus: bool,
        draw_outline: bool,
    ) -> egui::Response {
        let font = FontId::monospace(FONT_SIZE);
        let (cell_w, cell_h) = ui.fonts(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));

        // Fit the PTY grid to the available pixels.
        let cols = ((avail.x / cell_w).floor() as usize).max(10);
        let rows = ((avail.y / cell_h).floor() as usize).max(3);

        let grid_size = Vec2::new(cols as f32 * cell_w, rows as f32 * cell_h);
        let (rect, resp) = ui.allocate_exact_size(grid_size, Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::Text);

        // Resize the terminal to match the viewport.
        if let Some(term) = self.terminals.iter_mut().find(|t| t.id == id) {
            term.resize(cols, rows);
        }

        // Click to focus + select this terminal.
        if resp.clicked() {
            resp.request_focus();
            self.active = Some(id);
        }
        // Auto-focus if nothing else holds it.
        if auto_focus && ui.memory(|m| m.focused().is_none()) {
            resp.request_focus();
        }

        // Render the grid.
        if let Some(term) = self.terminals.iter().find(|t| t.id == id) {
            let grid = lock_grid(&term.grid);
            paint_grid_full(ui, rect, &grid, cell_w, cell_h, &font, resp.has_focus());
        }

        // Mark the selected cell with a blue outline.
        if draw_outline && self.active == Some(id) {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                2.0,
                Stroke::new(2.0, Color32::from_rgb(0x3B, 0x8E, 0xEA)),
            );
        }

        // Route keyboard input to the focused terminal.
        if resp.has_focus() {
            // Claim Tab and the arrow keys so egui sends them to the shell
            // instead of using them to move keyboard focus between widgets.
            let filter = egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            };
            ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, filter));

            let bytes = collect_input(ui.ctx());
            if !bytes.is_empty() {
                if let Some(term) = self.terminals.iter_mut().find(|t| t.id == id) {
                    term.send(&bytes);
                }
            }
        }

        resp
    }

    /// Tiled grid view: lay out the first `rows × cols` terminals side by side,
    /// each independently interactive.
    fn grid_central(&mut self, ui: &mut egui::Ui) {
        let cols = self.grid_cols.clamp(1, 8) as usize;
        let rows = self.grid_rows.clamp(1, 8) as usize;

        let ids: Vec<u64> = self
            .terminals
            .iter()
            .map(|t| t.id)
            .take(rows * cols)
            .collect();

        if ids.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("No terminals. Spawn some to fill the grid.").size(16.0),
                );
            });
            return;
        }

        let avail = ui.available_size();
        let pad = 4.0;
        // Clamp so a small window with many columns can't produce zero/negative
        // tile sizes.
        let cell_w = ((avail.x / cols as f32) - pad).max(80.0);
        let cell_h = ((avail.y / rows as f32) - pad).max(60.0);

        // A tile the user asked to open full-screen, applied after the loop.
        let mut maximize: Option<u64> = None;

        for r in 0..rows {
            ui.horizontal(|ui| {
                for c in 0..cols {
                    let idx = r * cols + c;
                    let Some(&id) = ids.get(idx) else { continue };
                    ui.allocate_ui(Vec2::new(cell_w, cell_h), |ui| {
                        ui.vertical(|ui| {
                            // Per-cell header: editable name + maximize button.
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button("⛶")
                                    .on_hover_text("Open this terminal full-screen")
                                    .clicked()
                                {
                                    maximize = Some(id);
                                }
                                if let Some(term) = self.terminals.iter_mut().find(|t| t.id == id) {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut term.title)
                                            .desired_width((cell_w - 40.0).max(50.0))
                                            .hint_text("name"),
                                    );
                                }
                            });
                            let inner = ui.available_size();
                            let resp = self.render_terminal(ui, id, inner, false, true);
                            if resp.double_clicked() {
                                maximize = Some(id);
                            }
                        });
                    });
                }
            });
        }

        // Double-clicking or the ⛶ button pulls one terminal out to full-screen.
        if let Some(id) = maximize {
            self.grid_view = false;
            self.active = Some(id);
        }
    }
}

/// Full-fidelity render: backgrounds, glyphs, bold, and a cursor.
fn paint_grid_full(
    ui: &egui::Ui,
    rect: Rect,
    grid: &Grid,
    cell_w: f32,
    cell_h: f32,
    font: &FontId,
    focused: bool,
) {
    let painter = ui.painter_at(rect);
    let origin = rect.min;

    for y in 0..grid.rows {
        for x in 0..grid.cols {
            let cell = grid.cell(x, y);
            let p = Pos2::new(origin.x + x as f32 * cell_w, origin.y + y as f32 * cell_h);
            let cell_rect = Rect::from_min_size(p, Vec2::new(cell_w, cell_h));

            let bg = Color32::from_rgb(cell.bg[0], cell.bg[1], cell.bg[2]);
            if bg != Color32::from_rgb(0x10, 0x10, 0x14) {
                painter.rect_filled(cell_rect, 0.0, bg);
            }

            if cell.c != ' ' {
                let fg = Color32::from_rgb(cell.fg[0], cell.fg[1], cell.fg[2]);
                let fid = if cell.bold {
                    FontId::monospace(font.size)
                } else {
                    font.clone()
                };
                painter.text(p, egui::Align2::LEFT_TOP, cell.c, fid, fg);
            }
        }
    }

    // Cursor block.
    let cx = origin.x + grid.cx as f32 * cell_w;
    let cy = origin.y + grid.cy as f32 * cell_h;
    let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(cell_w, cell_h));
    let cursor_col = if focused {
        Color32::from_rgba_unmultiplied(0x3B, 0x8E, 0xEA, 160)
    } else {
        Color32::from_rgba_unmultiplied(0x88, 0x88, 0x88, 90)
    };
    painter.rect_filled(cursor_rect, 0.0, cursor_col);
}

/// Cheap thumbnail render: one small block per cell coloured by content.
fn paint_grid_preview(ui: &egui::Ui, rect: Rect, grid: &Grid) {
    let painter = ui.painter_at(rect);
    let cw = rect.width() / grid.cols as f32;
    let ch = rect.height() / grid.rows as f32;

    for y in 0..grid.rows {
        for x in 0..grid.cols {
            let cell = grid.cell(x, y);
            let color = if cell.c != ' ' {
                Color32::from_rgb(cell.fg[0], cell.fg[1], cell.fg[2])
            } else if cell.bg != crate::grid::DEFAULT_BG {
                Color32::from_rgb(cell.bg[0], cell.bg[1], cell.bg[2])
            } else {
                continue;
            };
            let p = Pos2::new(rect.min.x + x as f32 * cw, rect.min.y + y as f32 * ch);
            painter.rect_filled(
                Rect::from_min_size(p, Vec2::new(cw.max(1.0), ch.max(1.0))),
                0.0,
                color,
            );
        }
    }
}

/// Translate this frame's egui keyboard events into terminal byte sequences.
fn collect_input(ctx: &egui::Context) -> Vec<u8> {
    let mut out = Vec::new();
    ctx.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Text(text) => out.extend_from_slice(text.as_bytes()),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Ctrl + letter -> control code.
                    if modifiers.ctrl && !modifiers.alt {
                        if let Some(c) = key_letter(*key) {
                            out.push((c as u8) & 0x1f);
                            continue;
                        }
                    }
                    match key {
                        Key::Enter => out.push(b'\r'),
                        Key::Backspace => out.push(0x7f),
                        Key::Tab => out.push(b'\t'),
                        Key::Escape => out.push(0x1b),
                        Key::ArrowUp => out.extend_from_slice(b"\x1b[A"),
                        Key::ArrowDown => out.extend_from_slice(b"\x1b[B"),
                        Key::ArrowRight => out.extend_from_slice(b"\x1b[C"),
                        Key::ArrowLeft => out.extend_from_slice(b"\x1b[D"),
                        Key::Home => out.extend_from_slice(b"\x1b[H"),
                        Key::End => out.extend_from_slice(b"\x1b[F"),
                        Key::Delete => out.extend_from_slice(b"\x1b[3~"),
                        Key::PageUp => out.extend_from_slice(b"\x1b[5~"),
                        Key::PageDown => out.extend_from_slice(b"\x1b[6~"),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    });
    out
}

fn key_letter(key: Key) -> Option<char> {
    use Key::*;
    Some(match key {
        A => 'a',
        B => 'b',
        C => 'c',
        D => 'd',
        E => 'e',
        F => 'f',
        G => 'g',
        H => 'h',
        I => 'i',
        J => 'j',
        K => 'k',
        L => 'l',
        M => 'm',
        N => 'n',
        O => 'o',
        P => 'p',
        Q => 'q',
        R => 'r',
        S => 's',
        T => 't',
        U => 'u',
        V => 'v',
        W => 'w',
        X => 'x',
        Y => 'y',
        Z => 'z',
        _ => return None,
    })
}
