//! Visualizations: what draws the drafts agents build in the thread.
use crate::{
    chromium::{self, Install},
    right_panel::tabular,
    settings::{
        SettingsView,
        widgets::{RED, note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, div, prelude::*, px};

fn megabytes(bytes: u64) -> u64 {
    (bytes + 500_000) / 1_000_000
}

impl SettingsView {
    pub(super) fn visualizations_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !cfg!(target_os = "macos") {
            return vec![self.group(
                Some("page-engine"),
                true,
                "Page engine",
                None,
                None,
                vec![note("Drafts need the macOS app.")],
            )];
        }
        let p = self.prefs.clone();
        let install = chromium::state(cx);
        let installed = chromium::installed();
        let chosen = if p.page_engine == "chromium" {
            "chromium"
        } else {
            "webkit"
        };
        let mut rows = vec![self.row(
            Some("page-engine"),
            true,
            "Page engine",
            Some(
                "WebKit is built into macOS. Chromium renders pages the way Chrome does and draws them as part of the thread, so menus and dialogs sit cleanly over a draft; it is downloaded the first time you choose it. Open drafts switch when you change it."
                    .into(),
            ),
            vec![self.segmented(
                "page-engine",
                "Page engine",
                chosen,
                &[("webkit", "WebKit"), ("chromium", "Chromium")],
                cx,
                |this, value, cx| {
                    this.update(cx, |p| p.page_engine = value.into());
                    if value == "chromium" {
                        chromium::install(cx);
                    }
                },
            )],
        )];
        let (label, description, controls): (&str, String, Vec<AnyElement>) = match install {
            Install::Downloading { received, total } => (
                "Downloading Chromium",
                "Drafts use WebKit until it is ready.".into(),
                vec![
                    amount(format!("{} / {} MB", megabytes(received), megabytes(total))),
                    progress(received as f32 / total.max(1) as f32),
                ],
            ),
            Install::Unpacking => (
                "Unpacking Chromium",
                "Almost ready. Drafts use WebKit until it is.".into(),
                vec![progress(1.)],
            ),
            Install::Failed(error) => (
                "Chromium could not be installed",
                format!("{error} Drafts use WebKit meanwhile."),
                vec![
                    secondary_button("chromium-retry", "Try again", false, false)
                        .child("Try again")
                        .on_click(cx.listener(|_, _, _, cx| chromium::install(cx)))
                        .into_any_element(),
                ],
            ),
            Install::Idle if installed && chosen == "chromium" && chromium::failure().is_some() => (
                "Chromium could not start",
                format!(
                    "{} Drafts use WebKit instead. Try again starts Chromium for the next draft.",
                    sentence(&chromium::failure().unwrap_or_default())
                ),
                vec![
                    secondary_button("chromium-start-again", "Try again", false, false)
                        .child("Try again")
                        .on_click(cx.listener(|_, _, _, cx| {
                            chromium::report_failure(None);
                            cx.notify();
                        }))
                        .into_any_element(),
                ],
            ),
            Install::Idle if installed => (
                "Chromium",
                format!(
                    "Version {} (CEF {}). Drafts that use it switch to WebKit if you remove it.",
                    chromium::CHROMIUM,
                    chromium::VERSION
                ),
                vec![
                    amount(format!("{} MB", megabytes(chromium::disk_size()))),
                    secondary_button("chromium-remove", "Remove Chromium", true, false)
                        .child(icon("trash", 14.).text_color(gpui::rgb(RED)))
                        .child("Remove")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if chromium::remove(cx).is_ok() {
                                this.update(cx, |p| p.page_engine = "webkit".into());
                            }
                        }))
                        .into_any_element(),
                ],
            ),
            Install::Idle if chosen == "chromium" => (
                "Chromium is not downloaded",
                format!(
                    "About {} MB. Drafts use WebKit until it is here.",
                    megabytes(chromium::download_size())
                ),
                vec![
                    secondary_button("chromium-download", "Download", false, false)
                        .child("Download")
                        .on_click(cx.listener(|_, _, _, cx| chromium::install(cx)))
                        .into_any_element(),
                ],
            ),
            Install::Idle => (
                "Chromium",
                format!(
                    "Not downloaded. Choosing it downloads about {} MB.",
                    megabytes(chromium::download_size())
                ),
                Vec::new(),
            ),
        };
        rows.push(self.row(
            Some("chromium-download"),
            false,
            label,
            Some(description.into()),
            controls,
        ));
        vec![self.group(None, true, "Drafts", None, None, rows)]
    }
}

/// A reason as a sentence of its own.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// A size or count beside a control, in figures that do not shift as they change.
fn amount(text: String) -> AnyElement {
    div()
        .text_size(px(12.))
        .text_color(content(0.55))
        .font_features(tabular())
        .whitespace_nowrap()
        .child(text)
        .into_any_element()
}

/// A thin bar for the download's progress.
fn progress(fraction: f32) -> AnyElement {
    div()
        .w(px(160.))
        .h(px(4.))
        .rounded_full()
        .bg(content(0.1))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(px(160. * fraction.clamp(0., 1.)))
                .bg(content(0.6)),
        )
        .into_any_element()
}
