//! A short-lived card for an undoable sidebar action, such as archiving a folder.
use crate::theme::*;
use gpui::{Context, EventEmitter, FontWeight, Render, Role, Window, div, prelude::*, px};

pub(crate) enum UndoNoticeEvent {
    Undo,
    Dismiss,
}
#[derive(Clone, Default)]
pub(crate) struct UndoNotice {
    message: Option<String>,
}
impl EventEmitter<UndoNoticeEvent> for UndoNotice {}
impl UndoNotice {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn set_message(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        if self.message != message {
            self.message = message;
            cx.notify();
        }
    }
    pub(crate) fn is_visible(&self) -> bool {
        self.message.is_some()
    }
}
impl Render for UndoNotice {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(message) = &self.message else {
            return div().into_any_element();
        };
        super::surface::toast_frame()
            .id("undo-notice")
            .debug_selector(|| "undo-notice".into())
            .role(Role::Status)
            .aria_label(message.clone())
            .child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .child(message.clone()),
                    )
                    .child(
                        div()
                            .id("undo-notice-undo")
                            .debug_selector(|| "undo-notice-undo".into())
                            .role(Role::Button)
                            .aria_label("Undo")
                            .cursor_pointer()
                            .rounded(px(6.))
                            .px(px(10.))
                            .py(px(5.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .bg(content(0.1))
                            .hover(|s| s.bg(content(0.15)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(UndoNoticeEvent::Undo)))
                            .child("Undo"),
                    )
                    .child(
                        icon_button("dismiss-undo-notice", "Dismiss", "close")
                            .debug_selector(|| "dismiss-undo-notice".into())
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(UndoNoticeEvent::Dismiss))),
                    ),
            )
            .into_any_element()
    }
}
