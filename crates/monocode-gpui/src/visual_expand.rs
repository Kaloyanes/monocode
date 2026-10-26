//! An HTML visual opened over the whole window.
use crate::monos::html::Html;
use crate::theme::{content, icon};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, KeyBinding, Role, SharedString, Window,
    actions, div, prelude::*, px,
};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = visual_expand, no_json)]
pub(crate) struct Open {
    pub title: SharedString,
    pub body: Arc<str>,
}

actions!(visual_expand, [Close]);

pub(crate) fn bind(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Close, Some("VisualExpand"))]);
}

pub(crate) struct Closed;
pub(crate) struct VisualExpand {
    title: SharedString,
    page: Entity<Html>,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}
impl EventEmitter<Closed> for VisualExpand {}

impl VisualExpand {
    pub(crate) fn new(open: Open, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // A fresh page: the inline one keeps its own state underneath.
        let page = cx.new(|cx| Html::modal(open.body.to_string(), cx));
        Self {
            title: open.title,
            page,
            focus,
            restore,
        }
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page.read(cx).hide();
        if let Some(restore) = self.restore.take() {
            window.focus(&restore, cx);
        }
        cx.emit(Closed);
        cx.stop_propagation();
    }
}

impl Render for VisualExpand {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let close = div()
            .id("visual-expand-close")
            .debug_selector(|| "visual-expand-close".into())
            .role(Role::Button)
            .aria_label("Close")
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
            .child(icon("x", 15.).text_color(content(0.7)))
            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)));
        div()
            .id("visual-expand")
            .debug_selector(|| "visual-expand".into())
            .role(Role::Dialog)
            .aria_label(format!("Visual: {}", self.title))
            .key_context("VisualExpand")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close(window, cx)))
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .p(px(28.))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .child(
                div()
                    .id("visual-expand-panel")
                    .size_full()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(crate::theme::floating_fill())
                    .overflow_hidden()
                    // Clicks on the panel stay in the panel.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(42.))
                            .flex_shrink_0()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(content(0.08))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(content(1.))
                                    .child(self.title.clone()),
                            )
                            .child(close),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p(px(20.))
                            .child(div().size_full().child(self.page.clone())),
                    ),
            )
    }
}

pub(crate) fn overlay(view: &Entity<VisualExpand>) -> impl IntoElement {
    gpui::deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(view.clone()),
    )
    .with_priority(100)
}
