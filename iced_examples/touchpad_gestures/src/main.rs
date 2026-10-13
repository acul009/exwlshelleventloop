use iced::advanced::layout::{Layout, Limits};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::widget::{column, container, pin, stack, text};
use iced::window::Id;
use iced::{
    Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Task, Theme, Vector, mouse,
};

use iced_exwlshell::daemon;
use iced_exwlshell::gesture::{Gesture, GestureReceiver, Pinch, Swipe};
use iced_exwlshell::reexport::{
    Anchor, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
};
use iced_exwlshell::settings::{ExWlSettings, LayerShellSettings};
use iced_exwlshell::to_layer_message;

fn main() -> iced_exwlshell::Result {
    daemon(
        Board::default,
        "touchpad gestures",
        Board::update,
        Board::view,
    )
    .wl_settings(ExWlSettings {
        // Required for touchpad gestures.
        gestures: true,
        layer_settings: LayerShellSettings {
            size: LayerSize::px(600, 600),
            anchor: Anchor::empty(),
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}

/// The side of the square at zoom 1, in logical pixels.
const SIDE: f32 = 120.0;
/// How far a swipe must travel, in logical pixels, to count.
const SWIPE_DISTANCE: f32 = 60.0;
/// The background of each page.
const PAGES: [Color; 4] = [
    Color::from_rgb(0.15, 0.15, 0.15),
    Color::from_rgb(0.12, 0.2, 0.14),
    Color::from_rgb(0.22, 0.13, 0.13),
    Color::from_rgb(0.13, 0.14, 0.24),
];

/// The action a finished swipe triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    OpenPanel,
    ClosePanel,
    PreviousPage,
    NextPage,
}

impl Command {
    /// Classifies a swipe of `fingers` that moved by `distance` in total.
    fn from_swipe(fingers: u32, distance: Vector) -> Option<Self> {
        let vertical = distance.y.abs() > distance.x.abs();
        let length = if vertical { distance.y } else { distance.x };
        if length.abs() < SWIPE_DISTANCE {
            return None;
        }
        match (fingers, vertical, length > 0.0) {
            (3, true, true) => Some(Self::OpenPanel),
            (3, true, false) => Some(Self::ClosePanel),
            (4, false, false) => Some(Self::PreviousPage),
            (4, false, true) => Some(Self::NextPage),
            _ => None,
        }
    }
}

struct Board {
    center: Point,
    zoom: f32,
    /// The zoom when the current pinch began.
    pinch_zoom: f32,
    /// Degrees, clockwise.
    rotation: f32,
    page: usize,
    /// The finger count and distance so far of the current swipe.
    swipe: Option<(u32, Vector)>,
    /// The panel layer surface, while it is open.
    panel: Option<Id>,
    last: String,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            center: Point::new(300.0, 300.0),
            zoom: 1.0,
            pinch_zoom: 1.0,
            rotation: 0.0,
            page: 0,
            swipe: None,
            panel: None,
            last: String::from("Swipe or pinch on the touchpad"),
        }
    }
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
    Gesture(Gesture),
}

impl Board {
    fn update(&mut self, message: Message) -> Task<Message> {
        let Message::Gesture(gesture) = message else {
            return Task::none();
        };
        self.last = format!("{gesture:?}");
        match gesture {
            Gesture::Pinch(Pinch::Begin { .. }) => self.pinch_zoom = self.zoom,
            Gesture::Pinch(Pinch::Update {
                delta,
                scale,
                rotation,
                ..
            }) => {
                // `scale` is relative to the start of the pinch; `rotation` is relative
                // to the previous update.
                self.zoom = (self.pinch_zoom * scale).clamp(0.2, 5.0);
                self.rotation = (self.rotation + rotation).rem_euclid(360.0);
                self.center += delta;
            }
            Gesture::Swipe(Swipe::Begin { fingers, .. }) => {
                self.swipe = Some((fingers, Vector::ZERO));
            }
            Gesture::Swipe(Swipe::Update { delta, .. }) => {
                if let Some((_, distance)) = &mut self.swipe {
                    *distance += delta;
                }
            }
            Gesture::Swipe(Swipe::End { cancelled, .. }) => {
                if let Some((fingers, distance)) = self.swipe.take()
                    && !cancelled
                    && let Some(command) = Command::from_swipe(fingers, distance)
                {
                    self.last = format!("{command:?}");
                    return self.run(command);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn run(&mut self, command: Command) -> Task<Message> {
        match command {
            Command::OpenPanel if self.panel.is_none() => {
                let id = Id::unique();
                self.panel = Some(id);
                Task::done(Message::NewLayerShell {
                    settings: NewLayerShellSettings {
                        size: LayerSize::fill_width(120),
                        anchor: Anchor::Top,
                        layer: Layer::Overlay,
                        keyboard_interactivity: KeyboardInteractivity::None,
                        ..Default::default()
                    },
                    id,
                })
            }
            Command::ClosePanel => match self.panel.take() {
                Some(id) => Task::done(Message::RemoveWindow(id)),
                None => Task::none(),
            },
            Command::PreviousPage => {
                self.page = (self.page + PAGES.len() - 1) % PAGES.len();
                Task::none()
            }
            Command::NextPage => {
                self.page = (self.page + 1) % PAGES.len();
                Task::none()
            }
            Command::OpenPanel => Task::none(),
        }
    }

    fn view(&self, id: Id) -> Element<'_, Message> {
        if self.panel == Some(id) {
            return self.panel_view();
        }
        let side = SIDE * self.zoom;
        let square = container(text(format!("{:.0}°", self.rotation)).color(Color::WHITE))
            .center(side)
            .style(|_: &Theme| container::background(Color::from_rgb(0.3, 0.4, 0.8)));
        let board = stack![
            pin(square).position(self.center - Vector::new(side, side) * 0.5),
            column![
                text(format!("page {}/{}", self.page + 1, PAGES.len())),
                text(format!("zoom x{:.2}", self.zoom)),
                text(&self.last).size(12),
            ]
            .padding(10),
        ];
        let background = PAGES[self.page];
        let board = container(board)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_: &Theme| container::background(background));
        GestureArea::new(board, Message::Gesture)._boxed()
    }

    fn panel_view(&self) -> Element<'_, Message> {
        let panel = container(text("Swipe up with three fingers to close").color(Color::WHITE))
            .center(Length::Fill)
            .style(|_: &Theme| container::background(Color::from_rgb(0.3, 0.4, 0.8)));
        // The panel receives gestures too, so it can be closed while the pointer is over it.
        GestureArea::new(panel, Message::Gesture)._boxed()
    }
}

/// Publishes the touchpad gestures that begin over its content.
struct GestureArea<'a> {
    content: Element<'a, Message>,
    on_gesture: fn(Gesture) -> Message,
}

impl<'a> GestureArea<'a> {
    fn new(
        content: impl Widget<Message, Theme, Renderer> + 'a,
        on_gesture: fn(Gesture) -> Message,
    ) -> Self {
        Self {
            content: content._boxed(),
            on_gesture,
        }
    }
}

impl Widget<Message, Theme, Renderer> for GestureArea<'_> {
    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GestureReceiver>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GestureReceiver::new())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) {
        self.content.layout(&mut tree.children[0], renderer, limits);
        tree.size = tree.children[0].size;
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
        operation.custom(
            None,
            layout.bounds(),
            tree.state.downcast_mut::<GestureReceiver>(),
        );
        let content_layout = layout
            .iter(&tree.children)
            .next()
            .expect("gesture content exists")
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // The runtime requests a redraw after delivering a gesture, so this runs right after
        // delivery.
        let receiver = tree.state.downcast_mut::<GestureReceiver>();
        while let Some(gesture) = receiver.take() {
            shell.publish((self.on_gesture)(gesture));
        }
        let content_layout = layout
            .iter(&tree.children)
            .next()
            .expect("gesture content exists")
            .0;
        self.content.update(
            &mut tree.children[0],
            event,
            content_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        self.content.draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .iter(&tree.children)
                .next()
                .expect("gesture content exists")
                .0,
            cursor,
            viewport,
        );
    }
}

impl iced::advanced::widget::Meta for GestureArea<'_> {}
