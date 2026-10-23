//! Python-style value handling for saved settings. Sessions are loose JSON written by the Python pack and the web UI,
//! so reads coerce the way the pack's `float(x)`, `int(x)`, `str(x or "")` and truthiness checks do.

use serde_json::{Map, Value};

/// A JSON object, the shape of every saved settings dict.
pub type Obj = Map<String, Value>;

/// Python `float(value)`: numbers, bools, numeric strings. `None` where Python would raise.
pub fn to_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite() || s.trim().eq_ignore_ascii_case("inf")),
        _ => None,
    }
}

/// Python truthiness.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `str(value or "")`.
pub fn text_or_empty(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(v) if !truthy(v) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(true)) => "True".into(),
        Some(other) => other.to_string(),
    }
}

/// Python `round()` on a float: ties go to the even integer.
pub fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - x.signum()) as i64
    } else {
        r as i64
    }
}

/// Python `round(x, digits)` for the small digit counts the pack uses.
pub fn round_to(x: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    // Ties are broken on the decimal text, like Python's correctly rounded `round`.
    let scaled = x * scale;
    let nearest = scaled.round();
    let value = if (scaled - scaled.trunc()).abs() == 0.5 {
        let text = format!("{:.*}", digits as usize, x);
        return text.parse().unwrap_or(nearest / scale);
    } else {
        nearest
    };
    value / scale
}

/// Python `float(value or 0)` for a loose JSON value, with Python's error text for unparsable strings.
pub fn float_or_zero(value: Option<&Value>) -> Result<f64, crate::error::CoreError> {
    match value {
        None => Ok(0.0),
        Some(v) if !truthy(v) => Ok(0.0),
        Some(v) => to_float(v).ok_or_else(|| {
            crate::error::CoreError::value(match v {
                Value::String(s) => format!("could not convert string to float: '{s}'"),
                _ => "float() argument must be a string or a real number".to_string(),
            })
        }),
    }
}

/// `f"{x:g}"`: six significant digits, no trailing zeros, exponent form for very small or large values.
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if !x.is_finite() {
        return if x.is_nan() { "nan".into() } else if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: String| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{}e{sign}{:02}", strip(mantissa.to_string()), exp.abs());
    }
    strip(format!("{:.*}", (5 - exp).max(0) as usize, x))
}

/// Python `str(float)` for the common range: `5.0` stays `5.0`, `8.25` stays `8.25`.
pub fn py_float_str(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

/// Python `str(value)` for a JSON value.
pub fn py_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".into(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) if n.is_f64() => py_float_str(n.as_f64().unwrap_or(0.0)),
        Some(other) => other.to_string(),
    }
}

/// `" ".join(text.split())`.
pub fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// First `n` characters (Python slices strings by code point).
pub fn take_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Python `int(value)`: whole numbers, floats (truncated), bools and integer text. `None` where Python would raise.
pub fn to_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().replace('_', "").parse::<i64>().ok(),
        _ => None,
    }
}

/// `_int_payload`: `int(payload.get(key, default))` clamped to `[min, max]`, the default when it is not a number.
pub fn int_payload(payload: &Obj, key: &str, default: i64, min: i64, max: i64) -> i64 {
    let value = payload.get(key).map_or(Some(default), to_int).unwrap_or(default);
    value.clamp(min, max)
}

/// `_float_payload`: `float(payload.get(key, default))` clamped to `[min, max]`.
pub fn float_payload(payload: &Obj, key: &str, default: f64, min: f64, max: f64) -> f64 {
    let value = payload.get(key).map_or(Some(default), to_float).unwrap_or(default);
    max.min(value).max(min)
}

/// `_bool_payload`: text such as `"true"` or `"on"` is true, anything else follows Python truthiness.
pub fn bool_payload(payload: &Obj, key: &str, default: bool) -> bool {
    match payload.get(key) {
        None => default,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Some(v) => truthy(v),
    }
}

/// `_first_payload_value`: the first listed key that is present and not null.
pub fn first_value<'a>(payload: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| payload.get(*k)).find(|v| !v.is_null())
}
