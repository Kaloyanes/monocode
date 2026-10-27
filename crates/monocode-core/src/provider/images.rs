//! Images returned inside tool results, such as browser screenshots or image
//! generators. Built-in file reads are excluded by callers: the agent only
//! looked at an existing file, it did not produce one.
use super::{Event, ImageOutput};
use serde_json::Value;

/// Collects Anthropic `{type:image, source:{type:base64, media_type, data}}`
/// and MCP/ACP `{type:image, data, mimeType}` blocks, including ACP's
/// `{type:content, content:…}` wrappers.
pub(super) fn collect(value: &Value, title: &str, events: &mut Vec<Event>) {
    // Every draft_preview call returns the page's screenshot for the agent.
    // The person watches the page in the Draft panel; only a kept draft
    // becomes a picture in the conversation.
    if title.ends_with("draft_preview") {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect(value, title, events);
            }
        }
        Value::Object(_) if value["type"] == "image" => {
            let (data, mime) = if value["source"]["type"] == "base64" {
                (&value["source"]["data"], &value["source"]["media_type"])
            } else {
                (&value["data"], &value["mimeType"])
            };
            if let (Some(data), Some(mime)) = (data.as_str(), mime.as_str()) {
                events.push(Event::Image(ImageOutput {
                    data: data.to_owned(),
                    mime_type: mime.to_owned(),
                    title: readable(title),
                }));
            }
        }
        Value::Object(_) => {
            if let Some(content) = value.get("content") {
                collect(content, title, events);
            }
        }
        _ => {}
    }
}

/// `mcp__browser__take_screenshot` or `browser:take_screenshot` becomes
/// "take screenshot"; prose titles from ACP agents pass through.
fn readable(tool: &str) -> String {
    let name = tool
        .rsplit("__")
        .next()
        .and_then(|name| name.rsplit(':').next())
        .unwrap_or(tool);
    if name.contains(' ') {
        name.to_owned()
    } else {
        name.replace('_', " ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn images(value: Value) -> Vec<(String, String)> {
        let mut events = Vec::new();
        collect(&value, "Screenshot", &mut events);
        events
            .into_iter()
            .map(|event| match event {
                Event::Image(image) => (image.data, image.mime_type),
                _ => panic!("only images are collected"),
            })
            .collect()
    }

    #[test]
    fn draft_screenshots_stay_out_of_the_transcript() {
        let result = json!([{"type":"image","data":"AAA","mimeType":"image/png"}]);
        for tool in ["mcp__monocode__draft_preview", "monocode:draft_preview"] {
            let mut events = Vec::new();
            collect(&result, tool, &mut events);
            assert!(events.is_empty(), "{tool}");
        }
        let mut events = Vec::new();
        collect(&result, "mcp__browser__screenshot", &mut events);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn anthropic_mcp_and_acp_shapes_are_collected_and_text_is_ignored() {
        let anthropic = json!([{"type":"text","text":"ok"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}}]);
        let mcp = json!({"content":[{"type":"image","data":"BBB","mimeType":"image/jpeg"}]});
        let acp = json!([{"type":"content","content":{"type":"image","data":"CCC","mimeType":"image/webp"}}]);
        assert_eq!(images(anthropic), [("AAA".into(), "image/png".into())]);
        assert_eq!(images(mcp), [("BBB".into(), "image/jpeg".into())]);
        assert_eq!(images(acp), [("CCC".into(), "image/webp".into())]);
        assert!(images(json!("text only")).is_empty());
    }

    #[test]
    fn tool_names_become_readable_titles() {
        assert_eq!(readable("mcp__browser__take_screenshot"), "take screenshot");
        assert_eq!(readable("playwright:browser_snapshot"), "browser snapshot");
        assert_eq!(readable("Generate a logo"), "Generate a logo");
    }
}
