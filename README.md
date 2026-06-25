# Terminull

Terminull is a desktop app for running a bunch of terminals at once and managing
them like windows. Each terminal is a real shell process behind its own
pseudo-terminal (ConPTY on Windows, a normal PTY elsewhere), so you get the same
shell you'd get from any other terminal, just wrapped in a UI that makes it easy
to spin up several, watch them all at once, and send commands to them.

I built it because I kept opening five terminal tabs to run the same command in
each one. Terminull lets you do that from a single window.

## What it can do

- Open as many terminals as you want, each one a separate shell.
- Spawn several at once when you need a batch of them.
- Minimise terminals to a strip of live thumbnails along the bottom. The
  thumbnails keep updating, so you can keep half an eye on a long build without
  giving it the whole screen.
- Tile terminals in a grid (pick the rows and columns) and work in all of them
  side by side.
- Click any terminal to type into just that one. Double-click (or the maximise
  button) to pop it out to full screen.
- Give terminals names so you can tell them apart.
- Broadcast a command to every terminal at once, or send Ctrl+C to all of them
  if something runs away.

The terminals understand the usual ANSI escape codes: 16/256/true-colour, bold,
cursor movement, line and screen clears, tabs, and the common keys like the
arrows, Home/End and Ctrl+C. They resize to fit whatever space they're given.

## Building

You'll need Rust (stable) and a working linker.

On Windows, use the MSVC toolchain. The GNU toolchain can fail to link because it
needs a complete MinGW install:

```
rustup default stable-x86_64-pc-windows-msvc
cargo run --release
```

On Linux or macOS the dependencies are cross-platform, so a plain `cargo run`
should work, though Windows is where I've actually used it.

## How it's put together

The code is small and split into four files:

- `src/grid.rs` holds the screen model (a grid of cells) and an implementation of
  `vte::Perform`, which is what turns raw shell output into something we can draw.
- `src/terminal.rs` owns one shell process and its PTY. A background thread reads
  the shell's output and feeds it through the parser into the grid.
- `src/app.rs` is the UI: the toolbar, the thumbnail dock, the single and grid
  views, the painting, and keyboard handling.
- `src/main.rs` just opens the window.

The flow is: shell writes to the PTY, the reader thread parses that into the grid
(which lives behind a mutex), and the UI thread locks the grid each frame to draw
it. The active terminal is drawn with real glyphs; the thumbnails are drawn as
small coloured blocks, which is cheap enough to do for all of them every frame.

## Using it

- Use the toolbar to open a new terminal or spawn a batch.
- Tick "Grid view" and set the rows and columns to tile them.
- Click a terminal to focus it, then type. The focused one gets a blue outline.
- Minimise sends a terminal to the dock; click its thumbnail to bring it back.
- The broadcast box sends a line to every terminal; "Ctrl+C all" interrupts them.
- Closing a terminal ends its shell. If a shell exits on its own, its terminal
  goes away automatically.

## Built with

- [egui / eframe](https://github.com/emilk/egui) for the UI.
- [portable-pty](https://crates.io/crates/portable-pty) for spawning shells in PTYs.
- [vte](https://crates.io/crates/vte) for parsing terminal escape sequences.

## License

MIT. See [LICENSE](LICENSE).
