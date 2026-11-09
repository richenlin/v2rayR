//! A very small YAML writer for `serde_json::Value`.
//!
//! We only ever serialise structures we built ourselves (mihomo's config), so
//! this does not need anchors, tags or multi-line scalars. It does need correct
//! quoting: a proxy name may contain `:`, `#`, quotes, or start with `-`.

use serde_json::Value;

pub fn to_yaml(value: &Value) -> String {
    let mut out = String::new();
    write_block(&mut out, value, 0);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn indent(level: usize) -> String {
    "  ".repeat(level)
}

fn write_block(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                out.push_str(&indent(level));
                out.push_str(&scalar(key));
                out.push(':');
                write_after_key(out, item, level);
            }
        }
        Value::Array(items) => {
            for item in items {
                out.push_str(&indent(level));
                out.push_str("- ");
                match item {
                    Value::Object(map) if !map.is_empty() => {
                        write_sequence_object(out, map, level);
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        out.push('\n');
                        write_block(out, &Value::Array(inner.clone()), level + 1);
                    }
                    other => {
                        write_scalar(out, other);
                        out.push('\n');
                    }
                }
            }
        }
        other => {
            out.push_str(&indent(level));
            write_scalar(out, other);
            out.push('\n');
        }
    }
}

/// Everything that follows `key:` on the same line.
fn write_after_key(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(" {}\n"),
        Value::Array(items) if items.is_empty() => out.push_str(" []\n"),
        Value::Object(_) | Value::Array(_) => {
            out.push('\n');
            write_block(out, value, level + 1);
        }
        other => {
            out.push(' ');
            write_scalar(out, other);
            out.push('\n');
        }
    }
}

/// The first entry of a mapping sits on the `- ` line, the rest are aligned
/// underneath it (`level + 1` indents to the same column as the dash's content).
fn write_sequence_object(out: &mut String, map: &serde_json::Map<String, Value>, level: usize) {
    for (index, (key, item)) in map.iter().enumerate() {
        if index > 0 {
            out.push_str(&indent(level + 1));
        }
        out.push_str(&scalar(key));
        out.push(':');
        write_after_key(out, item, level + 1);
    }
}

fn write_scalar(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => out.push_str(&scalar(text)),
        Value::Array(_) | Value::Object(_) => out.push_str("{}"),
    }
}

/// Quote when the plain form would change meaning.
fn scalar(text: &str) -> String {
    if text.is_empty() {
        return "\"\"".to_owned();
    }
    let first = text.chars().next().unwrap_or(' ');
    let needs_quotes = first.is_whitespace()
        || matches!(
            first,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
        || text.ends_with(char::is_whitespace)
        || text.contains(": ")
        || text.contains(" #")
        || text.contains('\n')
        || text.contains('\t')
        || matches!(
            text.to_ascii_lowercase().as_str(),
            "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~"
        )
        || text.parse::<f64>().is_ok();
    if needs_quotes {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_nested_structures() {
        let value = json!({
            "mixed-port": 10808,
            "dns": {"enable": true, "nameserver": ["1.1.1.1"]},
            "proxies": [{"name": "hkg-01", "type": "vless", "tls": true}],
        });
        let yaml = to_yaml(&value);
        assert!(yaml.contains("mixed-port: 10808\n"), "{yaml}");
        assert!(yaml.contains("  enable: true\n"), "{yaml}");
        assert!(yaml.contains("  - 1.1.1.1\n"), "{yaml}");
        assert!(yaml.contains("  - name: hkg-01\n"), "{yaml}");
        assert!(yaml.contains("    type: vless\n"), "{yaml}");
    }

    #[test]
    fn quotes_ambiguous_scalars() {
        assert_eq!(scalar("true"), "\"true\"");
        assert_eq!(scalar("1.5"), "\"1.5\"");
        assert_eq!(scalar("- node"), "\"- node\"");
        assert_eq!(scalar("a: b"), "\"a: b\"");
        assert_eq!(scalar("plain"), "plain");
        assert_eq!(scalar(""), "\"\"");
    }

    #[test]
    fn empty_containers_are_inline() {
        let value = json!({"a": {}, "b": []});
        assert!(to_yaml(&value).contains("a: {}"));
        assert!(to_yaml(&value).contains("b: []"));
    }

    #[test]
    fn nested_objects_inside_lists_are_indented() {
        let value = json!({"proxies": [{"name": "p", "ws-opts": {"path": "/ws"}}]});
        let yaml = to_yaml(&value);
        assert!(yaml.contains("  - name: p\n"), "{yaml}");
        assert!(yaml.contains("    ws-opts:\n      path: /ws\n"), "{yaml}");
    }
}
