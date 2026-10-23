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
