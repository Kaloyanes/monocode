//! Readable, conventional names for the branches and folders of new worktrees.
//! A deterministic slug names the worktree the moment it exists; a text model
//! may later refine only the branch to `mc/<type>/<slug>`.
use crate::{
    provider::{self, Kind, Request},
    session_title,
};
use std::sync::{atomic::AtomicBool, Arc};

pub const PREFIX: &str = "mc/";
/// Conventional-commit types, which also say what the work is.
pub const TYPES: [&str; 9] = [
    "feat", "fix", "refactor", "perf", "docs", "test", "build", "ci", "chore",
];
const MAX_SLUG: usize = 40;
const MAX_WORDS: usize = 5;
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "to", "of", "in", "on", "for", "with", "my", "our", "this",
    "that", "it", "is", "are", "be", "can", "could", "would", "should", "you", "your", "we", "i",
    "me", "please", "pls", "just", "so", "now", "also", "need", "want", "like", "let", "lets",
    "make", "sure", "help", "some", "any", "at", "from", "when", "while", "then", "if", "as",
    "but", "into", "whenever", "every", "each", "how", "what", "why", "where", "does", "do", "did",
    "get", "gets", "back", "again",
];

/// First message → `fix-sidebar-scroll`, or `None` when nothing safe remains.
pub fn heuristic(message: &str) -> Option<String> {
    let line = message.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut words: Vec<String> = Vec::new();
    for token in line.split_whitespace() {
        let private = looks_private(token);
        let word = slugify(token);
        if !private && !word.is_empty() && !STOPWORDS.contains(&word.as_str()) {
            words.push(word);
        }
        // A sentence end finishes the subject; a URL or path may contain dots.
        if words.len() == MAX_WORDS || (!private && token.ends_with(['.', '!', '?'])) {
            break;
        }
    }
    let slug = fit(words.join("-"));
    (slug.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3).then_some(slug)
}

/// Pasted tokens, links and ids must never become a pushed branch name.
fn looks_private(token: &str) -> bool {
    let bare = token.trim_matches(|c: char| !c.is_alphanumeric());
    let digits = bare.chars().filter(char::is_ascii_digit).count();
    token.contains(['/', '\\', ':', '@', '=', '#'])
        || bare.chars().count() > 24
        || digits >= 5
        || (bare.len() >= 8 && bare.chars().all(|c| c.is_ascii_hexdigit()))
        || (bare.len() >= 12 && digits > 0 && bare.chars().any(|c| c.is_ascii_alphabetic()))
}

/// Lowercase ASCII words joined by single dashes.
pub fn slugify(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if c == '\'' || c == '’' {
            continue;
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Cut on a word boundary so a slug never ends mid-word.
fn fit(slug: String) -> String {
    if slug.len() <= MAX_SLUG {
        return slug;
    }
    let cut = &slug[..MAX_SLUG];
    match slug.as_bytes()[MAX_SLUG] {
        b'-' => cut.to_owned(),
        _ => cut
            .rsplit_once('-')
            .map_or(cut, |(head, _)| head)
            .to_owned(),
    }
}

/// `mc/fix/sidebar-scroll`, or `mc/sidebar-scroll` without a known type.
pub fn compose(kind: Option<&str>, slug: &str) -> String {
    match kind {
        Some(kind) => format!("{PREFIX}{kind}/{slug}"),
        None => format!("{PREFIX}{slug}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub kind: Option<String>,
    pub slug: String,
}
impl Suggestion {
    pub fn branch(&self) -> String {
        compose(self.kind.as_deref(), &self.slug)
    }
}

const PROMPT: &str = r#"Name the git branch for this coding task the way a careful engineer would.
Return JSON with exactly two keys: type and slug. Do not call tools. Reply with JSON only.

type is one of: feat, fix, refactor, perf, docs, test, build, ci, chore.
- feat: new behavior or capability
- fix: something broken that should work
- refactor: restructure without changing behavior
- perf: speed or memory
- docs: documentation only
- test: tests only
- build, ci: tooling and pipelines
- chore: anything else, such as dependency bumps or cleanup
Choose the single best fit. When the task is only a question or an investigation, use chore.

slug rules:
- 2-5 lowercase words joined by hyphens, under 36 characters.
- Name the subject and the outcome, for example "sidebar-scroll-jump".
- No type word, ticket number, username, path, URL, or secret from the message.
- Never copy and truncate the user's message."#;

pub fn prompt(message: &str) -> String {
    format!(
        "{PROMPT}\n\nUser message:\n{}",
        crate::response::limit(message, 4_000)
    )
}

pub fn parse(raw: &str) -> Option<Suggestion> {
    let start = raw.find('{')?;
    let value = serde_json::Deserializer::from_str(&raw[start..])
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    let kind = value["type"]
        .as_str()
        .map(|kind| kind.trim().to_lowercase())
        .filter(|kind| TYPES.contains(&kind.as_str()));
    let words: Vec<String> = slugify(value["slug"].as_str()?)
        .split('-')
        .filter(|word| !word.is_empty() && Some(*word) != kind.as_deref())
        .map(str::to_owned)
        .collect();
    let slug = fit(words.join("-"));
    (slug.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3)
        .then_some(Suggestion { kind, slug })
}

/// Runs on a worker, apart from the user's provider thread and transcript.
pub fn generate_for(
    kind: Kind,
    model: Option<String>,
    cwd: &std::path::Path,
    message: &str,
    account: crate::accounts::Profile,
    cancel: Arc<AtomicBool>,
) -> Option<Suggestion> {
    if !session_title::supports(kind) {
        return None;
    }
    let binary = provider::resolve(kind)?;
    let output = provider::run_text(
        Request {
            kind,
            binary,
            cwd: cwd.into(),
            prompt: prompt(message),
            provider_id: None,
            model: model.or_else(|| session_title::cheap_model(kind)),
            settings: crate::agent::Settings {
                account,
                ..Default::default()
            },
        },
        cancel,
    )
    .ok()?;
    parse(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_name_the_subject_and_drop_filler() {
        assert_eq!(
            heuristic("Please fix the sidebar scroll jump when switching tabs").as_deref(),
            Some("fix-sidebar-scroll-jump-switching")
        );
        assert_eq!(
            heuristic("Can you add dark mode to the settings page? Thanks").as_deref(),
            Some("add-dark-mode-settings-page")
        );
        assert_eq!(heuristic("  \n  "), None);
        assert_eq!(heuristic("🙂🙂 日本語"), None);
        assert_eq!(heuristic("a b"), None);
    }

    #[test]
    fn slugs_never_leak_links_secrets_or_ids() {
        let slug = heuristic(
            "fix https://example.com/a?token=abc sk-live1234567890abcdef deadbeefcafe 12345678 user@x.io crash",
        )
        .unwrap();
        assert_eq!(slug, "fix-crash");
        assert_eq!(heuristic("src/app/main.rs failing"), Some("failing".into()));
    }

    #[test]
    fn slugs_are_bounded_and_end_on_a_word() {
        let slug = heuristic(&"internationalization ".repeat(10)).unwrap();
        assert!(slug.len() <= MAX_SLUG);
        assert!(!slug.ends_with('-'));
        assert_eq!(slug, "internationalization");
        let long =
            heuristic("refactorings transcription rendering pipeline internals now").unwrap();
        assert!(long.len() <= MAX_SLUG, "{long}");
        assert!(!long.ends_with('-'));
    }

    #[test]
    fn model_replies_become_conventional_branch_names() {
        let reply = "```json\n{\"type\":\"Fix\",\"slug\":\"Sidebar Scroll Jump!\"}\n```";
        let suggestion = parse(reply).unwrap();
        assert_eq!(suggestion.branch(), "mc/fix/sidebar-scroll-jump");
        // A repeated type word, an unknown type, and a hostile slug.
        assert_eq!(
            parse(r#"{"type":"fix","slug":"fix-login-redirect"}"#)
                .unwrap()
                .branch(),
            "mc/fix/login-redirect"
        );
        assert_eq!(
            parse(r#"{"type":"bug","slug":"login redirect"}"#)
                .unwrap()
                .branch(),
            "mc/login-redirect"
        );
        assert_eq!(
            parse(r#"{"type":"feat","slug":"../../etc/passwd"}"#)
                .unwrap()
                .branch(),
            "mc/feat/etc-passwd"
        );
        assert!(parse(r#"{"type":"feat","slug":"--"}"#).is_none());
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"type":"feat"}"#).is_none());
    }

    #[test]
    #[ignore = "live check: asks the installed provider CLIs to name branches"]
    fn installed_providers_name_real_requests_and_a_real_worktree_is_renamed() {
        let prompts = [
            "the sidebar jumps back to the top whenever I switch tabs, please fix it",
            "add a dark mode toggle to the settings page",
            "can you explain how the checkpoint code decides what to snapshot?",
            "rename the Conversation struct fields to snake_case and split title.rs, no behavior change",
            "bump serde to 1.0.220 https://github.com/serde-rs/serde/pull/2999 token sk-live1234567890abcdef",
        ];
        let mut ran = 0;
        for kind in [Kind::Claude, Kind::Codex] {
            if provider::resolve(kind).is_none() {
                continue;
            }
            for prompt in prompts {
                let started = std::time::Instant::now();
                let found = generate_for(
                    kind,
                    None,
                    std::env::temp_dir().as_path(),
                    prompt,
                    Default::default(),
                    Arc::new(AtomicBool::new(false)),
                );
                println!(
                    "{:?} {:>5.1}s  {:<60} -> {:?}",
                    kind,
                    started.elapsed().as_secs_f32(),
                    &prompt[..prompt.len().min(60)],
                    found.as_ref().map(Suggestion::branch)
                );
                let branch = found.expect("the provider returned a usable name").branch();
                assert!(
                    crate::git::git(
                        std::env::temp_dir().as_path(),
                        &["check-ref-format", "--branch", &branch]
                    )
                    .is_ok(),
                    "{branch}"
                );
                assert!(
                    !branch.contains("sk-live") && !branch.contains("2999"),
                    "{branch}"
                );
                ran += 1;
            }
        }
        assert!(ran > 0, "no provider CLI is installed");

        // End to end: create from a request, then refine with a real model.
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let run = |args: &[&str]| crate::git::git(&cwd, args).unwrap();
        run(&["init", "-b", "main"]);
        std::fs::write(cwd.join("file"), "x\n").unwrap();
        run(&["add", "file"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "Initial",
        ]);
        let message = "the sidebar jumps back to the top whenever I switch tabs, please fix it";
        let tree = crate::git::create_worktree(&cwd, "main", 1, Some(message)).unwrap();
        let provisional = crate::git::worktree_branch(&tree).unwrap();
        let wanted = generate_for(
            Kind::Claude,
            None,
            &tree,
            message,
            Default::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("claude names the branch")
        .branch();
        let final_name =
            crate::git::rename_worktree_branch(&cwd, &tree, &provisional, &wanted).unwrap();
        println!(
            "folder {:?}\nprovisional {provisional}\nfinal {final_name}",
            tree.file_name().unwrap()
        );
        assert_eq!(
            crate::git::worktree_branch(&tree).as_deref(),
            Some(final_name.as_str())
        );
        assert!(tree.is_dir());
    }
}
