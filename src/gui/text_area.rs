use freya::prelude::{
    AccessibilityExt, AccessibilityId, AccessibilityIdExt, AccessibilityRole, Area, Border,
    BorderAlignment, ChildrenExt, Component, ComponentKey, ContainerExt, ContainerSizeExt, Cursor,
    CursorIcon, DiffKey, Event, EventHandlersExt, Focus, FocusPressEventData, IntoElement, Key,
    KeyExt, KeyboardEventData, NamedKey, ParagraphHolder, PointerEventData, Size, SizedEventData,
    StyleExt, TextStyleExt, Writable, WritableUtils, paragraph, rect, use_focus, use_hook,
    use_state, use_theme,
};
use freya::text_edit::{EditableConfig, EditableEvent, EditorLine, TextEditor, use_editable};

use super::theme;

/// Enter adds a line instead of submitting.
#[derive(Clone, PartialEq)]
pub struct TextArea {
    value: Writable<String>,
    min_height: f32,
    key: DiffKey,
}

impl KeyExt for TextArea {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl TextArea {
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: value.into(),
            min_height: 120.,
            key: DiffKey::default(),
        }
    }

    #[must_use]
    pub fn min_height(mut self, min_height: f32) -> Self {
        self.min_height = min_height;
        self
    }
}

impl Component for TextArea {
    fn render(&self) -> impl IntoElement {
        let a11y_id = use_hook(AccessibilityId::new_unique);
        let focus = use_focus(a11y_id);
        let holder = use_state(ParagraphHolder::default);
        let mut area = use_state(Area::default);
        let mut dragging = use_state(|| false);
        let mut editable = use_editable(|| self.value.read().to_string(), EditableConfig::new);
        let mut value = self.value.clone();
        let is_empty = value.read().is_empty();
        let colors = use_theme().read().colors.clone();

        // Outside changes (Paste, Clear, Reset) must reach the editor.
        if *value.read() != editable.editor().read().committed_text() {
            let mut editor = editable.editor_mut().write();
            editor.set(&value.read());
            editor.editor_history().clear();
            editor.clear_selection();
            // Cursor is in UTF-16 units, which set() does not clamp correctly for non-ASCII text.
            editor.move_cursor_to(value.read().encode_utf16().count());
        }

        let on_key_down = move |event: Event<KeyboardEventData>| {
            match &event.key {
                Key::Named(NamedKey::Tab) => return,
                Key::Named(NamedKey::Escape) => {
                    a11y_id.request_unfocus();
                    return;
                }
                _ => {}
            }
            event.stop_propagation();
            event.prevent_default();
            editable.process_event(EditableEvent::KeyDown {
                key: &event.key,
                modifiers: event.modifiers,
            });
            let text = editable.editor().read().committed_text();
            if *value.read() != text {
                *value.write() = text;
            }
        };

        let on_key_up = move |event: Event<KeyboardEventData>| {
            event.stop_propagation();
            editable.process_event(EditableEvent::KeyUp { key: &event.key });
        };

        let on_focus_press = move |event: Event<FocusPressEventData>| {
            event.stop_propagation();
            event.prevent_default();
            a11y_id.request_focus();
            // Marks the press as inside, so the release does not unfocus.
            dragging.set_if_modified(true);
            // The paragraph shows the placeholder, so its geometry would put the cursor past the real text.
            if is_empty {
                return;
            }
            let area = area.read().to_f64();
            let global = event.global_location().clamp(area.min(), area.max());
            editable.process_event(EditableEvent::Down {
                location: (global - area.min()).to_point(),
                editor_line: EditorLine::SingleParagraph,
                holder: &holder.read(),
            });
        };

        let on_global_pointer_move = move |event: Event<PointerEventData>| {
            if is_empty || !a11y_id.is_focused() || !*dragging.read() {
                return;
            }
            let mut location = event.global_location();
            location.x -= area.read().min_x() as f64;
            location.y -= area.read().min_y() as f64;
            editable.process_event(EditableEvent::Move {
                location,
                editor_line: EditorLine::SingleParagraph,
                holder: &holder.read(),
            });
        };

        let on_pointer_press = move |event: Event<PointerEventData>| {
            event.stop_propagation();
            event.prevent_default();
            editable.process_event(EditableEvent::Release);
            dragging.set_if_modified(false);
        };

        let on_global_pointer_press = move |_: Event<PointerEventData>| {
            if !a11y_id.is_focused() {
                return;
            }
            editable.process_event(EditableEvent::Release);
            if *dragging.read() {
                dragging.set(false);
            } else {
                a11y_id.request_unfocus();
            }
        };

        let focused = focus() != Focus::Not;
        let (cursor_index, selection) = if focused {
            let editor = editable.editor().read();
            (
                Some(editor.cursor_pos()),
                editor.get_visible_selection(EditorLine::SingleParagraph),
            )
        } else {
            (None, None)
        };
        let border = Border::new()
            .fill(if focused {
                colors.primary
            } else {
                colors.border
            })
            .width(if focused { 2. } else { 1. })
            .alignment(BorderAlignment::Inner);
        let shown_text = editable.editor().read().rope().to_string();

        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(true)
            .a11y_role(AccessibilityRole::MultilineTextInput)
            .on_key_down(on_key_down)
            .on_key_up(on_key_up)
            .on_focus_press(on_focus_press)
            .on_pointer_press(on_pointer_press)
            .on_global_pointer_press(on_global_pointer_press)
            .on_global_pointer_move(on_global_pointer_move)
            .on_pointer_enter(|_| Cursor::set(CursorIcon::Text))
            .on_pointer_leave(|_| Cursor::set(CursorIcon::default()))
            .width(Size::fill())
            .min_height(Size::px(self.min_height))
            .background(colors.surface_tertiary)
            .border(border)
            .corner_radius(theme::corners())
            .padding(10.)
            .child(
                paragraph()
                    .holder(holder.read().clone())
                    .on_sized(move |event: Event<SizedEventData>| area.set(event.visible_area))
                    .width(Size::fill())
                    .font_size(13.)
                    .line_height(1.4)
                    .color(if is_empty {
                        colors.text_placeholder
                    } else {
                        colors.text_primary
                    })
                    .cursor_index(cursor_index)
                    .cursor_color(colors.text_primary)
                    .highlights(selection.map(|range| vec![range]))
                    .span(shown_text),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
