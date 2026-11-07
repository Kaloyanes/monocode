//! The first message of a "Ask about this PR" chat: the reviewer's question
//! with the pull request's description and diff, so the answer is grounded in
//! what changed rather than in whatever branch the project folder is on.
use super::{provider_label, Details, Diff, Item};

/// How much of the patch goes into the message. Larger patches are cut at a
/// line and the agent is told to read the rest from the files.
const PATCH_BUDGET: usize = 60_000;

/// Asked when the reviewer opens the chat without a question.
pub const DEFAULT_QUESTION: &str =
    "Summarize this pull request: what it changes, how, and what I should look at first.";

/// The title of the chat in the session list.
pub fn session_title(item: &Item) -> String {
    format!("{} · Ask · {}", item.reference(), item.title)
}

/// Whether `title` is one `session_title` made.
pub fn is_ask_title(title: &str) -> bool {
    title.contains(" · Ask · ")
}

/// Whether a path an answer cites is `file`, a path in the diff. The cited path
/// may be absolute (the chat resolves it against the project folder) or
/// shorter than the diff's.
pub fn same_file(cited: &str, file: &str) -> bool {
    let cited = cited.trim_start_matches("./");
    let file = file.trim_start_matches("./");
    !cited.is_empty()
        && !file.is_empty()
        && (cited == file
            || cited.ends_with(&format!("/{file}"))
            || file.ends_with(&format!("/{cited}")))
}

pub fn prompt(
    item: &Item,
    details: Option<&Details>,
    diff: Option<&Diff>,
    question: &str,
) -> String {
    let question = question.trim();
    let question = if question.is_empty() {
        DEFAULT_QUESTION
    } else {
        question
    };
    let mut out = format!(
        "I am reviewing a pull request and have questions about it. This chat is read-only: \
don't edit files, push, or post anything. Anything for the author, I will write myself.\n\n\
{} {}: {}\n{}\n",
        provider_label(&item.provider),
        item.reference(),
        item.title,
        item.url
    );
    if let Some(details) = details {
        if !details.head_ref_name.is_empty() || !details.base_ref_name.is_empty() {
            out.push_str(&format!(
                "Branch: {} into {}\n",
                details.head_ref_name, details.base_ref_name
            ));
        }
        if !details.author.is_empty() {
            out.push_str(&format!("Author: {}\n", details.author));
        }
        let body = details.body.trim();
        if !body.is_empty() {
            out.push_str(&format!("\nDescription:\n{body}\n"));
        }
    }
    out.push('\n');
    match diff {
        Some(diff) if !diff.patch.trim().is_empty() => {
            let (patch, omitted) = clip(&diff.patch, PATCH_BUDGET);
            out.push_str(&format!(
                "Diff ({} files, +{} −{}):\n```diff\n{}\n```\n",
                diff.files.len(),
                diff.additions,
                diff.deletions,
                patch.trim_end()
            ));
            if omitted > 0 || diff.truncated {
                out.push_str(
                    "The diff above is incomplete. For the rest, read the files or run `gh pr diff`.\n",
                );
            }
        }
        _ => out
            .push_str("The diff could not be loaded. Get it with `gh pr diff` before answering.\n"),
    }
    out.push_str(
        "\nHow to answer:\n\
- Base every claim on the diff or on code you have read. Say so when you can't tell.\n\
- Cite code as `path:line` using the new file's line numbers. For a deleted line use the old number and say it was deleted.\n\
- The project folder may not be on this pull request's branch. The diff is the truth about what changed; read other files only for context.\n\
- Keep it short. Number the risks.\n\n\
My question:\n",
    );
    out.push_str(question);
    out
}

/// `text` cut at the last line break within `limit` bytes, and how many bytes
/// were left out.
fn clip(text: &str, limit: usize) -> (&str, usize) {
    if text.len() <= limit {
        return (text, 0);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').unwrap_or(end);
    (&text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item {
            provider: "github".into(),
            kind: "pr".into(),
            repo: "acme/app".into(),
            number: 482,
            title: "Add retry to uploader".into(),
            url: "https://github.com/acme/app/pull/482".into(),
            ..Default::default()
        }
    }

    fn diff(patch: &str) -> Diff {
        Diff {
            additions: 2,
            deletions: 1,
            files: vec![super::super::DiffFile {
                path: "src/a.rs".into(),
                additions: 2,
                deletions: 1,
            }],
            patch: patch.into(),
            truncated: false,
        }
    }

    #[test]
    fn the_prompt_carries_the_pull_request_the_diff_and_the_question() {
        let details = Details {
            body: "Retries uploads.".into(),
            author: "sam".into(),
            head_ref_name: "feat/retry".into(),
            base_ref_name: "main".into(),
            ..Default::default()
        };
        let text = prompt(
            &item(),
            Some(&details),
            Some(&diff(
                "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b\n",
            )),
            "Why jitter?",
        );
        assert!(text.contains("#482: Add retry to uploader"));
        assert!(text.contains("https://github.com/acme/app/pull/482"));
        assert!(text.contains("Branch: feat/retry into main"));
        assert!(text.contains("Retries uploads."));
        assert!(text.contains("```diff\n--- a/src/a.rs"));
        assert!(text.contains("read-only"));
        assert!(text.ends_with("My question:\nWhy jitter?"));
        assert!(!text.contains("incomplete"));
    }

    #[test]
    fn no_question_asks_for_a_summary() {
        assert!(prompt(&item(), None, None, "  ").ends_with(DEFAULT_QUESTION));
    }

    #[test]
    fn a_missing_diff_tells_the_agent_to_fetch_it() {
        let text = prompt(&item(), None, None, "q");
        assert!(text.contains("could not be loaded"));
        assert!(!text.contains("```diff"));
    }

    #[test]
    fn a_large_patch_is_cut_at_a_line_and_flagged() {
        let patch = format!("{}\n", "é".repeat(40)).repeat(2_000);
        let text = prompt(&item(), None, Some(&diff(&patch)), "q");
        assert!(text.contains("incomplete"));
        assert!(text.len() < PATCH_BUDGET + 3_000);
        let (kept, omitted) = clip(&patch, PATCH_BUDGET);
        assert!(omitted > 0);
        assert!(!kept.ends_with('\n') && kept.ends_with('é'));
    }

    #[test]
    fn a_truncated_provider_diff_is_flagged_even_when_it_fits() {
        let mut d = diff("@@ -1 +1 @@\n-a\n+b\n");
        d.truncated = true;
        assert!(prompt(&item(), None, Some(&d), "q").contains("incomplete"));
    }

    #[test]
    fn an_ask_chat_is_recognised_by_its_title() {
        assert!(is_ask_title(&session_title(&item())));
        assert!(!is_ask_title("#482: Add retry to uploader"));
    }

    #[test]
    fn a_cited_path_matches_a_diff_file_by_its_tail() {
        assert!(same_file("src/a.rs", "src/a.rs"));
        assert!(same_file("/work/app/src/a.rs", "src/a.rs"));
        assert!(same_file("a.rs", "src/a.rs"));
        assert!(!same_file("/work/app/lib/a.rs", "src/a.rs"));
        assert!(!same_file("src/ba.rs", "a.rs"));
        assert!(!same_file("", "a.rs"));
    }

    #[test]
    fn the_chat_is_titled_with_the_reference() {
        assert_eq!(session_title(&item()), "#482 · Ask · Add retry to uploader");
    }
}
