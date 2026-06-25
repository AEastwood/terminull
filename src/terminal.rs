//! A single managed terminal: a real shell process behind a PTY, with a
//! background thread parsing its output into a shared [`Grid`].

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

use crate::grid::{lock_grid, Grid};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TermState {
    /// Shown full-size and receiving keyboard input.
    Active,
    /// Collapsed to a live-preview thumbnail in the dock.
    Minimized,
}

/// Flips an `alive` flag to `false` when dropped, so a terminal is marked dead
/// even if its reader thread exits by panicking rather than reaching the end.
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct Terminal {
    pub id: u64,
    pub title: String,
    pub grid: Arc<Mutex<Grid>>,
    pub state: TermState,
    pub alive: Arc<AtomicBool>,
    pub cols: usize,
    pub rows: usize,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Terminal {
    /// Launch a new shell in a fresh PTY of the given character dimensions.
    pub fn spawn(id: u64, cols: usize, rows: usize) -> Result<Terminal> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Use the platform's default interactive shell.
        let cmd = CommandBuilder::new_default_prog();
        let child = pair.slave.spawn_command(cmd)?;
        // The slave handle is owned by the child now; drop our copy so that
        // EOF propagates correctly when the shell exits.
        drop(pair.slave);

        let grid = Arc::new(Mutex::new(Grid::new(cols, rows)));
        let alive = Arc::new(AtomicBool::new(true));

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        // Reader thread: feed raw bytes through the VT parser into the grid.
        {
            let grid = Arc::clone(&grid);
            let alive = Arc::clone(&alive);
            thread::spawn(move || {
                // Marks the terminal dead on any exit, including a panic.
                let _guard = AliveGuard(alive);
                let mut parser = vte::Parser::new();
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut g = lock_grid(&grid);
                            for &b in &buf[..n] {
                                parser.advance(&mut *g, b);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        Ok(Terminal {
            id,
            title: format!("terminal {id}"),
            grid,
            state: TermState::Active,
            alive,
            cols,
            rows,
            master: pair.master,
            writer,
            child,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Send raw bytes (keystrokes) to the shell.
    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize both the PTY and the backing grid to new character dimensions.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        lock_grid(&self.grid).resize(cols, rows);
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Kill the shell and reap it so closing a terminal (or quitting the
        // app) never leaves an orphaned or zombie process behind. Dropping the
        // master afterwards unblocks the reader thread via EOF.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
