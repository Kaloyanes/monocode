//! The reviewer's pending comments: a bar above the pull request's tabs that
//! counts them, lists them, and posts them as one review.
use super::*;

impl Inbox {
    pub(super) fn review_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.detail_only {
            return None;
        }
        let comments = self.review_comments();
        if comments.is_empty() && self.review_posted.is_none() && self.review_error.is_none() {
            return None;
        }
        let count = comments.len();
        let posting = self.review_posting;
        let head = div()
            .h(px(36.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(icon("git-pull-request", 14.).text_color(content(0.7)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(match (count, &self.review_posted) {
                        (0, Some(_)) => "Review posted".to_string(),
                        (0, None) => "No pending comments".to_string(),
                        (1, _) => "1 pending comment · only you can see it".to_string(),
                        (n, _) => format!("{n} pending comments · only you can see them"),
                    }),
            )
            .when_some(self.review_posted.clone(), |d, url| {
                d.child(
                    action("inbox-review-open", "Open on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when(count > 0, |d| {
                d.child(
                    action(
                        "inbox-review-toggle",
                        if self.review_open { "Hide" } else { "Show" },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_review(cx))),
                )
                .child(
                    action(
                        "inbox-review-submit",
                        if posting {
                            "Submitting…"
                        } else {
                            "Submit review"
                        },
                    )
                    .px(px(12.))
                    .bg(content(1.))
                    .text_color(shell_fill())
                    .when(posting, |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.submit_review(cx))),
                )
            });
        let list = (self.review_open && count > 0).then(|| {
            div()
                .px(px(12.))
                .pb(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(comments.iter().map(|comment| {
                    let id = comment.id;
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(crate::fonts::mono())
                                .text_size(px(11.))
                                .line_height(px(18.))
                                .text_color(rgb(ACCENT))
                                .child(comment.label()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(content(0.85))
                                .child(comment.text.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("inbox-review-remove-{id}")))
                                .role(Role::Button)
                                .aria_label("Remove comment")
                                .size(px(18.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .text_color(content(0.55))
                                .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                                .child(icon("close", 12.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_review_comment(id, cx)
                                })),
                        )
                }))
        });
        Some(
            div()
                .id("inbox-review-bar")
                .role(Role::Group)
                .aria_label("Pending review comments")
                .flex_shrink_0()
                .border_b_1()
                .border_color(content(0.07))
                .bg(content(0.03))
                .child(head)
                .children(list)
                .when_some(self.review_error.clone(), |d, text| d.child(error(text)))
                .into_any_element(),
        )
    }
}
