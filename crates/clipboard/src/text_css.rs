//! Bounded parsing of the inline CSS subset emitted by our HTML exporter.
use reprise_doc::formatting::{TextFeature, TextStyle};
use std::collections::BTreeMap;

/// Split only outside quoted strings; a delimiter in a font name is data.
pub(crate) fn split(value: &str, delimiter: char) -> Vec<&str> {
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    let mut out = Vec::new();
    for (i, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if c == delimiter {
            out.push(&value[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&value[start..]);
    out
}

fn string(value: &str) -> Option<(String, &str)> {
    let value = value.trim_start();
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let mut chars = value[1..].char_indices().peekable();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        if c == quote {
            return Some((out, &value[i + 2..]));
        }
        if c == '\\' {
            let (_, first) = chars.next()?;
            if first.is_ascii_hexdigit() {
                let mut hex = String::from(first);
                while hex.len() < 6 && chars.peek().is_some_and(|(_, c)| c.is_ascii_hexdigit()) {
                    hex.push(chars.next()?.1);
                }
                if chars.peek().is_some_and(|(_, c)| c.is_ascii_whitespace()) {
                    chars.next();
                }
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            } else if !first.is_control() {
                out.push(first);
            } else {
                return None;
            }
        } else if !c.is_control() {
            out.push(c);
        } else {
            return None;
        }
    }
    None
}

pub(crate) fn families(value: &str) -> Option<Vec<String>> {
    split(value, ',')
        .into_iter()
        .map(|part| {
            let part = part.trim();
            if part.starts_with(['\'', '"']) {
                let (name, rest) = string(part)?;
                rest.trim().is_empty().then_some(name)
            } else if !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            {
                Some(part.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn features(value: &str) -> Option<Vec<TextFeature>> {
    if value == "normal" {
        return Some(Vec::new());
    }
    split(value, ',')
        .into_iter()
        .map(|part| {
            let (tag, rest) = string(part)?;
            let tag: [u8; 4] = tag.as_bytes().try_into().ok()?;
            let value = match rest.trim() {
                "" | "on" => 1,
                "off" => 0,
                n => n.parse().ok()?,
            };
            Some(TextFeature { tag, value })
        })
        .collect()
}

pub(crate) fn inline(
    attrs: &BTreeMap<String, String>,
    parent: &TextStyle,
    notes: &mut Vec<reprise_diag::Note>,
) -> Result<TextStyle, crate::ClipboardError> {
    let mut style = parent.clone();
    let mut candidate = TextStyle {
        language: attrs.get("lang").cloned(),
        ..Default::default()
    };
    if candidate.validate().is_ok() {
        if candidate.language.is_some() {
            style.language = candidate.language.take();
        }
    } else {
        omitted(notes);
    }
    if let Some(raw) = attrs.get("style") {
        for (i, declaration) in split(raw, ';').into_iter().enumerate() {
            if i >= 128 {
                return Err(crate::ClipboardError::Limit("CSS declarations"));
            }
            let Some((key, value)) = declaration.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let mut patch = TextStyle::default();
            let valid = match key.trim().to_ascii_lowercase().as_str() {
                "font-family" => {
                    patch.families = families(value);
                    patch.families.is_some()
                }
                "font-size" => {
                    patch.size = super::html::css_length(value);
                    patch.size.is_some()
                }
                "font-feature-settings" => {
                    patch.features = features(value);
                    patch.features.is_some()
                }
                _ => false,
            };
            if !valid || patch.validate().is_err() {
                omitted(notes);
                continue;
            }
            if patch.families.is_some() {
                style.families = patch.families;
            }
            if patch.size.is_some() {
                style.size = patch.size;
            }
            if patch.features.is_some() {
                style.features = patch.features;
            }
        }
    }
    Ok(style)
}

fn omitted(notes: &mut Vec<reprise_diag::Note>) {
    if !notes
        .iter()
        .any(|n| n.code == crate::codes::HTML_APPROXIMATED)
    {
        notes.push(reprise_diag::Note::warning(
            crate::codes::HTML_APPROXIMATED,
            "unsupported inline CSS or language omitted",
        ));
    }
}
