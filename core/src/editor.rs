//! Full-screen text editors: `vi` (modal — Normal/Insert/Command, like vi)
//! and `edit`/`nano` (the same engine started in Insert mode, modeless-ish).
//!
//! No embeddable Rust editor library fits our byte-stream model (they assume
//! termios + /dev/tty), so this is self-contained. It drives the terminal via
//! ANSI on stdout and reads keystrokes from stdin — which the Swift session
//! forwards verbatim once it sees the alternate-screen enter sequence.
//!
//! vi keys — Normal mode: h/j/k/l + arrows move; w/b word; 0/$ line ends;
//! gg/G top/bottom; i/a/A/o/O enter Insert; x delete char; dd delete line;
//! yy yank; p paste; `:` command line (:w :q :wq :q! :w <file>). Insert mode:
//! type text, Esc back to Normal.

use std::io::{Read, Write};
use std::path::PathBuf;

use brush_core::{builtins, ExecutionContext, ExecutionResult, ShellExtensions};
use clap::Parser;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    Normal,
    Insert,
    Command,
}

/// Edit a text file in a full-screen editor.
#[derive(Parser)]
pub struct EditorCommand {
    /// File to edit (created on save if missing).
    file: Option<String>,
    /// Internal: start modeless in Insert mode (edit/nano) vs modal (vi).
    #[arg(skip)]
    pub modeless: bool,
}

impl builtins::Command for EditorCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let modeless = matches!(context.command_name.as_str(), "edit" | "nano");
        let path = self.file.as_ref().map(|f| {
            let p = PathBuf::from(f);
            if p.is_absolute() { p } else { cwd.join(p) }
        });

        // Terminal size from COLUMNS/LINES (the session sets them); default 24x80.
        let rows = std::env::var("LINES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24usize)
            .max(4);
        let cols = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80usize)
            .max(20);

        let mut ed = Editor::new(path, rows, cols, modeless);
        let mut out = context.stdout();
        let mut stdin = context.stdin();
        ed.run(&mut stdin, &mut out)?;
        Ok(ExecutionResult::success())
    }
}

struct Editor {
    lines: Vec<String>,
    cx: usize, // cursor column (char index within the line)
    cy: usize, // cursor row (line index)
    row_off: usize,
    rows: usize,
    cols: usize,
    path: Option<PathBuf>,
    dirty: bool,
    status: String,
    clipboard: Option<String>,
    quit: bool,
    mode: Mode,
    cmd: String,
    pending: Option<char>, // for 2-key normal commands: d, g, y
}

impl Editor {
    fn new(path: Option<PathBuf>, rows: usize, cols: usize, modeless: bool) -> Self {
        let lines = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| {
                let mut v: Vec<String> = s.split('\n').map(str::to_string).collect();
                if v.len() > 1 && v.last().is_some_and(String::is_empty) {
                    v.pop();
                }
                v
            })
            .unwrap_or_else(|| vec![String::new()]);
        let name = path
            .as_ref()
            .map_or_else(|| "[new]".to_string(), |p| p.display().to_string());
        Self {
            lines: if lines.is_empty() { vec![String::new()] } else { lines },
            cx: 0,
            cy: 0,
            row_off: 0,
            rows,
            cols,
            path,
            dirty: false,
            status: if modeless {
                format!("{name} — ^S save  ^Q quit (nano-style)")
            } else {
                format!("{name} — press i to insert, :w to save, :q to quit")
            },
            clipboard: None,
            quit: false,
            mode: if modeless { Mode::Insert } else { Mode::Normal },
            cmd: String::new(),
            pending: None,
        }
    }

    fn run(&mut self, stdin: &mut impl Read, out: &mut impl Write) -> std::io::Result<()> {
        // Enter alternate screen (also flips the Swift side into raw passthrough).
        write!(out, "\x1b[?1049h")?;
        self.render(out)?;
        out.flush()?;

        let mut buf = [0u8; 1];
        while !self.quit {
            let n = stdin.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.handle_key(buf[0], stdin, out)?;
            if !self.quit {
                self.render(out)?;
                out.flush()?;
            }
        }
        // Leave alternate screen.
        write!(out, "\x1b[?1049l")?;
        out.flush()?;
        Ok(())
    }

    fn handle_key(
        &mut self,
        b: u8,
        stdin: &mut impl Read,
        _out: &mut impl Write,
    ) -> std::io::Result<()> {
        match self.mode {
            Mode::Insert => self.handle_insert(b, stdin)?,
            Mode::Normal => self.handle_normal(b, stdin)?,
            Mode::Command => self.handle_command(b),
        }
        Ok(())
    }

    fn handle_insert(&mut self, b: u8, stdin: &mut impl Read) -> std::io::Result<()> {
        match b {
            0x1B => {
                // Esc: back to Normal (unless modeless/nano — Esc does nothing there)
                if self.status.contains("nano") {
                    // consume a possible arrow escape
                    self.handle_escape(stdin)?;
                } else {
                    self.mode = Mode::Normal;
                    if self.cx > 0 {
                        self.cx -= 1;
                    }
                    self.status = "-- NORMAL --".to_string();
                }
            }
            0x13 => self.save(), // Ctrl-S works in both styles
            0x11 | 0x18 if self.status.contains("nano") => {
                if self.dirty {
                    self.status = "^S to save, ^Q again to discard".to_string();
                    self.dirty = false;
                } else {
                    self.quit = true;
                }
            }
            0x0D | 0x0A => self.insert_newline(),
            0x7F | 0x08 => self.backspace(),
            0x00..=0x1F => {}
            _ => {
                let ch = self.read_utf8(b, stdin)?;
                self.insert_char(ch);
            }
        }
        Ok(())
    }

    fn handle_normal(&mut self, b: u8, stdin: &mut impl Read) -> std::io::Result<()> {
        // Two-key sequences (dd, gg, yy).
        if let Some(first) = self.pending.take() {
            let c = b as char;
            match (first, c) {
                ('d', 'd') => {
                    self.clipboard = Some(self.lines.remove(self.cy));
                    if self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    self.cy = self.cy.min(self.lines.len() - 1);
                    self.cx = 0;
                    self.dirty = true;
                }
                ('y', 'y') => {
                    self.clipboard = self.lines.get(self.cy).cloned();
                    self.status = "1 line yanked".to_string();
                }
                ('g', 'g') => {
                    self.cy = 0;
                    self.cx = 0;
                }
                _ => {}
            }
            return Ok(());
        }
        match b {
            b'h' => self.move_left(),
            b'j' => self.move_down(),
            b'k' => self.move_up(),
            b'l' => self.move_right(),
            b'0' => self.cx = 0,
            b'$' => self.cx = self.cur_len().saturating_sub(1),
            b'w' => self.word_forward(),
            b'b' => self.word_back(),
            b'G' => {
                self.cy = self.lines.len() - 1;
                self.cx = 0;
            }
            b'd' | b'g' | b'y' => self.pending = Some(b as char),
            b'x' => {
                let len = self.cur_len();
                if self.cx < len {
                    let line = &mut self.lines[self.cy];
                    let s = char_to_byte(line, self.cx);
                    let e = char_to_byte(line, self.cx + 1);
                    line.replace_range(s..e, "");
                    self.dirty = true;
                }
            }
            b'p' => {
                if let Some(c) = self.clipboard.clone() {
                    self.lines.insert(self.cy + 1, c);
                    self.cy += 1;
                    self.dirty = true;
                }
            }
            b'i' => self.enter_insert(),
            b'a' => {
                if self.cx < self.cur_len() {
                    self.cx += 1;
                }
                self.enter_insert();
            }
            b'A' => {
                self.cx = self.cur_len();
                self.enter_insert();
            }
            b'o' => {
                self.lines.insert(self.cy + 1, String::new());
                self.cy += 1;
                self.cx = 0;
                self.dirty = true;
                self.enter_insert();
            }
            b'O' => {
                self.lines.insert(self.cy, String::new());
                self.cx = 0;
                self.dirty = true;
                self.enter_insert();
            }
            b':' => {
                self.mode = Mode::Command;
                self.cmd = String::from(":");
            }
            0x1B => self.handle_escape(stdin)?, // arrow keys in Normal
            _ => {}
        }
        Ok(())
    }

    fn handle_command(&mut self, b: u8) {
        match b {
            0x0D | 0x0A => {
                let cmd = self.cmd.trim_start_matches(':').trim().to_string();
                self.mode = Mode::Normal;
                self.cmd.clear();
                self.run_ex(&cmd);
            }
            0x1B => {
                self.mode = Mode::Normal;
                self.cmd.clear();
                self.status = "-- NORMAL --".to_string();
            }
            0x7F | 0x08 => {
                self.cmd.pop();
                if self.cmd.is_empty() {
                    self.mode = Mode::Normal;
                }
            }
            0x20..=0x7E => self.cmd.push(b as char),
            _ => {}
        }
    }

    fn run_ex(&mut self, cmd: &str) {
        let force = cmd.ends_with('!');
        let base = cmd.trim_end_matches('!');
        // :w [file], :q, :wq, :x, :q!
        if let Some(file) = base.strip_prefix("w ").map(str::trim) {
            self.path = Some(PathBuf::from(file));
            self.save();
        } else if base == "w" {
            self.save();
        } else if base == "wq" || base == "x" {
            self.save();
            self.quit = true;
        } else if base == "q" {
            if self.dirty && !force {
                self.status = "unsaved changes — :q! to discard".to_string();
            } else {
                self.quit = true;
            }
        } else {
            self.status = format!("unknown command: {cmd}");
        }
    }

    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        self.status = "-- INSERT --".to_string();
    }

    fn word_forward(&mut self) {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() && self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        } else {
            self.cx = i.min(chars.len());
        }
    }

    fn word_back(&mut self) {
        if self.cx == 0 {
            return;
        }
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx.saturating_sub(1);
        while i > 0 && chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i -= 1;
        }
        while i > 0 && chars.get(i - 1).is_some_and(|c| !c.is_whitespace()) {
            i -= 1;
        }
        self.cx = i;
    }

    fn read_utf8(&self, lead: u8, stdin: &mut impl Read) -> std::io::Result<char> {
        let extra = match lead {
            0x00..=0x7F => 0,
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF7 => 3,
            _ => 0,
        };
        let mut bytes = vec![lead];
        let mut b = [0u8; 1];
        for _ in 0..extra {
            if stdin.read(&mut b)? == 1 {
                bytes.push(b[0]);
            }
        }
        Ok(String::from_utf8_lossy(&bytes).chars().next().unwrap_or('?'))
    }

    fn handle_escape(&mut self, stdin: &mut impl Read) -> std::io::Result<()> {
        let mut seq = [0u8; 2];
        if stdin.read(&mut seq[..1])? == 0 || seq[0] != b'[' {
            return Ok(());
        }
        if stdin.read(&mut seq[1..2])? == 0 {
            return Ok(());
        }
        match seq[1] {
            b'A' => self.move_up(),
            b'B' => self.move_down(),
            b'C' => self.move_right(),
            b'D' => self.move_left(),
            b'H' => self.cx = 0,
            b'F' => self.cx = self.cur_len(),
            _ => {}
        }
        Ok(())
    }

    fn cur_len(&self) -> usize {
        self.lines.get(self.cy).map_or(0, |l| l.chars().count())
    }

    fn move_up(&mut self) {
        if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.cx.min(self.cur_len());
        }
    }
    fn move_down(&mut self) {
        if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = self.cx.min(self.cur_len());
        }
    }
    fn move_left(&mut self) {
        if self.cx > 0 {
            self.cx -= 1;
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.cur_len();
        }
    }
    fn move_right(&mut self) {
        if self.cx < self.cur_len() {
            self.cx += 1;
        } else if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        }
    }

    fn insert_char(&mut self, ch: char) {
        let line = &mut self.lines[self.cy];
        let byte_idx = char_to_byte(line, self.cx);
        line.insert(byte_idx, ch);
        self.cx += 1;
        self.dirty = true;
    }

    fn insert_newline(&mut self) {
        let line = self.lines[self.cy].clone();
        let byte_idx = char_to_byte(&line, self.cx);
        let (left, right) = line.split_at(byte_idx);
        self.lines[self.cy] = left.to_string();
        self.lines.insert(self.cy + 1, right.to_string());
        self.cy += 1;
        self.cx = 0;
        self.dirty = true;
    }

    fn backspace(&mut self) {
        if self.cx > 0 {
            let line = &mut self.lines[self.cy];
            let byte_idx = char_to_byte(line, self.cx - 1);
            let end = char_to_byte(line, self.cx);
            line.replace_range(byte_idx..end, "");
            self.cx -= 1;
            self.dirty = true;
        } else if self.cy > 0 {
            let cur = self.lines.remove(self.cy);
            self.cy -= 1;
            self.cx = self.cur_len();
            self.lines[self.cy].push_str(&cur);
            self.dirty = true;
        }
    }

    fn save(&mut self) {
        if let Some(p) = &self.path {
            let content = self.lines.join("\n") + "\n";
            match std::fs::write(p, content) {
                Ok(()) => {
                    self.dirty = false;
                    self.status = format!("saved {}", p.display());
                }
                Err(e) => self.status = format!("save failed: {e}"),
            }
        } else {
            self.status = "no filename (open with `edit <file>`)".to_string();
        }
    }

    fn render(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        // Scroll to keep the cursor visible.
        let text_rows = self.rows.saturating_sub(1);
        if self.cy < self.row_off {
            self.row_off = self.cy;
        } else if self.cy >= self.row_off + text_rows {
            self.row_off = self.cy - text_rows + 1;
        }

        write!(out, "\x1b[H\x1b[2J")?; // home + clear
        for screen_row in 0..text_rows {
            let li = self.row_off + screen_row;
            if li < self.lines.len() {
                let line = &self.lines[li];
                let truncated: String = line.chars().take(self.cols).collect();
                write!(out, "{truncated}\r\n")?;
            } else {
                write!(out, "~\r\n")?;
            }
        }
        // Bottom line: the ex command line in Command mode, else the status bar.
        if self.mode == Mode::Command {
            let line: String = self.cmd.chars().take(self.cols).collect();
            write!(out, "{line}")?;
            // Cursor at the end of the command line.
            write!(out, "\x1b[{};{}H", self.rows, self.cmd.chars().count() + 1)?;
        } else {
            let dirty_mark = if self.dirty { "*" } else { " " };
            let status = format!(
                "{dirty_mark} {} [{}:{}]",
                self.status,
                self.cy + 1,
                self.cx + 1
            );
            let status: String = status.chars().take(self.cols).collect();
            write!(out, "\x1b[7m{status}\x1b[0m")?;
            // Position the cursor in the text area.
            let screen_y = self.cy - self.row_off + 1;
            let screen_x = self.cx.min(self.cols - 1) + 1;
            write!(out, "\x1b[{screen_y};{screen_x}H")?;
        }
        Ok(())
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or_else(|| s.len(), |(i, _)| i)
}
