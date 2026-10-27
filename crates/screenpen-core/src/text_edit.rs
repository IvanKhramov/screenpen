//! A text buffer with a caret and a selection, edited the way text fields are:
//! arrows, Home/End, word jumps with Ctrl, Shift extends the selection.
//! Offsets are byte indices on char boundaries.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextBuffer {
    text: String,
    caret: usize,
    /// The other end of the selection; equal to `caret` when nothing is selected.
    anchor: usize,
}

/// Where a caret movement goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Up and down keep the column in chars.
    Up,
    Down,
    LineStart,
    LineEnd,
    Start,
    End,
}

impl TextBuffer {
    /// The caret goes to the end.
    pub fn new(text: &str) -> TextBuffer {
        TextBuffer {
            text: text.to_string(),
            caret: text.len(),
            anchor: text.len(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected byte range, empty when nothing is selected.
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn selected(&self) -> Option<&str> {
        let r = self.selection();
        (!r.is_empty()).then(|| &self.text[r])
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Puts the caret at `at` (clamped to a char boundary); `extend` keeps the anchor.
    pub fn set_caret(&mut self, at: usize, extend: bool) {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.caret = at;
        if !extend {
            self.anchor = at;
        }
    }

    /// Selects the word (or the run of spaces or punctuation) around `at`.
    pub fn select_word(&mut self, at: usize) {
        self.set_caret(at, false);
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                0
            } else if c == '\n' {
                1
            } else if c.is_whitespace() {
                2
            } else {
                3
            }
        };
        let after = self.text[self.caret..].chars().next();
        let before = self.text[..self.caret].chars().next_back();
        let Some(kind) = after.or(before).map(class) else {
            return;
        };
        if kind == 1 {
            return;
        }
        let start = self.text[..self.caret]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| class(c) == kind)
            .last()
            .map_or(self.caret, |(i, _)| i);
        let end = self.text[self.caret..]
            .char_indices()
            .find(|&(_, c)| class(c) != kind)
            .map_or(self.text.len(), |(i, _)| self.caret + i);
        self.anchor = start;
        self.caret = end;
    }

    /// Replaces the selection with `s` (newlines kept, other control chars dropped).
    pub fn insert(&mut self, s: &str) {
        let clean: String = s
            .replace("\r\n", "\n")
            .chars()
            .map(|c| if c == '\r' { '\n' } else { c })
            .filter(|&c| c == '\n' || !c.is_control())
            .collect();
        let r = self.selection();
        self.text.replace_range(r.clone(), &clean);
        self.caret = r.start + clean.len();
        self.anchor = self.caret;
    }

    /// Removes and returns the selection.
    pub fn cut(&mut self) -> Option<String> {
        let s = self.selected()?.to_string();
        self.insert("");
        Some(s)
    }

    /// Backspace: the selection, or the char (word with `word`) before the caret.
    pub fn backspace(&mut self, word: bool) {
        if self.selection().is_empty() {
            self.anchor = if word {
                self.word_left(self.caret)
            } else {
                self.prev(self.caret)
            };
        }
        self.insert("");
    }

    /// Delete: the selection, or the char (word with `word`) after the caret.
    pub fn delete(&mut self, word: bool) {
        if self.selection().is_empty() {
            self.anchor = if word {
                self.word_right(self.caret)
            } else {
                self.next(self.caret)
            };
        }
        self.insert("");
    }

    /// Moves the caret; `word` jumps by words, `extend` grows the selection.
    /// Without `extend`, Left/Right on a selection collapse it to that side.
    pub fn motion(&mut self, motion: Motion, word: bool, extend: bool) {
        let r = self.selection();
        let to = match motion {
            Motion::Left if !extend && !r.is_empty() => r.start,
            Motion::Right if !extend && !r.is_empty() => r.end,
            Motion::Left if word => self.word_left(self.caret),
            Motion::Right if word => self.word_right(self.caret),
            Motion::Left => self.prev(self.caret),
            Motion::Right => self.next(self.caret),
            Motion::Up | Motion::Down => self.vertical(motion == Motion::Down),
            Motion::LineStart if word => 0,
            Motion::LineEnd if word => self.text.len(),
            Motion::LineStart => self.line_start(self.caret),
            Motion::LineEnd => self.line_end(self.caret),
            Motion::Start => 0,
            Motion::End => self.text.len(),
        };
        self.set_caret(to, extend);
    }

    /// Line index and byte offset within that line of `at`.
    pub fn line_col(&self, at: usize) -> (usize, usize) {
        let start = self.line_start(at);
        (self.text[..start].matches('\n').count(), at - start)
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at]
            .chars()
            .next_back()
            .map_or(0, |c| at - c.len_utf8())
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }

    /// Skips spaces, then a word (or a run of punctuation), backwards.
    fn word_left(&self, at: usize) -> usize {
        let mut chars = self.text[..at].char_indices().rev().peekable();
        while chars.next_if(|&(_, c)| c.is_whitespace()).is_some() {}
        let Some(&(_, first)) = chars.peek() else {
            return 0;
        };
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let kind = word(first);
        let mut to = at;
        while let Some((i, _)) = chars.next_if(|&(_, c)| !c.is_whitespace() && word(c) == kind) {
            to = i;
        }
        if to == at { self.prev(at) } else { to }
    }

    /// Skips a word (or a run of punctuation), then spaces, forwards.
    fn word_right(&self, at: usize) -> usize {
        let rest = &self.text[at..];
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut chars = rest.char_indices().peekable();
        if let Some(&(_, first)) = chars.peek()
            && !first.is_whitespace()
        {
            let kind = word(first);
            while chars
                .next_if(|&(_, c)| !c.is_whitespace() && word(c) == kind)
                .is_some()
            {}
        }
        while chars
            .next_if(|&(_, c)| c.is_whitespace() && c != '\n')
            .is_some()
        {}
        let end = chars.peek().map_or(rest.len(), |&(i, _)| i);
        if end == 0 { self.next(at) } else { at + end }
    }

    fn vertical(&self, down: bool) -> usize {
        let start = self.line_start(self.caret);
        let column = self.text[start..self.caret].chars().count();
        let target = if down {
            let end = self.line_end(self.caret);
            if end == self.text.len() {
                return self.text.len();
            }
            end + 1
        } else {
            if start == 0 {
                return 0;
            }
            self.line_start(start - 1)
        };
        let line = &self.text[target..self.line_end(target)];
        target
            + line
                .char_indices()
                .nth(column)
                .map_or(line.len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, caret: usize) -> TextBuffer {
        let mut b = TextBuffer::new(text);
        b.set_caret(caret, false);
        b
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut b = TextBuffer::new("hello world");
        b.select_all();
        assert_eq!(b.selected(), Some("hello world"));
        b.insert("hi");
        assert_eq!((b.text(), b.caret()), ("hi", 2));
    }

    #[test]
    fn insert_normalizes_newlines_and_drops_controls() {
        let mut b = TextBuffer::new("");
        b.insert("a\r\nb\rc\u{1}\td");
        assert_eq!(b.text(), "a\nb\ncd");
    }

    #[test]
    fn arrows_walk_chars_and_collapse_a_selection() {
        let mut b = at("añb", 1);
        b.motion(Motion::Right, false, false);
        assert_eq!(b.caret(), 3);
        b.motion(Motion::Left, false, true);
        assert_eq!(b.selected(), Some("ñ"));
        b.motion(Motion::Right, false, false);
        assert_eq!((b.caret(), b.selected()), (3, None));
    }

    #[test]
    fn word_jumps_and_word_deletes() {
        let mut b = TextBuffer::new("one two, three");
        b.motion(Motion::Left, true, false);
        assert_eq!(b.caret(), 9);
        b.motion(Motion::Left, true, false);
        assert_eq!(b.caret(), 7);
        b.motion(Motion::Left, true, false);
        assert_eq!(b.caret(), 4);
        b.motion(Motion::Right, true, false);
        assert_eq!(b.caret(), 7);
        let mut b = TextBuffer::new("one two");
        b.backspace(true);
        assert_eq!(b.text(), "one ");
        b.backspace(true);
        assert_eq!(b.text(), "");
        let mut b = at("one two", 0);
        b.delete(true);
        assert_eq!(b.text(), "two");
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut b = at("abcd\nx\nabcd", 3);
        b.motion(Motion::Down, false, false);
        assert_eq!(b.line_col(b.caret()), (1, 1));
        b.motion(Motion::Down, false, false);
        assert_eq!(b.line_col(b.caret()), (2, 1));
        b.motion(Motion::Down, false, false);
        assert_eq!(b.caret(), b.text().len());
        b.motion(Motion::Up, false, false);
        b.motion(Motion::Up, false, false);
        b.motion(Motion::Up, false, false);
        assert_eq!(b.caret(), 0);
    }

    #[test]
    fn home_end_and_word_select() {
        let mut b = at("ab\ncd ef", 5);
        b.motion(Motion::LineStart, false, true);
        assert_eq!(b.selected(), Some("cd"));
        b.motion(Motion::LineEnd, false, false);
        assert_eq!(b.caret(), 8);
        b.select_word(4);
        assert_eq!(b.selected(), Some("cd"));
        b.select_word(8);
        assert_eq!(b.selected(), Some("ef"));
        assert_eq!(b.cut().as_deref(), Some("ef"));
        assert_eq!(b.text(), "ab\ncd ");
    }

    #[test]
    fn backspace_at_start_does_nothing() {
        let mut b = at("ab", 0);
        b.backspace(false);
        b.backspace(true);
        assert_eq!((b.text(), b.caret()), ("ab", 0));
    }
}
