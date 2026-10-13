use std::time::Instant;

use iced::advanced::layout::{Layout, Limits};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Renderer as _, Shell, Widget, renderer};
use iced::widget::{Column, container, text};
use iced::{
    Color, Element, Event, Length, Rectangle, Renderer, Size, Task, Theme, Vector, mouse, window,
};

use iced_exwlshell::gesture::{self, Gesture, GestureReceiver, Source};
use iced_exwlshell::layershell::application;
use iced_exwlshell::reexport::{Anchor, LayerSize};
use iced_exwlshell::settings::{ExWlSettings, LayerShellSettings};
use iced_exwlshell::to_layer_message;

fn main() -> iced_exwlshell::Result {
    application(|| Rows, "kinetic scroll", update, view)
        .wl_settings(ExWlSettings {
            // Required for `gesture::current` and touchpad gestures.
            gestures: true,
            layer_settings: LayerShellSettings {
                size: LayerSize::px(400, 600),
                anchor: Anchor::empty(),
                ..Default::default()
            },
            ..Default::default()
        })
        .run()
}

struct Rows;

#[to_layer_message]
#[derive(Debug, Clone)]
enum Message {}

fn update(_: &mut Rows, _: Message) -> Task<Message> {
    Task::none()
}

fn view(_: &Rows) -> Element<'_, Message> {
    let rows = (0..100).map(|i| {
        let shade = if i % 2 == 0 { 0.2 } else { 0.25 };
        container(text(format!("Row {i}")).size(20).color(Color::WHITE))
            .padding(12)
            .width(Length::Fill)
            .style(move |_: &Theme| container::background(Color::from_rgb(shade, shade, shade)))
            ._boxed()
    });
    Kinetic::new(Column::with_children(rows))._boxed()
}

/// Pixels per wheel step.
const LINE_HEIGHT: f32 = 40.0;
/// How fast a wheel scroll glides: the distance left shrinks by `e^-SMOOTHING` every second.
const SMOOTHING: f32 = 20.0;
/// A wheel scroll closer than this to its target, in pixels, snaps to it.
const SNAP: f32 = 0.5;
/// How fast a fling slows down: the velocity shrinks by `e^-DECAY` every second.
const DECAY: f32 = 4.0;
/// A fling slower than this, in pixels per second, stops.
const MIN_VELOCITY: f32 = 20.0;
/// Fingers resting longer than this, in milliseconds, before lifting do not fling.
const MAX_REST: u32 = 50;
/// Maximum gap between scroll frames, in milliseconds, for measuring velocity.
const MAX_FRAME_GAP: u32 = 100;

/// Scrolls its content vertically, glides on wheel steps, and keeps moving after a touchpad scroll ends.
struct Kinetic<'a> {
    content: Element<'a, Message>,
}

impl<'a> Kinetic<'a> {
    fn new(content: impl Widget<Message, Theme, Renderer> + 'a) -> Self {
        Self {
            content: content._boxed(),
        }
    }
}

struct State {
    receiver: GestureReceiver,
    offset: f32,
    /// Pixels per second, positive towards the end of the content.
    velocity: f32,
    /// Compositor time of the last finger scroll, in milliseconds.
    last_scroll: Option<u32>,
    /// When the fling was last advanced; `Some` while flinging.
    fling: Option<Instant>,
    /// The offset a wheel scroll glides to, and when it was last advanced.
    glide: Option<(f32, Instant)>,
}

impl State {
    fn scroll_by(&mut self, pixels: f32, max_offset: f32) {
        self.offset = (self.offset + pixels).clamp(0.0, max_offset);
    }

    fn scroll_with_fingers(&mut self, pixels: f32, time: Option<u32>) {
        let elapsed = time
            .zip(self.last_scroll)
            .map(|(now, last)| now.wrapping_sub(last))
            .filter(|elapsed| (1..=MAX_FRAME_GAP).contains(elapsed));
        self.velocity = match elapsed {
            // Smooth out uneven frames.
            Some(elapsed) => (self.velocity + pixels / (elapsed as f32 / 1000.0)) / 2.0,
            None => 0.0,
        };
        self.last_scroll = time;
    }

    fn glide_by(&mut self, pixels: f32, max_offset: f32) {
        let (target, last) = self.glide.unwrap_or((self.offset, Instant::now()));
        self.glide = Some(((target + pixels).clamp(0.0, max_offset), last));
    }

    fn stop(&mut self) {
        self.velocity = 0.0;
        self.fling = None;
        self.glide = None;
    }
}

impl Widget<Message, Theme, Renderer> for Kinetic<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            receiver: GestureReceiver::new(),
            offset: 0.0,
            velocity: 0.0,
            last_scroll: None,
            fling: None,
            glide: None,
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        self.content.layout(
            &mut tree.children[0],
            renderer,
            &Limits::new(Size::ZERO, Size::new(size.width, f32::INFINITY)),
        );
        tree.size = size;
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // Exposes the receiver so the runtime can deliver touchpad gestures to it.
        let state = tree.state.downcast_mut::<State>();
        operation.custom(None, layout.bounds(), &mut state.receiver);
        let content_layout = layout
            .iter(&tree.children)
            .next()
            .expect("kinetic content exists")
            .0;
        operation.traverse(&mut |operation| {
            self.content.operate(
                &mut tree.children[0],
                content_layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let content = layout
            .iter(&tree.children)
            .next()
            .expect("kinetic content exists")
            .0
            .bounds();
        let max_offset = (content.height - bounds.height).max(0.0);

        // The runtime requests a redraw after delivering a gesture, so this runs soon after the
        // fingers are lifted or put down.
        while let Some(gesture) = state.receiver.take() {
            match gesture {
                Gesture::Stop(stop) if stop.axes.y => {
                    let rested = stop
                        .time
                        .zip(state.last_scroll)
                        .is_some_and(|(stop, last)| stop.wrapping_sub(last) > MAX_REST);
                    if !rested && state.velocity.abs() >= MIN_VELOCITY {
                        state.glide = None;
                        state.fling = Some(Instant::now());
                        shell.request_redraw();
                    } else {
                        state.stop();
                    }
                }
                // Putting fingers back on the touchpad stops the fling.
                Gesture::Hold(_) => state.stop(),
                _ => {}
            }
        }

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                state.fling = None;
                match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => {
                        state.glide_by(-y * LINE_HEIGHT, max_offset);
                    }
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        // Pixel scrolls already move smoothly, so they follow at once.
                        state.glide = None;
                        state.scroll_by(-y, max_offset);
                        // Only touchpads send stops, so only they fling.
                        if let Some(frame) = gesture::current()
                            && frame.source == Some(Source::Finger)
                        {
                            state.receiver.claim();
                            state.scroll_with_fingers(-y, frame.time);
                        }
                    }
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if let Some(last) = state.fling {
                    let elapsed = now.saturating_duration_since(last).as_secs_f32();
                    state.scroll_by(state.velocity * elapsed, max_offset);
                    state.velocity *= (-DECAY * elapsed).exp();
                    let at_edge = state.offset == 0.0 || state.offset == max_offset;
                    if state.velocity.abs() < MIN_VELOCITY || at_edge {
                        state.stop();
                    } else {
                        state.fling = Some(*now);
                        shell.request_redraw();
                    }
                }
                if let Some((target, last)) = state.glide {
                    let elapsed = now.saturating_duration_since(last).as_secs_f32();
                    let left = (target - state.offset) * (-SMOOTHING * elapsed).exp();
                    if left.abs() < SNAP {
                        state.offset = target;
                        state.glide = None;
                    } else {
                        state.offset = target - left;
                        state.glide = Some((target, *now));
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) if cursor.is_over(bounds) => {
                state.stop();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let Some(visible) = layout.bounds().intersection(viewport) else {
            return;
        };
        let translation = Vector::new(0.0, state.offset);
        renderer.with_layer(visible, |renderer| {
            renderer.with_translation(-translation, |renderer| {
                self.content.draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    layout
                        .iter(&tree.children)
                        .next()
                        .expect("kinetic content exists")
                        .0,
                    cursor + translation,
                    &(visible + translation),
                );
            });
        });
    }
}

impl iced::advanced::widget::Meta for Kinetic<'_> {}
