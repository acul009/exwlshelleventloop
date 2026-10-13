use iced_core::{Event, Point, Size, event::Status, mouse::Cursor, window::Id};
use iced_core::{renderer::Style, widget::Operation};
use iced_program::{Instance, Program};
use iced_runtime::{
    UserInterface as IcedUserInterface,
    user_interface::{Cache, State},
};
use std::{collections::HashMap, mem, slice};

use crate::gesture;

pub(crate) trait UserInterfaceReclaim<Message, Theme, Renderer> {
    fn reclaim(&mut self, ui: IcedUserInterface<'static, Message, Theme, Renderer>);
}

/// Provide a guard to hold the ui and prevent leaking the reference to the application. A user can hold this guard without querying from map each time.
/// When this guard is dropped, it will return the ui to the manager if it is not taken.
pub(crate) struct UserInterfaceMutGuard<'a, Message, Theme, Renderer, Reclaim>
where
    Reclaim: UserInterfaceReclaim<Message, Theme, Renderer>,
{
    reclaim: Reclaim,
    /// Building 'static IcedUserInterface will draw nothing, so we should use the safe lifetime as
    /// application.
    ui: Option<IcedUserInterface<'a, Message, Theme, Renderer>>,
}

impl<'a, Message, Theme, Renderer, Reclaim>
    UserInterfaceMutGuard<'a, Message, Theme, Renderer, Reclaim>
where
    Renderer: iced_core::Renderer,
    Message: 'a,
    Theme: 'a,
    Reclaim: UserInterfaceReclaim<Message, Theme, Renderer>,
{
    fn take(&mut self) -> IcedUserInterface<'a, Message, Theme, Renderer> {
        self.ui.take().expect("ui is taken")
    }

    pub fn draw(&mut self, renderer: &mut Renderer, theme: &Theme, style: &Style, cursor: Cursor) {
        let mut ui = self.take();
        ui.draw(renderer, theme, style, cursor);
        self.ui = Some(ui);
    }

    #[allow(unused)]
    pub fn into_cache(mut self) -> Cache {
        self.take().into_cache()
    }

    pub fn operate(&mut self, renderer: &Renderer, operation: &mut dyn Operation<()>) {
        let mut ui = self.take();
        ui.operate(renderer, operation);
        self.ui = Some(ui);
    }

    pub fn relayout(mut self, bounds: Size, renderer: &mut Renderer) -> Self {
        let ui = self.take().relayout(bounds, renderer);
        self.ui = Some(ui);
        self
    }

    pub fn update(
        &mut self,
        events: &[Event],
        cursor: Cursor,
        renderer: &mut Renderer,
        window: &dyn iced_core::window::Window,
        waker: &iced_core::shell::Waker,
        messages: &mut Vec<Message>,
        receipts: &mut Vec<iced_core::shell::Receipt>,
    ) -> (State, Vec<Status>) {
        let mut ui = self.take();
        let res = {
            let mut bus = iced_core::shell::Bus::new();
            let result = ui.update(window, waker, events, cursor, renderer, &mut bus);
            for (message, receipt) in bus.drain() {
                messages.push(message);
                receipts.push(receipt);
            }
            result
        };
        self.ui = Some(ui);
        res
    }

    /// Like [`Self::update`], but takes [`Input`]s. While handling a scroll,
    /// [`gesture::current`] returns its frame. Gestures go to the [`gesture::GestureReceiver`]
    /// tracked by `owners`.
    ///
    /// Returns the statuses of the events in order, and whether a gesture was received.
    pub fn update_with_gestures(
        &mut self,
        inputs: &[Input],
        owners: &mut gesture::Owners,
        cursor: Cursor,
        renderer: &mut Renderer,
        window: &dyn iced_core::window::Window,
        waker: &iced_core::shell::Waker,
        messages: &mut Vec<Message>,
        receipts: &mut Vec<iced_core::shell::Receipt>,
    ) -> (State, Vec<Status>, bool) {
        let mut ui = self.take();
        let mut result: Option<(State, Vec<Status>)> = None;
        let mut gesture_delivered = false;
        let mut plain = Vec::new();
        let mut update = |ui: &mut IcedUserInterface<'a, _, _, _>,
                          renderer: &mut Renderer,
                          events: &[Event],
                          frame: Option<gesture::Frame>,
                          result: &mut Option<(State, Vec<Status>)>| {
            let ((state, statuses), claim) = gesture::with_current(frame, || {
                let mut bus = iced_core::shell::Bus::new();
                let result = ui.update(window, waker, events, cursor, renderer, &mut bus);
                for (message, receipt) in bus.drain() {
                    messages.push(message);
                    receipts.push(receipt);
                }
                result
            });
            *result = Some(match result.take() {
                None => (state, statuses),
                Some((earlier, mut all_statuses)) => {
                    all_statuses.extend(statuses);
                    (merge_states(earlier, state), all_statuses)
                }
            });
            claim
        };
        for input in inputs {
            match input {
                Input::Event(event, None) => plain.push(event.clone()),
                Input::Event(event, Some(frame)) => {
                    if !plain.is_empty() {
                        update(&mut ui, renderer, &mem::take(&mut plain), None, &mut result);
                    }
                    owners.scroll = update(
                        &mut ui,
                        renderer,
                        slice::from_ref(event),
                        Some(*frame),
                        &mut result,
                    );
                }
                Input::Gesture(gesture, position) => {
                    if !plain.is_empty() {
                        update(&mut ui, renderer, &mem::take(&mut plain), None, &mut result);
                    }
                    gesture_delivered |= owners.deliver(*gesture, *position, |operation| {
                        ui.operate(renderer, operation);
                    });
                }
            }
        }
        if !plain.is_empty() || result.is_none() {
            update(&mut ui, renderer, &plain, None, &mut result);
        }
        self.ui = Some(ui);
        let (state, statuses) = result.expect("update ran at least once");
        (state, statuses, gesture_delivered)
    }
}

/// An input queued for a window's user interface.
#[derive(Debug, Clone)]
pub(crate) enum Input {
    /// An event, with its scroll frame if it is a scroll.
    Event(Event, Option<gesture::Frame>),
    /// A touchpad gesture, with the cursor position when it arrived.
    Gesture(gesture::Gesture, Option<Point>),
}

/// Merges the states of two consecutive updates the way iced merges them within one batch.
fn merge_states(earlier: State, later: State) -> State {
    match (earlier, later) {
        (State::Outdated, _) | (_, State::Outdated) => State::Outdated,
        (
            State::Updated {
                redraw_request: earlier_redraw,
                input_method: mut merged_input_method,
                change: earlier_change,
                clipboard: mut earlier_clipboard,
                ..
            },
            State::Updated {
                mouse_interaction,
                redraw_request,
                input_method,
                change,
                clipboard: mut later_clipboard,
            },
        ) => {
            merged_input_method.merge(&input_method);
            earlier_clipboard.merge(&mut later_clipboard);
            State::Updated {
                mouse_interaction,
                redraw_request: earlier_redraw.min(redraw_request),
                input_method: merged_input_method,
                change: match (earlier_change, change) {
                    (iced_runtime::user_interface::Change::Layout, _)
                    | (_, iced_runtime::user_interface::Change::Layout) => {
                        iced_runtime::user_interface::Change::Layout
                    }
                    (iced_runtime::user_interface::Change::Overlay, _)
                    | (_, iced_runtime::user_interface::Change::Overlay) => {
                        iced_runtime::user_interface::Change::Overlay
                    }
                    _ => iced_runtime::user_interface::Change::None,
                },
                clipboard: earlier_clipboard,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::merge_states;
    use iced_core::{Clipboard, InputMethod, clipboard, mouse, window::RedrawRequest};
    use iced_runtime::user_interface::{Change, State};

    #[test]
    fn batched_updates_preserve_clipboard_requests_and_strongest_invalidation() {
        let earlier = State::Updated {
            mouse_interaction: mouse::Interaction::Idle,
            redraw_request: RedrawRequest::NextFrame,
            input_method: InputMethod::Disabled,
            clipboard: Clipboard {
                reads: vec![clipboard::Kind::Text],
                write: Some(clipboard::Content::Text("first".into())),
            },
            change: Change::Layout,
        };
        let later = State::Updated {
            mouse_interaction: mouse::Interaction::Pointer,
            redraw_request: RedrawRequest::Wait,
            input_method: InputMethod::Disabled,
            clipboard: Clipboard {
                reads: vec![clipboard::Kind::Html],
                write: Some(clipboard::Content::Text("last".into())),
            },
            change: Change::Overlay,
        };
        let State::Updated {
            mouse_interaction,
            redraw_request,
            clipboard,
            change,
            ..
        } = merge_states(earlier, later)
        else {
            panic!("updated batches must stay updated");
        };
        assert_eq!(mouse_interaction, mouse::Interaction::Pointer);
        assert_eq!(redraw_request, RedrawRequest::NextFrame);
        assert_eq!(change, Change::Layout);
        assert_eq!(
            clipboard.reads,
            vec![clipboard::Kind::Text, clipboard::Kind::Html]
        );
        assert_eq!(
            clipboard.write,
            Some(clipboard::Content::Text("last".into()))
        );
    }

    #[test]
    fn batched_update_keeps_an_earlier_write_when_later_batch_has_none() {
        let state = |clipboard| State::Updated {
            mouse_interaction: mouse::Interaction::Idle,
            redraw_request: RedrawRequest::Wait,
            input_method: InputMethod::Disabled,
            clipboard,
            change: Change::None,
        };
        let State::Updated { clipboard, .. } = merge_states(
            state(Clipboard {
                reads: vec![],
                write: Some("copied".to_owned().into()),
            }),
            state(Clipboard::new()),
        ) else {
            panic!("updated batches must stay updated");
        };
        assert_eq!(
            clipboard.write,
            Some(clipboard::Content::Text("copied".into()))
        );
        assert!(matches!(
            merge_states(State::Outdated, state(Clipboard::new())),
            State::Outdated
        ));
    }
}

impl<Message, Theme, Renderer, Reclaim> Drop
    for UserInterfaceMutGuard<'_, Message, Theme, Renderer, Reclaim>
where
    Reclaim: UserInterfaceReclaim<Message, Theme, Renderer>,
{
    fn drop(&mut self) {
        if let Some(ui) = self.ui.take() {
            // SAFETY There is no public api to change ui. It always refers to application.
            let ui: IcedUserInterface<'static, _, _, _> = unsafe { mem::transmute(ui) };
            self.reclaim.reclaim(ui);
        }
    }
}

pub struct UserInterfaces<P: Program> {
    // SAFETY application will only be dropped after all uis are dropped. And we won't
    // allow publicly access to IcedUserInterface<'static, A::Message, A::Theme, A::Renderer>, so
    // reference to application won't be leaked to public.
    #[allow(clippy::type_complexity)]
    uis: HashMap<Id, IcedUserInterface<'static, P::Message, P::Theme, P::Renderer>>,
    application: Instance<P>,
}

impl<P: Program> UserInterfaces<P>
where
    P: Program + 'static,
{
    pub fn new(application: Instance<P>) -> Self {
        Self {
            uis: HashMap::new(),
            application,
        }
    }

    pub fn application(&self) -> &Instance<P> {
        &self.application
    }

    /// get the mut application
    pub fn application_mut(&mut self) -> &mut Instance<P> {
        &mut self.application
    }

    pub fn remove(&mut self, id: &Id) -> Option<Cache> {
        self.uis.remove(id).map(IcedUserInterface::into_cache)
    }

    pub fn extract_all(&mut self) -> (HashMap<Id, Cache>, &mut Instance<P>) {
        // SAFETY remove all references before return mut reference of application
        let caches = self
            .uis
            .drain()
            .map(|(id, ui)| (id, ui.into_cache()))
            .collect();
        (caches, &mut self.application)
    }

    #[allow(clippy::type_complexity)]
    pub fn ui_mut(
        &mut self,
        id: &Id,
    ) -> Option<UserInterfaceMutGuard<'static, P::Message, P::Theme, P::Renderer, (&mut Self, Id)>>
    {
        self.uis.remove(id).map(|ui| UserInterfaceMutGuard {
            reclaim: (self, *id),
            ui: Some(ui),
        })
    }

    pub fn build(&mut self, id: Id, cache: Cache, renderer: &mut P::Renderer, size: Size) {
        let view_span = iced_debug::view(id);
        let view = self.application.view(id);
        view_span.finish();

        let layout_span = iced_debug::layout(id);
        let ui = IcedUserInterface::build(view, size, cache, renderer);
        layout_span.finish();
        // SAFETY ui won't outlive application.
        let ui: IcedUserInterface<'static, _, _, _> = unsafe { mem::transmute(ui) };
        self.uis.insert(id, ui);
    }
}

impl<P: Program> Drop for UserInterfaces<P> {
    fn drop(&mut self) {
        // SAFETY drop all references of application before dropping application
        self.uis.clear();
    }
}

impl<P: Program> UserInterfaceReclaim<P::Message, P::Theme, P::Renderer>
    for (&mut UserInterfaces<P>, Id)
{
    fn reclaim(&mut self, ui: IcedUserInterface<'static, P::Message, P::Theme, P::Renderer>) {
        self.0.uis.insert(self.1, ui);
    }
}
