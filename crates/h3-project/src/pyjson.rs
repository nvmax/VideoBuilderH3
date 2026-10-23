//! JSON text the way the pack writes it: `json.dumps(value, indent=2, ensure_ascii=False)`.
//!
//! The Python Video Builder and this app share every session file. Writing the same bytes means a project that
//! neither app changed shows no diff, and floats keep Python's spelling (`1e-05`, `100000.0`, `1e+16`).

use serde_json::Value;

/// Python `repr(float)`.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Shortest round trip digits, as `d.ddde<exp>`. When two candidates of that length are equally close (an exact tie),
    // Rust's shortest form rounds up and Python's rounds to even, so format again at the same length with exact
    // rounding (ties to even).
    let shortest = format!("{x:e}");
    let digit_count = shortest.split('e').next().unwrap_or("").chars().filter(char::is_ascii_digit).count().max(1);
    let sci = format!("{x:.*e}", digit_count - 1);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let head = &digits[..1];
        let tail = &digits[1..];
        let body = if tail.is_empty() { head.to_string() } else { format!("{head}.{tail}") };
        let exp_sign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{body}e{exp_sign}{:02}", exp.abs());
    }
    let point = exp + 1; // digits before the decimal point
    let body = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point as usize >= digits.len() {
        format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    format!("{sign}{body}")
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&float_repr(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&"  ".repeat(depth + 1));
                write_value(out, item, depth + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let last = map.len() - 1;
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(&"  ".repeat(depth + 1));
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, depth + 1);
                out.push_str(if i < last { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
    }
}

/// `json.dumps(value, indent=2, ensure_ascii=False)` without the trailing newline.
pub fn dumps(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

/// What `atomic_write_json` writes: `dumps` plus a newline.
pub fn dumps_file(value: &Value) -> String {
    let mut text = dumps(value);
    text.push('\n');
    text
}

/// Parse JSON text, tolerating a UTF-8 byte order mark (Python reads sessions as `utf-8-sig`).
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
}
