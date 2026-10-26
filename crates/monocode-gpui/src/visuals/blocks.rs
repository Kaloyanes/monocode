//! Code, diff and callout blocks for agent visuals. They reuse the review
//! diff's engine and the transcript's syntax palette, so an agent's diff reads
//! like the app's own.
use super::color;
use crate::{
    diff_view::{expand_tabs, highlight_file},
    markdown::MONO,
    right_panel::{DIFF_ADD, DIFF_DEL},
    syntax::{HighlightRequest, HighlightSpan},
    theme::content,
};
use gpui::{
    AnyElement, FontWeight, HighlightStyle, IntoElement, SharedString, StyledText, div, prelude::*,
    px, rgb,
};
use monocode_core::{
    diff::{self, Block, LineKind},
    visual::ThemeColor,
};
use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    ops::Range,
    rc::Rc,
};

const LINE: f32 = 18.;
const CODE_SIZE: f32 = 12.;
const CACHE_ENTRIES: usize = 32;

type Spans = Rc<Vec<Vec<(Range<usize>, u32)>>>;
type Cache = RefCell<HashMap<u64, Spans>>;

thread_local! {
    // Every interaction rerenders the tree; highlighting an unchanged block
    // again would repeat the same syntect pass on the UI thread.
    static CACHE: Cache = RefCell::new(HashMap::new());
}

fn cached(key: impl Hash, compute: impl FnOnce() -> Vec<Vec<(Range<usize>, u32)>>) -> Spans {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let key = hasher.finish();
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(hit) = cache.get(&key) {
            return hit.clone();
        }
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
        let value = Rc::new(compute());
        cache.insert(key, value.clone());
        value
    })
}

fn pairs(spans: &[HighlightSpan]) -> Vec<(Range<usize>, u32)> {
    spans.iter().map(|s| (s.range.clone(), s.color)).collect()
}

fn styled(text: String, spans: Option<&[(Range<usize>, u32)]>) -> StyledText {
    let styles: Vec<_> = spans
        .unwrap_or(&[])
        .iter()
        .filter(|(range, _)| range.end <= text.len())
        .map(|(range, color)| {
            (
                range.clone(),
                HighlightStyle {
                    color: Some(rgb(*color).into()),
                    ..Default::default()
                },
            )
        })
        .collect();
    StyledText::new(text).with_highlights(styles)
}

fn frame(key: &str, title: Option<&str>, lines: Vec<AnyElement>) -> AnyElement {
    let selector = format!("visual-block-{key}");
    div()
        .debug_selector(move || selector)
        .id(SharedString::from(format!("visual-block-{key}")))
        .w_full()
        .min_w_0()
        .rounded(px(8.))
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.03))
        .overflow_hidden()
        .flex()
        .flex_col()
        .when_some(title, |d, title| {
            d.child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(content(0.08))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.65))
                    .truncate()
                    .child(title.to_owned()),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("visual-block-scroll-{key}")))
                .w_full()
                .overflow_x_scroll()
                .py_2()
                .flex()
                .flex_col()
                .font_family(MONO)
                .text_size(px(CODE_SIZE))
                .children(lines),
        )
        .into_any_element()
}

pub(super) fn code(
    key: &str,
    code: &str,
    language: Option<&str>,
    title: Option<&str>,
) -> AnyElement {
    let spans = cached((code, language), || {
        crate::syntax::highlight(HighlightRequest {
            source: code,
            path: None,
            fence_tag: language,
        })
        .map(|doc| doc.lines.iter().map(|l| pairs(l)).collect())
        .unwrap_or_default()
    });
    let lines = code
        .lines()
        .enumerate()
        .map(|(ix, line)| {
            let text = expand_tabs(line).into_owned();
            div()
                .h(px(LINE))
                .line_height(px(LINE))
                .px_3()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(styled(text, spans.get(ix).map(|s| s.as_slice())))
                .into_any_element()
        })
        .collect();
    frame(key, title, lines)
}

pub(super) fn diff(
    key: &str,
    before: &str,
    after: &str,
    path: Option<&str>,
    title: Option<&str>,
    context: usize,
) -> AnyElement {
    let file = diff::build_unified_file(before, after, context);
    let blocks = file.blocks;
    let tokens = highlight_file(path.unwrap_or(""), &blocks);
    let mut rows: Vec<AnyElement> = Vec::new();
    for (b, block) in blocks.iter().enumerate() {
        if let Block::Fold { lines, .. } = block {
            rows.push(
                div()
                    .h(px(LINE + 6.))
                    .px_3()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .bg(content(0.04))
                    .text_color(content(0.45))
                    .child(format!("{} unchanged lines", lines.len()))
                    .into_any_element(),
            );
            continue;
        }
        for (i, line) in block.lines().iter().enumerate() {
            if line.kind == LineKind::Hunk {
                continue;
            }
            let (mark, ink) = match line.kind {
                LineKind::Add => ("+", Some(rgb(DIFF_ADD))),
                LineKind::Del => ("−", Some(rgb(DIFF_DEL))),
                _ => (" ", None),
            };
            let number = if line.kind == LineKind::Del {
                line.old_number
            } else {
                line.new_number
            };
            let spans = tokens
                .as_ref()
                .and_then(|t| t.get(b))
                .and_then(|t| t.get(i))
                .map(|s| pairs(s));
            rows.push(
                div()
                    .h(px(LINE))
                    .line_height(px(LINE))
                    .flex()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when_some(ink, |d, ink| d.bg(gpui::Hsla::from(ink).opacity(0.08)))
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .pr_2()
                            .text_right()
                            .text_size(px(11.))
                            .text_color(content(0.35))
                            .child(number.map(|n| n.to_string()).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .w(px(16.))
                            .flex_shrink_0()
                            .text_color(ink.map(gpui::Hsla::from).unwrap_or_else(|| content(0.35)))
                            .child(mark),
                    )
                    .child(div().pr_3().child(styled(
                        expand_tabs(&line.text).into_owned(),
                        spans.as_deref(),
                    )))
                    .into_any_element(),
            );
        }
    }
    if rows.is_empty() {
        rows.push(
            div()
                .px_3()
                .py_1()
                .text_color(content(0.5))
                .child("No changes")
                .into_any_element(),
        );
    }
    let heading = title
        .or(path)
        .map(|t| format!("{t}  +{} −{}", file.additions, file.deletions));
    frame(key, heading.as_deref(), rows)
}

pub(super) fn callout(text: &str, title: Option<&str>, token: &ThemeColor) -> AnyElement {
    let ink = color(token);
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded(px(8.))
        .border_l_2()
        .border_color(ink)
        .bg(ink.opacity(0.08))
        .when_some(title, |d, title| {
            d.child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ink)
                    .child(title.to_owned()),
            )
        })
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(content(0.85))
                .child(text.to_owned()),
        )
        .into_any_element()
}
