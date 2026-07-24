//! Full-screen text editors: `vi` (modal — Normal/Insert/Command, like vi)
//! and `edit`/`nano` (the same engine started in Insert mode, modeless-ish).
//!
//! No embeddable Rust editor library fits our byte-stream model (they assume
//! termios + /dev/tty), so this is self-contained. It drives the terminal via
//! ANSI on stdout and reads keystrokes from stdin — which the Swift session
//! forwards verbatim once it sees the alternate-screen enter sequence.
//!
//! vi keys — Normal mode motions: h/j/k/l + arrows; w/b word; 0/^/$ line ends;
//! gg/G top/bottom; a numeric count prefix repeats them (5j, 10G). Edits:
//! i/I/a/A/o/O/s enter Insert; x/X delete char; r replace char; ~ toggle case;
//! D/C to end of line; J join; dd/dw/cc/cw operators; yy yank; p/P paste;
//! u undo, Ctrl-R redo. Search: /pattern then n/N. `:` command line
//! (:w :q :wq :q! :w <file>). Files with a known extension get syntax
//! highlighting (keywords/strings/numbers/comments) in the text area.

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
    pending: Option<char>, // for 2-key normal commands: d, g, y, c, r
    count: Option<usize>,  // numeric prefix accumulator (5j, 10dd, 3G)
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_search: String,
    filetype: Option<Ft>,
}

/// A point-in-time buffer state for undo/redo.
type Snapshot = (Vec<String>, usize, usize);

/// Syntax profile for the open file, chosen by extension.
#[derive(Clone, Copy)]
struct Ft {
    line_comment: &'static str,
    keywords: &'static [&'static str],
    backtick: bool,
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
        let filetype = path.as_ref().and_then(|p| detect_ft(p));
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
            count: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_search: String::new(),
            filetype,
        }
    }

    /// Snapshot the buffer before a mutation so `u` can revert it.
    fn push_undo(&mut self) {
        self.undo.push((self.lines.clone(), self.cy, self.cx));
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn do_undo(&mut self) {
        if let Some((lines, cy, cx)) = self.undo.pop() {
            self.redo.push((self.lines.clone(), self.cy, self.cx));
            self.lines = lines;
            self.cy = cy.min(self.lines.len().saturating_sub(1));
            self.cx = cx.min(self.cur_len());
            self.dirty = true;
            self.status = "undo".to_string();
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    fn do_redo(&mut self) {
        if let Some((lines, cy, cx)) = self.redo.pop() {
            self.undo.push((self.lines.clone(), self.cy, self.cx));
            self.lines = lines;
            self.cy = cy.min(self.lines.len().saturating_sub(1));
            self.cx = cx.min(self.cur_len());
            self.dirty = true;
            self.status = "redo".to_string();
        } else {
            self.status = "already at newest change".to_string();
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
        // Two-key sequences (dd, dw, cc, cw, gg, yy, r<char>).
        if let Some(first) = self.pending.take() {
            let c = b as char;
            let n = self.count.take().unwrap_or(1);
            match (first, c) {
                ('r', _) if b >= 0x20 => {
                    // Replace the char under the cursor with the next keystroke.
                    let ch = self.read_utf8(b, stdin)?;
                    if self.cx < self.cur_len() {
                        self.push_undo();
                        let line = &mut self.lines[self.cy];
                        let s = char_to_byte(line, self.cx);
                        let e = char_to_byte(line, self.cx + 1);
                        line.replace_range(s..e, &ch.to_string());
                        self.dirty = true;
                    }
                }
                ('d', 'd') => {
                    self.push_undo();
                    self.delete_lines(n);
                }
                ('d', 'w') => {
                    self.push_undo();
                    self.delete_word();
                }
                ('c', 'c') => {
                    self.push_undo();
                    self.lines[self.cy].clear();
                    self.cx = 0;
                    self.enter_insert();
                }
                ('c', 'w') => {
                    self.push_undo();
                    self.delete_word();
                    self.enter_insert();
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
        // Numeric count prefix: digits accumulate (but a leading 0 is the "start
        // of line" motion, not a count).
        if b.is_ascii_digit() && !(b == b'0' && self.count.is_none()) {
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10) + (b - b'0') as usize);
            return Ok(());
        }
        let n = self.count.take().unwrap_or(1);
        match b {
            b'h' => self.repeat(n, Self::move_left),
            b'j' => self.repeat(n, Self::move_down),
            b'k' => self.repeat(n, Self::move_up),
            b'l' => self.repeat(n, Self::move_right),
            b'0' => self.cx = 0,
            b'^' => self.cx = self.first_non_blank(),
            b'$' => self.cx = self.cur_len().saturating_sub(1),
            b'w' => self.repeat(n, Self::word_forward),
            b'b' => self.repeat(n, Self::word_back),
            b'G' => {
                // With a count, jump to that line; otherwise last line.
                self.cy = if self.count_was(n) {
                    (n - 1).min(self.lines.len() - 1)
                } else {
                    self.lines.len() - 1
                };
                self.cx = 0;
            }
            b'd' | b'g' | b'y' | b'c' | b'r' => {
                self.count = if n == 1 { None } else { Some(n) }; // preserve for the pair
                self.pending = Some(b as char);
            }
            b'x' => {
                self.push_undo();
                for _ in 0..n {
                    let len = self.cur_len();
                    if self.cx < len {
                        let line = &mut self.lines[self.cy];
                        let s = char_to_byte(line, self.cx);
                        let e = char_to_byte(line, self.cx + 1);
                        line.replace_range(s..e, "");
                        self.dirty = true;
                    }
                }
            }
            b'X' => {
                self.push_undo();
                for _ in 0..n {
                    self.backspace();
                }
            }
            b'D' => {
                self.push_undo();
                let s = char_to_byte(&self.lines[self.cy], self.cx);
                self.lines[self.cy].truncate(s);
                self.dirty = true;
            }
            b'C' => {
                self.push_undo();
                let s = char_to_byte(&self.lines[self.cy], self.cx);
                self.lines[self.cy].truncate(s);
                self.dirty = true;
                self.enter_insert();
            }
            b'J' => {
                self.push_undo();
                self.join_line();
            }
            b'~' => {
                self.push_undo();
                self.toggle_case();
            }
            b'p' => {
                if let Some(c) = self.clipboard.clone() {
                    self.push_undo();
                    self.lines.insert(self.cy + 1, c);
                    self.cy += 1;
                    self.dirty = true;
                }
            }
            b'P' => {
                if let Some(c) = self.clipboard.clone() {
                    self.push_undo();
                    self.lines.insert(self.cy, c);
                    self.dirty = true;
                }
            }
            b'u' => self.do_undo(),
            0x12 => self.do_redo(), // Ctrl-R
            b'i' => {
                self.push_undo();
                self.enter_insert();
            }
            b'I' => {
                self.push_undo();
                self.cx = self.first_non_blank();
                self.enter_insert();
            }
            b's' => {
                self.push_undo();
                let len = self.cur_len();
                if self.cx < len {
                    let line = &mut self.lines[self.cy];
                    let s = char_to_byte(line, self.cx);
                    let e = char_to_byte(line, self.cx + 1);
                    line.replace_range(s..e, "");
                }
                self.enter_insert();
            }
            b'a' => {
                self.push_undo();
                if self.cx < self.cur_len() {
                    self.cx += 1;
                }
                self.enter_insert();
            }
            b'A' => {
                self.push_undo();
                self.cx = self.cur_len();
                self.enter_insert();
            }
            b'o' => {
                self.push_undo();
                self.lines.insert(self.cy + 1, String::new());
                self.cy += 1;
                self.cx = 0;
                self.dirty = true;
                self.enter_insert();
            }
            b'O' => {
                self.push_undo();
                self.lines.insert(self.cy, String::new());
                self.cx = 0;
                self.dirty = true;
                self.enter_insert();
            }
            b'/' => {
                self.mode = Mode::Command;
                self.cmd = String::from("/");
            }
            b'n' => self.search_repeat(true),
            b'N' => self.search_repeat(false),
            b':' => {
                self.mode = Mode::Command;
                self.cmd = String::from(":");
            }
            0x1B => self.handle_escape(stdin)?, // arrow keys in Normal
            _ => {}
        }
        Ok(())
    }

    /// Was the count explicitly typed (vs the default of 1)? Used by `G`.
    fn count_was(&self, n: usize) -> bool {
        n != 1 || self.count.is_some()
    }

    fn repeat(&mut self, n: usize, f: fn(&mut Self)) {
        for _ in 0..n {
            f(self);
        }
    }

    fn first_non_blank(&self) -> usize {
        self.lines[self.cy]
            .chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(0)
    }

    fn delete_lines(&mut self, n: usize) {
        let mut yanked = Vec::new();
        for _ in 0..n {
            if self.cy < self.lines.len() {
                yanked.push(self.lines.remove(self.cy));
            }
            if self.lines.is_empty() {
                self.lines.push(String::new());
                break;
            }
        }
        self.clipboard = Some(yanked.join("\n"));
        self.cy = self.cy.min(self.lines.len() - 1);
        self.cx = 0;
        self.dirty = true;
    }

    fn delete_word(&mut self) {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let s = char_to_byte(&self.lines[self.cy], self.cx);
        let e = char_to_byte(&self.lines[self.cy], i);
        self.lines[self.cy].replace_range(s..e, "");
        self.dirty = true;
    }

    fn join_line(&mut self) {
        if self.cy + 1 < self.lines.len() {
            let next = self.lines.remove(self.cy + 1);
            let trimmed = next.trim_start();
            let cur = &mut self.lines[self.cy];
            if !cur.is_empty() && !cur.ends_with(' ') && !trimmed.is_empty() {
                cur.push(' ');
            }
            cur.push_str(trimmed);
            self.dirty = true;
        }
    }

    fn toggle_case(&mut self) {
        if self.cx < self.cur_len() {
            let chars: Vec<char> = self.lines[self.cy].chars().collect();
            let c = chars[self.cx];
            let flipped: String = if c.is_uppercase() {
                c.to_lowercase().collect()
            } else {
                c.to_uppercase().collect()
            };
            let s = char_to_byte(&self.lines[self.cy], self.cx);
            let e = char_to_byte(&self.lines[self.cy], self.cx + 1);
            self.lines[self.cy].replace_range(s..e, &flipped);
            if self.cx + 1 <= self.cur_len() {
                self.cx += 1;
            }
            self.dirty = true;
        }
    }

    /// Search for `last_search` starting just after the cursor; `forward`
    /// controls direction, and the scan wraps around the buffer.
    fn search_repeat(&mut self, forward: bool) {
        if self.last_search.is_empty() {
            self.status = "no previous search".to_string();
            return;
        }
        let pat = self.last_search.clone();
        let total = self.lines.len();
        // Build a flat list of (line, byte-col) candidates by scanning outward.
        for step in 1..=total {
            let li = if forward {
                (self.cy + step) % total
            } else {
                (self.cy + total - step) % total
            };
            if let Some(col) = self.lines[li].find(&pat) {
                let cx = self.lines[li][..col].chars().count();
                self.cy = li;
                self.cx = cx;
                self.status = format!("/{pat}");
                return;
            }
        }
        // Also check the current line after (forward) / before (back) the cursor.
        self.status = format!("pattern not found: {pat}");
    }

    fn handle_command(&mut self, b: u8) {
        match b {
            0x0D | 0x0A => {
                let entry = std::mem::take(&mut self.cmd);
                self.mode = Mode::Normal;
                if let Some(pat) = entry.strip_prefix('/') {
                    // `/pattern`: record and jump to the first forward match.
                    if !pat.is_empty() {
                        self.last_search = pat.to_string();
                    }
                    self.search_repeat(true);
                } else {
                    let cmd = entry.trim_start_matches(':').trim().to_string();
                    self.run_ex(&cmd);
                }
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
                let painted = self.highlight_line(&truncated);
                write!(out, "{painted}\r\n")?;
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

impl Editor {
    /// Colorize one already-width-truncated line for the text area. Returns the
    /// line unchanged when the file has no known syntax profile. Uses a simple
    /// single-line tokenizer (no multi-line strings/comments), which is enough
    /// to read code at a glance without a full parser.
    fn highlight_line(&self, s: &str) -> String {
        let Some(ft) = self.filetype else {
            return s.to_string();
        };
        const GREY: &str = "\x1b[38;5;245m"; // comments
        const GREEN: &str = "\x1b[38;5;114m"; // strings
        const BLUE: &str = "\x1b[38;5;75m"; // keywords
        const YELLOW: &str = "\x1b[38;5;179m"; // numbers
        const RESET: &str = "\x1b[0m";

        let chars: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            // Line comment to end of line.
            if !ft.line_comment.is_empty() && starts_with_at(&chars, i, ft.line_comment) {
                out.push_str(GREY);
                out.extend(chars[i..].iter());
                out.push_str(RESET);
                return out;
            }
            // String literal (single line).
            if c == '"' || c == '\'' || (ft.backtick && c == '`') {
                let quote = c;
                out.push_str(GREEN);
                out.push(c);
                i += 1;
                while i < chars.len() {
                    let d = chars[i];
                    out.push(d);
                    i += 1;
                    if d == '\\' && i < chars.len() {
                        out.push(chars[i]);
                        i += 1;
                        continue;
                    }
                    if d == quote {
                        break;
                    }
                }
                out.push_str(RESET);
                continue;
            }
            // Number.
            if c.is_ascii_digit() {
                out.push_str(YELLOW);
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
                {
                    out.push(chars[i]);
                    i += 1;
                }
                out.push_str(RESET);
                continue;
            }
            // Identifier / keyword.
            if c.is_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if ft.keywords.contains(&word.as_str()) {
                    out.push_str(BLUE);
                    out.push_str(&word);
                    out.push_str(RESET);
                } else {
                    out.push_str(&word);
                }
                continue;
            }
            out.push(c);
            i += 1;
        }
        out
    }
}

fn starts_with_at(chars: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    i + p.len() <= chars.len() && chars[i..i + p.len()] == p[..]
}

const KW_RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "union",
    "unsafe", "use", "where", "while",
];
const KW_PY: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield",
];
const KW_JS: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete",
    "do", "else", "export", "extends", "false", "finally", "for", "function", "if", "import", "in",
    "instanceof", "let", "new", "null", "of", "return", "super", "switch", "this", "throw", "true",
    "try", "typeof", "var", "void", "while", "yield",
];
const KW_C: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum",
    "extern", "float", "for", "goto", "if", "int", "long", "register", "return", "short", "signed",
    "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile",
    "while", "bool", "class", "namespace", "new", "delete", "public", "private", "template",
];
const KW_GO: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for",
    "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select",
    "struct", "switch", "type", "var", "nil", "true", "false",
];
const KW_SH: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in",
    "function", "select", "return", "export", "local", "readonly", "declare", "echo", "cd", "exit",
];

/// Pick a syntax profile from the file extension.
fn detect_ft(path: &std::path::Path) -> Option<Ft> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    let (line_comment, keywords, backtick): (&str, &[&str], bool) = match ext.as_str() {
        "rs" => ("//", KW_RUST, false),
        "py" | "pyw" => ("#", KW_PY, false),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => ("//", KW_JS, true),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "java" | "swift" | "kt" => ("//", KW_C, false),
        "go" => ("//", KW_GO, true),
        "sh" | "bash" | "zsh" | "profile" | "bashrc" | "zshrc" => ("#", KW_SH, false),
        "json" | "toml" | "yaml" | "yml" | "cfg" | "ini" | "conf" => ("#", &[], false),
        _ => return None,
    };
    Some(Ft {
        line_comment,
        keywords,
        backtick,
    })
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or_else(|| s.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a Normal-mode editor seeded with the given lines.
    fn ed(lines: &[&str]) -> Editor {
        let mut e = Editor::new(Some(PathBuf::from("t.rs")), 24, 80, false);
        e.lines = lines.iter().map(|s| (*s).to_string()).collect();
        e.cx = 0;
        e.cy = 0;
        e.dirty = false;
        e
    }

    /// Replay a byte stream through the same per-key loop `run()` uses.
    fn drive(e: &mut Editor, keys: &[u8]) {
        let mut cur = Cursor::new(keys.to_vec());
        let mut sink: Vec<u8> = Vec::new();
        let mut b = [0u8; 1];
        while Read::read(&mut cur, &mut b).unwrap() == 1 {
            e.handle_key(b[0], &mut cur, &mut sink).unwrap();
        }
    }

    const ESC: u8 = 0x1B;
    const LF: u8 = 0x0A;

    #[test]
    fn dw_tilde_append() {
        let mut e = ed(&["let x = 42"]);
        // ^ (bol), dw -> drop "let ", ~ -> X, A + " END" + Esc
        let mut keys = vec![b'^', b'd', b'w', b'~', b'A', b' ', b'E', b'N', b'D'];
        keys.push(ESC);
        drive(&mut e, &keys);
        assert_eq!(e.lines, vec!["X = 42 END"]);
    }

    #[test]
    fn dd_then_undo_restores() {
        let mut e = ed(&["a", "b", "c"]);
        drive(&mut e, b"dddd"); // delete "a", then "b" -> ["c"]
        assert_eq!(e.lines, vec!["c"]);
        drive(&mut e, b"u"); // undo one dd -> ["b","c"]
        assert_eq!(e.lines, vec!["b", "c"]);
    }

    #[test]
    fn count_prefixed_dd() {
        let mut e = ed(&["a", "b", "c", "d"]);
        drive(&mut e, b"2dd");
        assert_eq!(e.lines, vec!["c", "d"]);
    }

    #[test]
    fn count_prefixed_motion() {
        let mut e = ed(&["l0", "l1", "l2", "l3", "l4"]);
        drive(&mut e, b"3j");
        assert_eq!(e.cy, 3);
    }

    #[test]
    fn search_jumps_to_match() {
        let mut e = ed(&["foo", "bar", "baz"]);
        let mut keys = b"/bar".to_vec();
        keys.push(LF);
        drive(&mut e, &keys);
        assert_eq!(e.cy, 1);
        assert_eq!(e.cx, 0);
    }

    #[test]
    fn join_lines() {
        let mut e = ed(&["foo", "   bar"]);
        drive(&mut e, b"J");
        assert_eq!(e.lines, vec!["foo bar"]);
    }

    #[test]
    fn delete_to_eol() {
        let mut e = ed(&["hello world"]);
        drive(&mut e, b"wD"); // w -> col6, D truncates
        assert_eq!(e.lines, vec!["hello "]);
    }

    #[test]
    fn replace_char() {
        let mut e = ed(&["cat"]);
        drive(&mut e, b"rb"); // replace 'c' with 'b'
        assert_eq!(e.lines, vec!["bat"]);
    }

    #[test]
    fn redo_after_undo() {
        let mut e = ed(&["a", "b"]);
        drive(&mut e, b"dd"); // ["b"]
        drive(&mut e, b"u"); // ["a","b"]
        drive(&mut e, &[0x12]); // Ctrl-R redo -> ["b"]
        assert_eq!(e.lines, vec!["b"]);
    }

    #[test]
    fn highlight_emits_ansi_for_known_ext() {
        let e = ed(&["let x = 42"]);
        let painted = e.highlight_line("let x = 42");
        assert!(painted.contains("\x1b[38;5;75m"), "keyword color");
        assert!(painted.contains("\x1b[38;5;179m"), "number color");
    }

    #[test]
    fn highlight_noop_for_unknown_ext() {
        let mut e = Editor::new(Some(PathBuf::from("data.unknownext")), 24, 80, false);
        e.lines = vec!["let x = 42".to_string()];
        assert_eq!(e.highlight_line("let x = 42"), "let x = 42");
    }
}
