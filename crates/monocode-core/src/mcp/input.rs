//! Turns whatever the user pasted into one MCP server, and says which
//! providers can take it and where it would be written.
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{claude_desktop_config, dirs_home, expand_home, opencode_config_path};

/// One server read out of pasted text. `name` is empty when the text had none.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedServer {
    pub name: String,
    pub server: Value,
    /// What the text looked like: "JSON", "CLI command", "Shell command" or "URL".
    pub source: &'static str,
}

/// `Ok(None)` for empty text, an error that tells the user what to paste instead
/// when nothing is recognised.
pub fn parse_input(text: &str) -> Result<Option<ParsedServer>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.starts_with('{') {
        return from_json(text).map(Some);
    }
    if text.starts_with("http://") || text.starts_with("https://") {
        if text.contains(char::is_whitespace) {
            return Err("A URL cannot contain spaces.".into());
        }
        let url = url::Url::parse(text).map_err(|_| "That URL is not valid.".to_owned())?;
        return Ok(Some(ParsedServer {
            name: name_from_host(&url),
            server: json!({ "type": "http", "url": text }),
            source: "URL",
        }));
    }
    let words = split_words(text);
    match words.first().map(String::as_str) {
        Some("claude" | "codex")
            if words.get(1).map(String::as_str) == Some("mcp")
                && words.get(2).map(String::as_str) == Some("add") =>
        {
            from_cli(&words).map(Some)
        }
        Some(
            "npx" | "bunx" | "pnpx" | "pnpm" | "yarn" | "bun" | "uvx" | "uv" | "node" | "deno"
            | "python" | "python3" | "docker",
        ) => Ok(Some(from_runner(&words))),
        _ => Err(
            "Not recognised. Paste JSON, a \u{201c}claude mcp add\u{201d} command, an npx or uvx command, or a URL."
                .into(),
        ),
    }
}

fn from_json(text: &str) -> Result<ParsedServer, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or("Server configuration must be an object")?;
    let wrapper = ["mcpServers", "servers", "mcp"]
        .iter()
        .find_map(|key| object.get(*key));
    let (name, server) = if let Some(servers) = wrapper {
        let servers = servers.as_object().ok_or("mcpServers must be an object")?;
        if servers.len() != 1 {
            return Err(format!(
                "Add one server at a time. This has {}.",
                servers.len()
            ));
        }
        let (name, server) = servers.iter().next().unwrap();
        (name.clone(), server.clone())
    } else if object.contains_key("command") || object.contains_key("url") {
        (String::new(), value.clone())
    } else if let Some((name, server)) = object
        .iter()
        .next()
        .filter(|(_, v)| object.len() == 1 && v.is_object())
    {
        (name.clone(), server.clone())
    } else {
        return Err("Needs a command or a URL.".into());
    };
    check_shape(&server)?;
    Ok(ParsedServer {
        name,
        server,
        source: "JSON",
    })
}

/// A server is either a local command or a remote URL, never both or neither.
fn check_shape(server: &Value) -> Result<(), String> {
    let object = server
        .as_object()
        .ok_or("Server configuration must be an object")?;
    let command = match object.get("command") {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        _ => false,
    };
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if command == url {
        return Err("Needs either a command or a URL, not both.".into());
    }
    Ok(())
}

fn from_cli(words: &[String]) -> Result<ParsedServer, String> {
    let mut transport = None;
    let mut url_flag = None;
    let mut bearer = None;
    let mut env = Map::new();
    let mut headers = Map::new();
    let mut positional = Vec::new();
    let mut rest = None;
    let mut i = 3;
    while i < words.len() {
        let word = &words[i];
        i += 1;
        if word == "--" {
            rest = Some(words[i..].to_vec());
            break;
        }
        let (flag, inline) = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (word.as_str(), None),
        };
        let has_inline = inline.is_some();
        match flag {
            "--transport" | "-t" => transport = take(words, &mut i, inline),
            "--scope" | "-s" => {
                take(words, &mut i, inline);
            }
            "--url" => url_flag = take(words, &mut i, inline),
            "--bearer-token-env-var" => bearer = take(words, &mut i, inline),
            "--env" | "-e" => {
                // Claude's -e takes several KEY=value pairs in a row.
                let mut next = take(words, &mut i, inline);
                while let Some(pair) = next {
                    if let Some((key, value)) = pair.split_once('=') {
                        env.insert(key.to_owned(), json!(value));
                    }
                    next = (!has_inline)
                        .then(|| words.get(i))
                        .flatten()
                        .filter(|w| w.contains('=') && !w.starts_with('-'))
                        .cloned();
                    if next.is_some() {
                        i += 1;
                    }
                }
            }
            "--header" | "-H" => {
                if let Some(header) = take(words, &mut i, inline) {
                    if let Some((key, value)) = header.split_once(':') {
                        headers.insert(key.trim().to_owned(), json!(value.trim()));
                    }
                }
            }
            flag if flag.starts_with('-') => {}
            _ => positional.push(word.clone()),
        }
    }
    let name = positional.first().cloned().unwrap_or_default();
    let mut server = if let Some(command) = rest {
        let (program, args) = command
            .split_first()
            .ok_or("Missing a command after \u{201c}--\u{201d}.")?;
        json!({ "command": program, "args": args })
    } else if let Some(url) = url_flag.or_else(|| {
        positional
            .get(1)
            .filter(|p| p.starts_with("http://") || p.starts_with("https://"))
            .cloned()
    }) {
        let kind = if transport.as_deref() == Some("sse") {
            "sse"
        } else {
            "http"
        };
        json!({ "type": kind, "url": url })
    } else if let Some(program) = positional.get(1) {
        json!({ "command": program, "args": &positional[2..] })
    } else {
        return Err("Missing a command or a URL.".into());
    };
    let object = server.as_object_mut().unwrap();
    if !env.is_empty() {
        object.insert("env".into(), Value::Object(env));
    }
    if !headers.is_empty() {
        object.insert("headers".into(), Value::Object(headers));
    }
    if let Some(var) = bearer {
        object.insert("bearerTokenEnvVar".into(), json!(var));
    }
    Ok(ParsedServer {
        name,
        server,
        source: "CLI command",
    })
}

fn take(words: &[String], at: &mut usize, inline: Option<String>) -> Option<String> {
    inline.or_else(|| {
        let word = words.get(*at).cloned();
        if word.is_some() {
            *at += 1;
        }
        word
    })
}

fn from_runner(words: &[String]) -> ParsedServer {
    let program = &words[0];
    let args = &words[1..];
    let subject = if program == "docker" {
        args.iter().rev().find(|a| !a.starts_with('-'))
    } else {
        args.iter()
            .find(|a| !a.starts_with('-') && !matches!(a.as_str(), "dlx" | "exec" | "run" | "x"))
    };
    ParsedServer {
        name: subject.map(|s| name_from_package(s)).unwrap_or_default(),
        server: json!({ "command": program, "args": args }),
        source: "Shell command",
    }
}

/// Splits on whitespace, keeping quoted text together.
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let text = text.replace("\\\n", " ");
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// `@scope/server-docs-mcp@latest` becomes `docs`.
fn name_from_package(arg: &str) -> String {
    let base = arg.rsplit('/').next().unwrap_or(arg);
    let base = base.split('@').next().unwrap_or(base);
    let base = base.rsplit_once('.').map_or(base, |(stem, ext)| {
        if matches!(ext, "js" | "mjs" | "cjs" | "ts" | "py") {
            stem
        } else {
            base
        }
    });
    let mut name = base;
    for prefix in ["mcp-server-", "server-", "mcp-"] {
        name = name.strip_prefix(prefix).unwrap_or(name);
    }
    for suffix in ["-mcp-server", "-server", "-mcp"] {
        name = name.strip_suffix(suffix).unwrap_or(name);
    }
    let name = sanitize(name);
    if name.is_empty() {
        "server".into()
    } else {
        name
    }
}

/// `https://mcp.linear.app/mcp` becomes `linear`.
fn name_from_host(url: &url::Url) -> String {
    let host = match url.host() {
        Some(url::Host::Domain(host)) => host,
        _ => return "server".into(),
    };
    let mut host = host;
    for prefix in ["www.", "mcp.", "api."] {
        host = host.strip_prefix(prefix).unwrap_or(host);
    }
    let name = sanitize(host.split('.').next().unwrap_or(host));
    if name.is_empty() {
        "server".into()
    } else {
        name
    }
}

/// Why `name` cannot be used with `provider`, or `None`.
pub fn name_issue(provider: &str, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Give it a name.".into());
    }
    if name.chars().any(char::is_control) {
        return Some("The name cannot contain control characters.".into());
    }
    if provider != "opencode"
        && !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Some("Names can use letters, numbers, hyphens and underscores.".into());
    }
    None
}

/// Why `provider` cannot keep this server as written, or `None`.
pub fn provider_issue(provider: &str, server: &Value) -> Option<String> {
    match provider {
        "claude_desktop" if server.get("command").is_none() => {
            Some("Claude Desktop only takes local servers (it needs a command).".into())
        }
        "codex" => codex_issue(server),
        _ => None,
    }
}

/// The Codex CLI can only be given a few fields; anything else would be dropped.
pub(super) fn codex_issue(config: &Value) -> Option<String> {
    let object = config.as_object()?;
    let remote = object.get("url").and_then(Value::as_str);
    let allowed: &[&str] = if remote.is_some() {
        &["type", "url", "bearerTokenEnvVar"]
    } else {
        &["type", "command", "args", "env"]
    };
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Some(format!(
            "Codex cannot keep '{key}' when adding; edit its config file instead."
        ));
    }
    let kind = object.get("type").and_then(Value::as_str);
    if remote.is_some() && !matches!(kind, None | Some("http") | Some("remote")) {
        return Some("Codex supports HTTP URLs only.".into());
    }
    if remote.is_none() && !matches!(kind, None | Some("stdio") | Some("local")) {
        return Some("Local servers must be stdio.".into());
    }
    None
}

/// Where adding to `provider` at `scope` writes, shortened for display.
pub fn destination(provider: &str, scope: &str, cwd: &str) -> String {
    let Some(home) = dirs_home() else {
        return String::new();
    };
    let home = Path::new(&home);
    let project = expand_home(cwd);
    let path = match (provider, scope) {
        ("claude", "project") => return ".mcp.json".into(),
        ("claude", "local") => return "~/.claude.json \u{b7} this project".into(),
        ("claude", _) => return "~/.claude.json".into(),
        ("codex", _) => return "~/.codex/config.toml".into(),
        ("cursor", "project") => return ".cursor/mcp.json".into(),
        ("cursor", _) => return "~/.cursor/mcp.json".into(),
        ("claude_desktop", _) => claude_desktop_config(home),
        ("opencode", scope) => {
            let override_path = std::env::var_os("OPENCODE_CONFIG").map(std::path::PathBuf::from);
            opencode_config_path(home, &project, scope, override_path.as_deref())
        }
        _ => return String::new(),
    };
    if scope != "user" {
        if let Ok(relative) = path.strip_prefix(&project) {
            return relative.display().to_string();
        }
    }
    match path.strip_prefix(home) {
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ParsedServer {
        parse_input(text).unwrap().unwrap()
    }

    #[test]
    fn reads_json_with_and_without_a_wrapper() {
        let wrapped = parse(r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","x"]}}}"#);
        assert_eq!((wrapped.name.as_str(), wrapped.source), ("docs", "JSON"));
        let bare = parse(r#"{"url":"https://example.com/mcp"}"#);
        assert_eq!(bare.name, "");
        let keyed = parse(r#"{"docs":{"command":"npx"}}"#);
        assert_eq!(keyed.name, "docs");
        assert!(
            parse_input(r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}"#).is_err()
        );
        assert!(parse_input(r#"{"command":"x","url":"https://a.b"}"#).is_err());
        assert!(parse_input(r#"{"mcpServers":{"a":{"command":"x" "args":[]}}}"#).is_err());
        assert!(parse_input("   ").unwrap().is_none());
    }

    #[test]
    fn reads_claude_add_commands() {
        let http = parse("claude mcp add --transport http linear https://mcp.linear.app/mcp");
        assert_eq!(http.name, "linear");
        assert_eq!(
            http.server,
            json!({"type":"http","url":"https://mcp.linear.app/mcp"})
        );
        let local =
            parse("claude mcp add -s user -e KEY=1 TOKEN=\"a b\" docs -- npx -y @acme/docs-mcp");
        assert_eq!(local.name, "docs");
        assert_eq!(local.server["command"], "npx");
        assert_eq!(local.server["args"], json!(["-y", "@acme/docs-mcp"]));
        assert_eq!(local.server["env"], json!({"KEY":"1","TOKEN":"a b"}));
        let header = parse(
            "claude mcp add --transport sse --header \"Authorization: Bearer x\" feed https://a.b/sse",
        );
        assert_eq!(header.server["type"], "sse");
        assert_eq!(header.server["headers"]["Authorization"], "Bearer x");
        assert!(parse_input("claude mcp add docs").is_err());
    }

    #[test]
    fn names_servers_from_packages_and_hosts() {
        assert_eq!(parse("npx -y @upstash/context7-mcp").name, "context7");
        assert_eq!(
            parse("npx -y @modelcontextprotocol/server-filesystem@latest ~/Dev").name,
            "filesystem"
        );
        assert_eq!(parse("uvx mcp-server-git").name, "git");
        assert_eq!(parse("node /opt/tools/search.js").name, "search");
        let url = parse("https://mcp.notion.com/mcp");
        assert_eq!((url.name.as_str(), url.source), ("notion", "URL"));
        assert!(parse_input("hello there").is_err());
    }

    #[test]
    fn says_what_each_provider_cannot_take() {
        let remote = json!({"type":"http","url":"https://a.b/mcp"});
        let local = json!({"command":"npx","args":["x"]});
        assert!(provider_issue("claude_desktop", &remote).is_some());
        assert!(provider_issue("claude_desktop", &local).is_none());
        assert!(provider_issue("claude", &remote).is_none());
        assert!(provider_issue("codex", &remote).is_none());
        let headers = json!({"url":"https://a.b/mcp","headers":{"A":"b"}});
        assert!(provider_issue("codex", &headers)
            .unwrap()
            .contains("headers"));
        assert!(provider_issue("codex", &json!({"type":"sse","url":"https://a.b"})).is_some());
        assert!(name_issue("claude", "my server").is_some());
        assert!(name_issue("opencode", "my server").is_none());
        assert!(name_issue("claude", "  ").is_some());
    }

    #[test]
    fn codex_issue_agrees_with_what_the_cli_path_refuses() {
        let cases = [
            json!({"url":"https://a.b","headers":{}}),
            json!({"type":"sse","url":"https://a.b"}),
            json!({"command":"x","cwd":"/tmp"}),
            json!({"type":"http","command":"x"}),
        ];
        for case in cases {
            assert!(codex_issue(&case).is_some(), "{case}");
            assert!(
                super::super::mcp_add_args("codex", "user", "docs", &case, Some("/bin/codex"))
                    .is_err(),
                "{case}"
            );
        }
        let ok = json!({"command":"x","args":["y"],"env":{"A":"b"}});
        assert!(codex_issue(&ok).is_none());
        assert!(
            super::super::mcp_add_args("codex", "user", "docs", &ok, Some("/bin/codex")).is_ok()
        );
    }
}
