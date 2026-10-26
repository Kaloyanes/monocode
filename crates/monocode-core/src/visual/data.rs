//! Data files for an HTML visual. The page cannot read files or fetch, so the
//! agent names them and they travel inside the page as one JSON block that
//! the kit exposes as `mc.data`.
use serde_json::{Map, Value};
use std::path::Path;

const MAX_FILES: usize = 8;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROWS: usize = 50_000;
/// The block's opening tag; checks that budget the page find it by this.
pub const BLOCK_START: &str = r#"<script type="application/json" id="mc-data">"#;

/// `data` maps names to file paths: `.json`, `.csv`, `.tsv`, or any other
/// text file, which arrives as a string. Returns the page with the block
/// in front of it.
pub fn embed_data(html: &str, data: &Value) -> Result<String, String> {
    let files = match data {
        Value::Null => return Ok(html.to_owned()),
        Value::Object(files) => files,
        _ => return Err("data must map names to file paths".into()),
    };
    if files.len() > MAX_FILES {
        return Err(format!("A visual may embed at most {MAX_FILES} data files"));
    }
    let mut out = Map::new();
    for (name, path) in files {
        let valid = !name.is_empty()
            && name.len() <= 40
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!(
                "Data name {name:?} must be 1–40 letters, digits or underscores"
            ));
        }
        let path = path.as_str().ok_or("Each data entry must be a file path")?;
        let value = load(path).map_err(|error| format!("Data {name} ({path}): {error}"))?;
        out.insert(name.clone(), value);
    }
    // `<` never appears raw, so the block cannot close itself early.
    let json = serde_json::to_string(&Value::Object(out))
        .map_err(|e| e.to_string())?
        .replace('<', "\\u003c");
    if json.len() > MAX_TOTAL_BYTES {
        return Err("Data files add up to more than 4 MiB once embedded".into());
    }
    Ok(format!("{BLOCK_START}{json}</script>{html}"))
}

fn load(path: &str) -> Result<Value, String> {
    let path = match path.strip_prefix("~/") {
        Some(rest) => Path::new(&std::env::var_os("HOME").ok_or("HOME is not set")?).join(rest),
        None => Path::new(path).to_path_buf(),
    };
    if !path.is_absolute() {
        return Err("use an absolute path".into());
    }
    let length = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if length > MAX_FILE_BYTES {
        return Err("larger than 2 MiB; aggregate it first".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "json" => serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}")),
        "csv" => table(&text, ','),
        "tsv" => table(&text, '\t'),
        _ => Ok(Value::String(text)),
    }
}

/// Rows as objects keyed by the header; numbers become numbers and empty
/// cells become null.
fn table(text: &str, separator: char) -> Result<Value, String> {
    let mut rows = records(text.trim_start_matches('\u{feff}'), separator).into_iter();
    let header = rows.next().ok_or("the file is empty")?;
    let mut out = Vec::new();
    for row in rows {
        if row.len() == 1 && row[0].is_empty() {
            continue;
        }
        if out.len() == MAX_ROWS {
            return Err(format!("more than {MAX_ROWS} rows; aggregate it first"));
        }
        let mut object = Map::new();
        for (i, key) in header.iter().enumerate() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            object.insert(key.clone(), cell_value(cell));
        }
        out.push(Value::Object(object));
    }
    Ok(Value::Array(out))
}

fn cell_value(cell: &str) -> Value {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    let numeric = trimmed.replace('_', "");
    // Leading zeros mark identifiers (zip codes, ids), not quantities.
    let identifier = numeric.len() > 1 && numeric.starts_with('0') && !numeric.starts_with("0.");
    match numeric.parse::<f64>() {
        Ok(number) if number.is_finite() && !identifier => serde_json::Number::from_f64(number)
            .map(|n| {
                if number.fract() == 0.0 && number.abs() < 9e15 {
                    Value::from(number as i64)
                } else {
                    Value::Number(n)
                }
            })
            .unwrap_or(Value::Null),
        _ => Value::String(cell.to_owned()),
    }
}

/// RFC 4180 records: quoted cells may hold separators, quotes and newlines.
fn records(text: &str, separator: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
        } else if c == '"' && cell.is_empty() {
            quoted = true;
        } else if c == separator {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else {
            cell.push(c);
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csv_files_become_rows_with_numbers_and_quoted_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sales.csv");
        std::fs::write(
            &path,
            "\u{feff}month,revenue,note,zip\r\nJan,1200.5,\"Launch, \"\"v2\"\"\",02139\nFeb,,plain,94107\n",
        )
        .unwrap();
        let page = embed_data("<p>x</p>", &json!({"sales": path})).unwrap();
        assert!(page.starts_with(BLOCK_START));
        assert!(page.ends_with("</script><p>x</p>"));
        let block = &page[BLOCK_START.len()..page.find("</script>").unwrap()];
        let value: Value = serde_json::from_str(block).unwrap();
        assert_eq!(
            value["sales"],
            json!([
                {"month":"Jan","revenue":1200.5,"note":"Launch, \"v2\"","zip":"02139"},
                {"month":"Feb","revenue":null,"note":"plain","zip":94107}
            ])
        );
    }

    #[test]
    fn json_text_and_markup_cannot_close_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("a.json");
        std::fs::write(
            &json_path,
            r#"{"html":"</script><script>alert(1)</script>"}"#,
        )
        .unwrap();
        let text_path = dir.path().join("notes.md");
        std::fs::write(&text_path, "# Notes").unwrap();
        let page = embed_data("", &json!({"a": json_path, "notes": text_path})).unwrap();
        assert_eq!(page.matches("</script>").count(), 1);
        let block = &page[BLOCK_START.len()..page.len() - "</script>".len()];
        let value: Value = serde_json::from_str(block).unwrap();
        assert_eq!(value["a"]["html"], "</script><script>alert(1)</script>");
        assert_eq!(value["notes"], "# Notes");
    }

    #[test]
    fn bad_names_paths_and_shapes_are_explained() {
        assert!(embed_data("", &json!({"bad name": "/x.csv"}))
            .unwrap_err()
            .contains("letters, digits or underscores"));
        assert!(embed_data("", &json!({"a": "relative.csv"}))
            .unwrap_err()
            .contains("absolute path"));
        assert!(embed_data("", &json!(["/x.csv"])).is_err());
        assert_eq!(embed_data("<p>", &Value::Null).unwrap(), "<p>");
    }
}
