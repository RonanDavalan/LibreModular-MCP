//! Single exit point for everything the Hub returns, writes or logs.
//!
//! Why this module exists
//! Protecting one output path at a time leaves the others open. Every payload goes
//! through `finalize`, which protects the class of data: the bridge token never
//! leaves, sizes are bounded, and only JSON-safe values come out.

use std::fmt;

use serde_json::{Map, Value};

use crate::config::Limits;
use crate::security::{SecurityError, Token, bounded};

/// What replaces every occurrence of the token.
pub const REDACTION: &str = "[redacted]";
const MAX_DEPTH: usize = 64;
const TOO_DEEP: &str = "[nested too deep]";

/// A payload that cannot be turned into a safe answer.
#[derive(Debug)]
pub enum OutputError {
    /// The payload of a tool answer is always a JSON object.
    NotAnObject,
    /// A size bound could not be applied.
    Bound(SecurityError),
    Serialize(serde_json::Error),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputError::NotAnObject => f.write_str("the payload must be a JSON object"),
            OutputError::Bound(error) => write!(f, "cannot bound the output: {error}"),
            OutputError::Serialize(error) => write!(f, "cannot serialise the output: {error}"),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OutputError::Bound(error) => Some(error),
            OutputError::Serialize(error) => Some(error),
            OutputError::NotAnObject => None,
        }
    }
}

/// Replace every occurrence of `needle` in `text`, ignoring ASCII case.
fn redact(text: &str, needle: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let haystack = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut kept_from = 0_usize;
    for (start, _) in haystack.match_indices(&needle) {
        out.push_str(text.get(kept_from..start).unwrap_or(""));
        out.push_str(REDACTION);
        kept_from = start + needle.len();
    }
    out.push_str(text.get(kept_from..).unwrap_or(""));
    out
}

struct Scrubber<'t> {
    token: &'t str,
    limit: usize,
    truncated: bool,
}

impl Scrubber<'_> {
    fn text(&mut self, value: &str) -> Result<String, OutputError> {
        // The token is removed before truncation: cutting first could leave a
        // prefix of the token at the end of the kept text.
        let cleaned = redact(value, self.token);
        let (cut, was_cut) = bounded(&cleaned, self.limit).map_err(OutputError::Bound)?;
        self.truncated = self.truncated || was_cut;
        Ok(redact(&cut, self.token))
    }

    fn walk(&mut self, value: &Value, depth: usize) -> Result<Value, OutputError> {
        if depth > MAX_DEPTH {
            return Ok(Value::String(TOO_DEEP.to_owned()));
        }
        Ok(match value {
            Value::String(text) => Value::String(self.text(text)?),
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.walk(item, depth + 1)?);
                }
                Value::Array(out)
            }
            Value::Object(entries) => {
                let mut out = Map::new();
                for (key, item) in entries {
                    out.insert(self.text(key)?, self.walk(item, depth + 1)?);
                }
                Value::Object(out)
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        })
    }
}

/// Return a JSON-safe copy of `payload`: token removed, strings bounded.
///
/// Each string is bounded to `max_output_bytes`. When anything was cut, the
/// `truncated` key is set to true. Structures nested deeper than a fixed depth are
/// replaced by a marker, so a hostile payload cannot exhaust the stack.
///
/// # Errors
/// The payload is not an object, or a bound cannot be applied.
pub fn finalize(payload: &Value, token: &Token, limits: &Limits) -> Result<Value, OutputError> {
    if !payload.is_object() {
        return Err(OutputError::NotAnObject);
    }
    let mut scrubber = Scrubber {
        token: token.as_str(),
        limit: limits.max_output_bytes,
        truncated: false,
    };
    let mut result = scrubber.walk(payload, 0)?;
    if scrubber.truncated
        && let Value::Object(entries) = &mut result
    {
        entries.insert("truncated".to_owned(), Value::Bool(true));
    }
    Ok(result)
}

/// Serialise a finalized payload as compact JSON.
///
/// # Errors
/// [`OutputError::Serialize`] if the value cannot be written.
pub fn to_text(payload: &Value) -> Result<String, OutputError> {
    serde_json::to_string(payload).map_err(OutputError::Serialize)
}
