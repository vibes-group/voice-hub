use std::ops::Range;

use gpui::{prelude::*, *};

actions!(
    input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectAll,
        Home,
        End,
        Paste,
        Copy,
        Cut,
        Submit,
        Newline,
        SelectLeft,
        SelectRight,
        SelectHome,
        SelectEnd,
        Up,
        Down,
        Undo,
        Redo
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
        KeyBinding::new("enter", Submit, Some("Input")),
        KeyBinding::new("shift-enter", Newline, Some("Input")),
        KeyBinding::new("shift-left", SelectLeft, Some("Input")),
        KeyBinding::new("shift-right", SelectRight, Some("Input")),
        KeyBinding::new("shift-home", SelectHome, Some("Input")),
        KeyBinding::new("shift-end", SelectEnd, Some("Input")),
        KeyBinding::new("up", Up, Some("Input")),
        KeyBinding::new("down", Down, Some("Input")),
        KeyBinding::new("ctrl-z", Undo, Some("Input")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("Input")),
        KeyBinding::new("ctrl-y", Redo, Some("Input")),
    ]);
}

pub struct Input {
    pub text: String,
    focus: FocusHandle,
    secret: bool,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    bounds: Option<Bounds<Pixels>>,
    multiline: bool,
    reversed: bool,
    dragging: bool,
    layout: Vec<(usize, WrappedLine, Point<Pixels>)>,
    undo: Vec<(String, Range<usize>, bool)>,
    redo: Vec<(String, Range<usize>, bool)>,
}

pub enum InputEvent {
    Submit,
    PasteImage(gpui::Image),
}
impl EventEmitter<InputEvent> for Input {}

impl Input {
    pub fn new(text: &str, secret: bool, cx: &mut Context<Self>) -> Self {
        Self {
            text: text.into(),
            focus: cx.focus_handle(),
            secret,
            selection: text.len()..text.len(),
            marked: None,
            bounds: None,
            multiline: false,
            reversed: false,
            dragging: false,
            layout: vec![],
            undo: vec![],
            redo: vec![],
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.undo.clear();
        self.redo.clear();
        self.reversed = false;
        self.selection = 0..0;
        self.marked = None;
        cx.notify();
    }

    pub fn set(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.into();
        self.selection = text.len()..text.len();
        self.marked = None;
        self.reversed = false;
        self.undo.clear();
        self.redo.clear();
        cx.notify();
    }
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
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
        self.reversed = false;
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
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(image) = item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            }) {
                cx.emit(InputEvent::PasteImage(image));
            } else if let Some(text) = item.text() {
                self.replace_text_in_range(None, &text, window, cx);
            }
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

    fn caret(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }
    fn select_to(&mut self, byte: usize, cx: &mut Context<Self>) {
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        self.selection = anchor.min(byte)..anchor.max(byte);
        self.reversed = byte < anchor;
        self.marked = None;
        cx.notify();
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let caret = self.caret();
        let previous = self.text[..caret]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
        self.select_to(previous, cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let caret = self.caret();
        let next = caret + self.text[caret..].chars().next().map_or(0, char::len_utf8);
        self.select_to(next, cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.text.len(), cx);
    }
    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, selection, reversed)) = self.undo.pop() {
            self.redo.push((
                std::mem::replace(&mut self.text, text),
                std::mem::replace(&mut self.selection, selection),
                std::mem::replace(&mut self.reversed, reversed),
            ));
            self.marked = None;
            cx.notify();
        }
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, selection, reversed)) = self.redo.pop() {
            self.undo.push((
                std::mem::replace(&mut self.text, text),
                std::mem::replace(&mut self.selection, selection),
                std::mem::replace(&mut self.reversed, reversed),
            ));
            self.marked = None;
            cx.notify();
        }
    }
    fn point_for_byte(&self, byte: usize) -> Option<Point<Pixels>> {
        let display = self.display_offset(byte);
        self.layout.iter().find_map(|(start, line, origin)| {
            if display >= *start && display <= *start + line.len() {
                line.position_for_index(display - start, px(22.0))
                    .map(|p| p + *origin)
            } else {
                None
            }
        })
    }
    fn byte_for_point(&self, point: Point<Pixels>) -> usize {
        let Some((start, line, origin)) = self
            .layout
            .iter()
            .rev()
            .find(|(_, _, origin)| point.y >= origin.y)
            .or(self.layout.first())
        else {
            return self.text.len();
        };
        let relative = point - *origin;
        let index = match line.closest_index_for_position(relative, px(22.0)) {
            Ok(index) | Err(index) => index,
        };
        let display = (*start + index).min(self.display().len());
        if self.secret {
            self.text
                .char_indices()
                .nth(display / 3)
                .map_or(self.text.len(), |(byte, _)| byte)
        } else {
            display.min(self.text.len())
        }
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(position) = self.point_for_byte(self.caret()) {
            let byte = self.byte_for_point(position - point(px(0.0), px(22.0)));
            self.move_to(byte, cx);
        }
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(position) = self.point_for_byte(self.caret()) {
            let byte = self.byte_for_point(position + point(px(0.0), px(22.0)));
            self.move_to(byte, cx);
        }
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
            reversed: self.reversed,
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
            .filter(|ch| !ch.is_control() || (self.multiline && *ch == '\n'))
            .take(4096)
            .collect();
        if self.text.len() - bytes.len() + text.len() > 16_384 {
            return;
        }
        self.undo
            .push((self.text.clone(), self.selection.clone(), self.reversed));
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.text.replace_range(bytes.clone(), &text);
        self.reversed = false;
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
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let position = self.point_for_byte(self.byte_offset(range.start))?;
        Some(Bounds::new(position, size(px(2.0), px(22.0))))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.utf16_offset(self.byte_for_point(point)))
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
        let multiline = self.multiline;
        div()
            .key_context("Input")
            .track_focus(&self.focus)
            .w_full()
            .min_w_0()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(0x111821))
            .border_1()
            .border_color(rgb(0x314050))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus.focus(window);
                    let byte = this.byte_for_point(event.position);
                    if event.modifiers.shift {
                        this.select_to(byte, cx);
                    } else {
                        this.move_to(byte, cx);
                    }
                    this.dragging = true;
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.dragging && event.pressed_button == Some(MouseButton::Left) {
                    let byte = this.byte_for_point(event.position);
                    this.select_to(byte, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
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
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let input = entity.read(cx);
                        let display: SharedString = input.display().into();
                        let cursor = input.display_offset(input.caret());
                        let selection = input.display_offset(input.selection.start)
                            ..input.display_offset(input.selection.end);
                        let focus = input.focus.clone();
                        let run = TextRun {
                            len: display.len(),
                            font: window.text_style().font(),
                            color: rgb(0xe3edf7).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let Ok(lines) = window.text_system().shape_text(
                            display,
                            px(15.0),
                            &[run],
                            multiline.then_some(bounds.size.width),
                            None,
                        ) else {
                            return;
                        };
                        let height = px(22.0);
                        let mut layout = Vec::new();
                        let mut start = 0;
                        let mut y = bounds.top();
                        let mut caret = point(bounds.left(), bounds.top());
                        for line in lines {
                            if cursor >= start
                                && cursor <= start + line.len()
                                && let Some(position) =
                                    line.position_for_index(cursor - start, height)
                            {
                                caret = point(bounds.left(), y) + position;
                            }
                            let line_height = line.size(height).height;
                            layout.push((start, line, point(bounds.left(), y)));
                            start += layout.last().expect("line").1.len() + 1;
                            y += line_height;
                        }
                        let scroll_x = if multiline {
                            px(0.0)
                        } else {
                            (caret.x - bounds.right() + px(3.0)).max(px(0.0))
                        };
                        let scroll_y = if multiline {
                            (caret.y + height - bounds.bottom()).max(px(0.0))
                        } else {
                            px(0.0)
                        };
                        for (start, line, origin) in &mut layout {
                            *origin -= point(scroll_x, scroll_y);
                            if focus.is_focused(window) {
                                for (index, ch) in line.text.char_indices() {
                                    let global = *start + index;
                                    if global >= selection.start
                                        && global < selection.end
                                        && let Some(position) =
                                            line.position_for_index(index, height)
                                    {
                                        let next = line
                                            .position_for_index(index + ch.len_utf8(), height)
                                            .unwrap_or(position + point(px(8.0), px(0.0)));
                                        let width = if next.y > position.y {
                                            bounds.right() - origin.x - position.x
                                        } else {
                                            next.x - position.x
                                        };
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                *origin + position,
                                                size(width.max(px(1.0)), height),
                                            ),
                                            rgb(0x285050),
                                        ));
                                    }
                                }
                            }
                            let _ = line.paint(
                                *origin,
                                height,
                                TextAlign::Left,
                                Some(bounds),
                                window,
                                cx,
                            );
                        }
                        if focus.is_focused(window) && selection.is_empty() {
                            window.paint_quad(fill(
                                Bounds::new(
                                    caret - point(scroll_x, scroll_y),
                                    size(px(1.5), height),
                                ),
                                rgb(0x64dfc3),
                            ));
                        }
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        entity.update(cx, |input, _| {
                            input.bounds = Some(bounds);
                            input.layout = layout;
                        });
                    },
                )
                .w_full()
                .h(px(if multiline { 88.0 } else { 24.0 })),
            )
    }
}
