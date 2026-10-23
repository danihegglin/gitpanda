//! A small text field (single or multi-line). It consumes editing keys and
//! lets Enter, Escape and ⌘-shortcuts bubble up to the owning view, which
//! decides what submitting or cancelling means.

use gpui::{
    ClipboardItem, Context, FocusHandle, KeyDownEvent, MouseButton, SharedString, Window, div,
    prelude::*, px,
};

use super::theme::*;

pub struct TextInput {
    pub focus: FocusHandle,
    text: String,
    cursor: usize,
    all_selected: bool,
    multiline: bool,
    placeholder: SharedString,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>, multiline: bool) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            cursor: 0,
            all_selected: false,
            multiline,
            placeholder: placeholder.into(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.all_selected = false;
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.all_selected = !self.text.is_empty();
        self.cursor = self.text.len();
        cx.notify();
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..].chars().next().map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    fn word_left(&self) -> usize {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c: char| !is_word(c));
        trimmed.char_indices().rev().find(|&(_, c)| !is_word(c)).map_or(0, |(i, c)| i + c.len_utf8())
    }

    fn word_right(&self) -> usize {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let rest = &self.text[self.cursor..];
        let skip = rest.char_indices().skip_while(|&(_, c)| !is_word(c)).find(|&(_, c)| !is_word(c));
        skip.map_or(self.text.len(), |(i, _)| self.cursor + i)
    }

    /// Move vertically by one line, keeping the column in chars.
    fn vertical(&self, down: bool) -> Option<usize> {
        let start = self.line_start();
        let col = self.text[start..self.cursor].chars().count();
        let (s, e) = if down {
            let end = self.line_end();
            if end == self.text.len() {
                return None;
            }
            let s = end + 1;
            (s, self.text[s..].find('\n').map_or(self.text.len(), |i| s + i))
        } else {
            if start == 0 {
                return None;
            }
            let e = start - 1;
            (self.text[..e].rfind('\n').map_or(0, |i| i + 1), e)
        };
        let line = &self.text[s..e];
        Some(s + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i))
    }

    fn replace_selection(&mut self) {
        if self.all_selected {
            self.text.clear();
            self.cursor = 0;
            self.all_selected = false;
        }
    }

    fn insert(&mut self, s: &str) {
        self.replace_selection();
        let s = if self.multiline { s.to_string() } else { s.replace(['\n', '\r'], " ") };
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let cmd = m.platform || m.control;
        let key = ks.key.as_str();
        let handled = match key {
            "escape" | "tab" => false,
            "enter" if cmd || !self.multiline => false,
            "enter" => {
                self.insert("\n");
                true
            }
            "backspace" => {
                if self.all_selected {
                    self.replace_selection();
                } else if cmd {
                    let s = self.line_start();
                    let s = if s == self.cursor { self.prev() } else { s };
                    self.text.replace_range(s..self.cursor, "");
                    self.cursor = s;
                } else {
                    let p = if m.alt { self.word_left() } else { self.prev() };
                    self.text.replace_range(p..self.cursor, "");
                    self.cursor = p;
                }
                true
            }
            "delete" => {
                if self.all_selected {
                    self.replace_selection();
                } else {
                    let n = self.next();
                    self.text.replace_range(self.cursor..n, "");
                }
                true
            }
            "left" => {
                self.cursor = if cmd { self.line_start() } else if m.alt { self.word_left() } else { self.prev() };
                true
            }
            "right" => {
                self.cursor = if cmd { self.line_end() } else if m.alt { self.word_right() } else { self.next() };
                true
            }
            "up" if self.multiline => match self.vertical(false) {
                Some(p) => {
                    self.cursor = p;
                    true
                }
                None => {
                    self.cursor = 0;
                    true
                }
            },
            "down" if self.multiline => match self.vertical(true) {
                Some(p) => {
                    self.cursor = p;
                    true
                }
                None => {
                    self.cursor = self.text.len();
                    true
                }
            },
            "home" => {
                self.cursor = self.line_start();
                true
            }
            "end" => {
                self.cursor = self.line_end();
                true
            }
            "a" if cmd => {
                self.all_selected = !self.text.is_empty();
                self.cursor = self.text.len();
                cx.notify();
                return cx.stop_propagation();
            }
            "c" | "x" if cmd => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
                if key == "x" {
                    self.text.clear();
                    self.cursor = 0;
                }
                true
            }
            "v" if cmd => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&t);
                }
                true
            }
            _ if cmd => false,
            _ => match &ks.key_char {
                Some(ch) if !ch.chars().any(char::is_control) => {
                    self.insert(ch);
                    true
                }
                _ => false,
            },
        };
        if handled {
            if !matches!(key, "a") {
                self.all_selected = false;
            }
            cx.stop_propagation();
            cx.notify();
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let caret = || div().w(px(2.)).h(px(16.)).mt(px(1.)).bg(c(BLUE)).rounded(px(1.));
        let mut body = div().flex().flex_col().w_full();
        if self.text.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .h(px(20.))
                    .items_center()
                    .when(focused, |d| d.child(caret()))
                    .child(div().text_color(c(COMMENT)).child(self.placeholder.clone())),
            );
        } else {
            let mut offset = 0;
            for line in self.text.split('\n') {
                let start = offset;
                let end = start + line.len();
                offset = end + 1;
                let mut row = div().flex().h(px(20.)).items_center().whitespace_nowrap();
                if self.all_selected {
                    row = row.child(div().bg(c(BG_SELECT)).child(SharedString::from(line.to_string())));
                } else if focused && self.cursor >= start && self.cursor <= end {
                    let at = self.cursor - start;
                    let (a, b) = line.split_at(at);
                    if !a.is_empty() {
                        row = row.child(SharedString::from(a.to_string()));
                    }
                    row = row.child(caret());
                    if !b.is_empty() {
                        row = row.child(SharedString::from(b.to_string()));
                    }
                } else {
                    row = row.child(SharedString::from(line.to_string()));
                }
                body = body.child(row);
            }
        }
        div()
            .id("text-input")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus);
                    cx.notify();
                }),
            )
            .w_full()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(c(BG_DARKER))
            .border_1()
            .border_color(if focused { c(BLUE0) } else { c(BORDER_HI) })
            .text_color(c(FG))
            .text_size(px(13.))
            .cursor_text()
            .when(self.multiline, |d| d.min_h(px(84.)).overflow_hidden())
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

    use super::*;

    /// A focused input in its own window.
    fn input(cx: &mut TestAppContext, multiline: bool) -> (Entity<TextInput>, &mut VisualTestContext) {
        cx.add_window_view(|window, cx| {
            let i = TextInput::new(cx, "", multiline);
            window.focus(&i.focus);
            i
        })
    }

    /// Runs `f` on an input holding `text`, with the cursor at `|`.
    fn at<R>(cx: &mut TestAppContext, text: &str, f: impl FnOnce(&TextInput) -> R) -> R {
        let i = cx.new(|cx| TextInput::new(cx, "", true));
        i.update(cx, |i, _| {
            i.cursor = text.find('|').expect("cursor marker");
            i.text = text.replacen('|', "", 1);
            f(i)
        })
    }

    /// The input's text with `|` at the cursor.
    fn shown(i: &Entity<TextInput>, cx: &mut VisualTestContext) -> String {
        i.read_with(cx, |i, _| {
            let mut t = i.text.clone();
            t.insert(i.cursor, '|');
            t
        })
    }

    #[gpui::test]
    fn moves_by_character_over_multibyte_text(cx: &mut TestAppContext) {
        assert_eq!(at(cx, "aé|b", |i| i.prev()), 1);
        assert_eq!(at(cx, "a|éb", |i| i.next()), 3);
        assert_eq!(at(cx, "|ab", |i| i.prev()), 0);
        assert_eq!(at(cx, "ab|", |i| i.next()), 2);
    }

    #[gpui::test]
    fn moves_by_word(cx: &mut TestAppContext) {
        assert_eq!(at(cx, "foo bar_baz|", |i| i.word_left()), 4);
        assert_eq!(at(cx, "foo bar  |", |i| i.word_left()), 4);
        assert_eq!(at(cx, "foo|", |i| i.word_left()), 0);
        assert_eq!(at(cx, "|foo bar", |i| i.word_right()), 3);
        assert_eq!(at(cx, "foo| bar", |i| i.word_right()), 7);
        assert_eq!(at(cx, "foo bar|", |i| i.word_right()), 7);
    }

    #[gpui::test]
    fn finds_line_bounds(cx: &mut TestAppContext) {
        assert_eq!(at(cx, "one\ntw|o\nthree", |i| (i.line_start(), i.line_end())), (4, 7));
        assert_eq!(at(cx, "o|ne", |i| (i.line_start(), i.line_end())), (0, 3));
    }

    #[gpui::test]
    fn moves_vertically_keeping_the_column(cx: &mut TestAppContext) {
        assert_eq!(at(cx, "abcd\nab|cd", |i| i.vertical(false)), Some(2));
        assert_eq!(at(cx, "ab|cd\nabcd", |i| i.vertical(true)), Some(7));
        // A shorter line clamps the column to its end.
        assert_eq!(at(cx, "abcd|\nab\nabcd", |i| i.vertical(true)), Some(7));
        // Columns count characters, not bytes.
        assert_eq!(at(cx, "éé|é\nabc", |i| i.vertical(true)), Some(9));
        assert_eq!(at(cx, "ab|c", |i| i.vertical(false)), None);
        assert_eq!(at(cx, "ab|c", |i| i.vertical(true)), None);
    }

    #[gpui::test]
    fn types_and_edits(cx: &mut TestAppContext) {
        let (i, cx) = input(cx, false);
        cx.simulate_input("hello world");
        assert_eq!(shown(&i, cx), "hello world|");
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(shown(&i, cx), "hello |");
        cx.simulate_keystrokes("left left backspace");
        assert_eq!(shown(&i, cx), "hel|o ");
        cx.simulate_keystrokes("delete");
        assert_eq!(shown(&i, cx), "hel| ");
        cx.simulate_keystrokes("home");
        assert_eq!(shown(&i, cx), "|hel ");
        cx.simulate_keystrokes("end");
        assert_eq!(shown(&i, cx), "hel |");
        cx.simulate_keystrokes("cmd-backspace");
        assert_eq!(shown(&i, cx), "|");
    }

    #[gpui::test]
    fn single_line_input_flattens_newlines(cx: &mut TestAppContext) {
        let (i, cx) = input(cx, false);
        i.update(cx, |i, _| i.insert("a\nb\r\nc"));
        assert_eq!(shown(&i, cx), "a b  c|");
    }

    #[gpui::test]
    fn enter_adds_a_line_only_when_multiline(cx: &mut TestAppContext) {
        let (i, cx) = input(cx, true);
        cx.simulate_input("ab");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("cd");
        assert_eq!(shown(&i, cx), "ab\ncd|");
        cx.simulate_keystrokes("up");
        assert_eq!(shown(&i, cx), "ab|\ncd");
        cx.simulate_keystrokes("up");
        assert_eq!(shown(&i, cx), "|ab\ncd");
        cx.simulate_keystrokes("down down");
        assert_eq!(shown(&i, cx), "ab\ncd|");
        // ⌘⏎ is left to the owner (to commit, say).
        cx.simulate_keystrokes("cmd-enter");
        assert_eq!(shown(&i, cx), "ab\ncd|");

        let (i, cx) = input(&mut cx.cx, false);
        cx.simulate_input("ab");
        cx.simulate_keystrokes("enter");
        assert_eq!(shown(&i, cx), "ab|");
    }

    #[gpui::test]
    fn select_all_then_type_replaces(cx: &mut TestAppContext) {
        let (i, cx) = input(cx, false);
        i.update(cx, |i, cx| {
            i.set_text("old name", cx);
            i.select_all(cx);
        });
        cx.simulate_input("new");
        assert_eq!(shown(&i, cx), "new|");

        cx.simulate_keystrokes("cmd-a backspace");
        assert_eq!(shown(&i, cx), "|");
        // Selecting nothing selects nothing.
        cx.simulate_keystrokes("cmd-a");
        assert!(!i.read_with(cx, |i, _| i.all_selected));
    }

    #[gpui::test]
    fn copy_cut_and_paste(cx: &mut TestAppContext) {
        let (i, cx) = input(cx, false);
        cx.simulate_input("abc");
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("abc"));
        cx.simulate_keystrokes("cmd-x");
        assert_eq!(shown(&i, cx), "|");
        cx.write_to_clipboard(ClipboardItem::new_string("x\ny".into()));
        cx.simulate_keystrokes("cmd-v cmd-v");
        assert_eq!(shown(&i, cx), "x yx y|");
    }
}
