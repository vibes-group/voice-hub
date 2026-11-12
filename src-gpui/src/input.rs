use std::ops::Range;

use gpui::{prelude::*, *};

actions!(
    input,
    [
        Backspace, Delete, Left, Right, SelectAll, Home, End, Paste, Copy, Cut
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Input")),
        KeyBinding::new("delete", Delete, Some("Input")),
        KeyBinding::new("left", Left, Some("Input")),
        KeyBinding::new("right", Right, Some("Input")),
        KeyBinding::new("home", Home, Some("Input")),
        KeyBinding::new("end", End, Some("Input")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Input")),
        KeyBinding::new("ctrl-v", Paste, Some("Input")),
        KeyBinding::new("ctrl-c", Copy, Some("Input")),
        KeyBinding::new("ctrl-x", Cut, Some("Input")),
        KeyBinding::new("cmd-a", SelectAll, Some("Input")),
        KeyBinding::new("cmd-v", Paste, Some("Input")),
        KeyBinding::new("cmd-c", Copy, Some("Input")),
        KeyBinding::new("cmd-x", Cut, Some("Input")),
    ]);
}

pub struct Input {
    pub text: String,
    focus: FocusHandle,
    secret: bool,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    bounds: Option<Bounds<Pixels>>,
}

impl Input {
    pub fn new(text: &str, secret: bool, cx: &mut Context<Self>) -> Self {
        Self {
            text: text.into(),
            focus: cx.focus_handle(),
            secret,
            selection: text.len()..text.len(),
            marked: None,
            bounds: None,
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.selection = 0..0;
        self.marked = None;
        cx.notify();
    }

    fn byte_offset(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (byte, ch) in self.text.char_indices() {
            if units >= utf16 {
                return byte;
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }

    fn utf16_offset(&self, byte: usize) -> usize {
        self.text[..byte].encode_utf16().count()
    }

    fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        self.byte_offset(range.start)..self.byte_offset(range.end)
    }

    fn move_to(&mut self, byte: usize, cx: &mut Context<Self>) {
        self.selection = byte..byte;
        self.marked = None;
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.selection.start = self.text[..self.selection.start]
                .char_indices()
                .last()
                .map_or(0, |(byte, _)| byte);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.selection.end += self.text[self.selection.end..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let previous = if self.selection.is_empty() {
            self.text[..self.selection.start]
                .char_indices()
                .last()
                .map_or(0, |(byte, _)| byte)
        } else {
            self.selection.start
        };
        self.move_to(previous, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let next = if self.selection.is_empty() {
            self.selection.end
                + self.text[self.selection.end..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8)
        } else {
            self.selection.end
        };
        self.move_to(next, cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = 0..self.text.len();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selection.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        self.replace_text_in_range(None, "", window, cx);
    }

    fn display(&self) -> String {
        if self.secret {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    fn display_offset(&self, byte: usize) -> usize {
        if self.secret {
            self.text[..byte].chars().count() * '•'.len_utf8()
        } else {
            byte
        }
    }
}

impl EntityInputHandler for Input {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = self.byte_range(range);
        *actual = Some(self.utf16_offset(bytes.start)..self.utf16_offset(bytes.end));
        Some(self.text[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_offset(self.selection.start)..self.utf16_offset(self.selection.end),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.utf16_offset(r.start)..self.utf16_offset(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = range
            .map(|r| self.byte_range(r))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let text: String = text
            .chars()
            .filter(|ch| !ch.is_control())
            .take(4096)
            .collect();
        if self.text.len() - bytes.len() + text.len() > 16_384 {
            return;
        }
        self.text.replace_range(bytes.clone(), &text);
        self.selection = bytes.start + text.len()..bytes.start + text.len();
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = range
            .as_ref()
            .map(|r| self.byte_offset(r.start))
            .or(self.marked.as_ref().map(|r| r.start))
            .unwrap_or(self.selection.start);
        self.replace_text_in_range(range, text, window, cx);
        let end = self.selection.end;
        self.marked = (start < end).then_some(start..end);
        if let Some(range) = selected {
            let base = self.utf16_offset(start);
            self.selection = self.byte_range(base + range.start..base + range.end);
        }
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.bounds
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.utf16_offset(self.selection.end))
    }
}

impl Focusable for Input {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Input {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .key_context("Input")
            .track_focus(&self.focus)
            .tab_index(0)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(0x111821))
            .border_1()
            .border_color(rgb(0x314050))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window);
                    this.move_to(this.text.len(), cx);
                }),
            )
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let input = entity.read(cx);
                        let display: SharedString = input.display().into();
                        let cursor = input.display_offset(input.selection.end);
                        let selection = input.display_offset(input.selection.start)..cursor;
                        let focus = input.focus.clone();
                        let run = TextRun {
                            len: display.len(),
                            font: window.text_style().font(),
                            color: rgb(0xe3edf7).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = window
                            .text_system()
                            .shape_line(display, px(15.0), &[run], None);
                        let offset =
                            (line.x_for_index(cursor) - bounds.size.width + px(3.0)).max(px(0.0));
                        let origin = point(bounds.left() - offset, bounds.top());
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        if focus.is_focused(window) {
                            if selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(origin.x + line.x_for_index(cursor), origin.y),
                                        size(px(1.5), bounds.size.height),
                                    ),
                                    rgb(0x64dfc3),
                                ));
                            } else {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            origin.x + line.x_for_index(selection.start),
                                            origin.y,
                                        ),
                                        point(
                                            origin.x + line.x_for_index(selection.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgb(0x285050),
                                ));
                            }
                        }
                        let _ = line.paint(origin, bounds.size.height, window, cx);
                        entity.update(cx, |input, _| input.bounds = Some(bounds));
                    },
                )
                .w_full()
                .h(px(24.0)),
            )
    }
}
