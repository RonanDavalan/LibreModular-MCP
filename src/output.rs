//! Point de sortie unique de tout ce que le Hub rend, écrit ou journalise.
//!
//! Pourquoi ce module existe
//! Protéger un chemin de sortie à la fois laisse les autres ouverts. Chaque charge utile
//! passe par `finalize`, qui protège la classe de données : le jeton ne sort jamais,
//! les tailles sont bornées, et seules des valeurs sûres en JSON sortent.

use std::fmt;

use serde_json::{Map, Value};

use crate::config::Limits;
use crate::security::{SecurityError, Token, bounded};

/// Ce qui remplace chaque occurrence du jeton.
pub const REDACTION: &str = "[redacted]";
const MAX_DEPTH: usize = 64;
const TOO_DEEP: &str = "[nested too deep]";

/// Une charge utile qui ne peut pas devenir une réponse sûre.
#[derive(Debug)]
pub enum OutputError {
    /// La charge utile d'une réponse d'outil est toujours un objet JSON.
    NotAnObject,
    /// Une borne de taille n'a pas pu être appliquée.
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

/// Remplace chaque occurrence de `needle` dans `text`, sans tenir compte de la casse ASCII.
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
        // Le jeton est retiré avant la troncature : couper d'abord pourrait laisser un
        // préfixe du jeton à la fin du texte conservé.
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

/// Rend une copie de `payload` sûre en JSON : jeton retiré, chaînes bornées.
///
/// Chaque chaîne est bornée à `max_output_bytes`. Quand quelque chose a été coupé, la
/// clé `truncated` vaut vrai. Les structures imbriquées plus profondément qu'une
/// profondeur fixe sont remplacées par un marqueur, pour qu'une charge hostile ne
/// puisse pas épuiser la pile.
///
/// # Erreurs
/// La charge utile n'est pas un objet, ou une borne ne peut pas être appliquée.
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

/// Sérialise une charge utile finalisée en JSON compact.
///
/// # Erreurs
/// [`OutputError::Serialize`] si la valeur ne peut pas être écrite.
pub fn to_text(payload: &Value) -> Result<String, OutputError> {
    serde_json::to_string(payload).map_err(OutputError::Serialize)
}
