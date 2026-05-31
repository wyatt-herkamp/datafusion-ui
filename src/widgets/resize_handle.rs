//! A thin draggable column-resize handle, sitting at the right edge of a data
//! grid's header cell. Built directly on Iced's `advanced` widget API because
//! Iced has no built-in column resizer.
//!
//! Behaviour (pane_grid-resizer pattern):
//! - press + drag → emits `on_resize(new_width)` continuously (tracking the
//!   cursor even when it leaves the handle's narrow bounds, since `update`
//!   receives every `CursorMoved`);
//! - release after a real drag → emits `on_release` (the caller persists);
//! - double-click → emits `on_autofit`.

use std::time::Instant;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse, renderer};
use iced::{Element, Event, Length, Rectangle, Size};

use crate::theme::palette;

/// Smallest width a column may be dragged to. Shared with the resize handlers
/// so the widget and the app agree on the floor.
pub const MIN_COL_WIDTH: f32 = 48.0;
const MAX_COL_WIDTH: f32 = 2000.0;
/// Width of the (invisible) hit area; the visible divider is 1px centered.
const HANDLE_HIT_WIDTH: f32 = 8.0;
/// Two presses within this window count as a double-click (autofit).
const DOUBLE_CLICK_MS: u128 = 400;

pub struct ResizeHandle<'a, Message> {
    /// The column's current committed width (drag origin).
    current_width: f32,
    on_resize: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Message,
    on_autofit: Message,
}

/// Build a resize handle for a column of `current_width`.
pub fn resize_handle<'a, Message: 'a>(
    current_width: f32,
    on_resize: impl Fn(f32) -> Message + 'a,
    on_release: Message,
    on_autofit: Message,
) -> ResizeHandle<'a, Message> {
    ResizeHandle {
        current_width,
        on_resize: Box::new(on_resize),
        on_release,
        on_autofit,
    }
}

#[derive(Default)]
struct State {
    drag: Option<Drag>,
    last_click: Option<Instant>,
}

struct Drag {
    press_x: f32,
    press_width: f32,
    moved: bool,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for ResizeHandle<'_, Message>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fixed(HANDLE_HIT_WIDTH),
            height: Length::Fill,
        }
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fixed(HANDLE_HIT_WIDTH), Length::Fill)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position() else { return };
                if !bounds.contains(pos) {
                    return;
                }
                let now = Instant::now();
                let is_double = state
                    .last_click
                    .map(|prev| now.duration_since(prev).as_millis() <= DOUBLE_CLICK_MS)
                    .unwrap_or(false);
                if is_double {
                    state.last_click = None;
                    state.drag = None;
                    shell.publish(self.on_autofit.clone());
                } else {
                    state.last_click = Some(now);
                    state.drag = Some(Drag {
                        press_x: pos.x,
                        press_width: self.current_width,
                        moved: false,
                    });
                    shell.request_redraw();
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(drag) = state.drag.as_mut() {
                    drag.moved = true;
                    let new_width = (drag.press_width + (position.x - drag.press_x))
                        .clamp(MIN_COL_WIDTH, MAX_COL_WIDTH);
                    shell.publish((self.on_resize)(new_width));
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some(drag) = state.drag.take() {
                    if drag.moved {
                        shell.publish(self.on_release.clone());
                    }
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        if state.drag.is_some() || cursor.is_over(layout.bounds()) {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let active = state.drag.is_some() || cursor.is_over(bounds);
        let color = if active {
            palette::accent_warm()
        } else {
            palette::border_subtle()
        };
        // 1px (2px when active) vertical divider, centered in the hit area.
        let w = if active { 2.0 } else { 1.0 };
        let line = Rectangle {
            x: bounds.x + (bounds.width - w) / 2.0,
            y: bounds.y,
            width: w,
            height: bounds.height,
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: line,
                ..renderer::Quad::default()
            },
            color,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<ResizeHandle<'a, Message>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(handle: ResizeHandle<'a, Message>) -> Self {
        Element::new(handle)
    }
}
