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
                "font-weight" => {
                    patch.weight = weight(value);
                    patch.weight.is_some()
                }
                "font-style" => {
                    patch.slant = slant(value);
                    patch.slant.is_some()
                }
                "text-decoration" | "text-decoration-line" => {
                    if let Some(d) = decoration(value) {
                        patch.decoration = d;
                        true
                    } else {
                        false
                    }
                }
                "color" => {
                    patch.color = color(value);
                    patch.color.is_some()
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
            if patch.weight.is_some() {
                style.weight = patch.weight;
            }
            if patch.slant.is_some() {
                style.slant = patch.slant;
            }
            if patch.color.is_some() {
                style.color = patch.color;
            }
            style.decoration.overlay(patch.decoration);
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

pub(crate) fn weight(value: &str) -> Option<u16> {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        _ => value.parse().ok().filter(|w| (1..=1000).contains(w)),
    }
}
pub(crate) fn slant(value: &str) -> Option<reprise_doc::TextSlant> {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "normal" => Some(reprise_doc::TextSlant::Normal),
        "italic" => Some(reprise_doc::TextSlant::Italic),
        "oblique" => Some(reprise_doc::TextSlant::Oblique),
        _ => None,
    }
}
pub(crate) fn decoration(value: &str) -> Option<reprise_doc::Decoration> {
    let value = value.to_ascii_lowercase();
    let mut out = reprise_doc::Decoration {
        underline: Some(false),
        strike: Some(false),
    };
    let tokens: Vec<_> = value.split_ascii_whitespace().collect();
    if tokens.is_empty() || tokens.len() > 2 {
        return None;
    }
    if tokens == ["none"] {
        return Some(out);
    }
    for token in tokens {
        match token {
            "underline" => out.underline = Some(true),
            "line-through" => out.strike = Some(true),
            _ => return None,
        }
    }
    Some(out)
}
fn alpha(value: &str) -> Option<u8> {
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    if fractional.len() > 6 || !fractional.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let denom = 10u32.checked_pow(fractional.len() as u32)?;
    let whole: u32 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac: u32 = if fractional.is_empty() {
        0
    } else {
        fractional.parse().ok()?
    };
    let n = whole.checked_mul(denom)?.checked_add(frac)?;
    if n > denom {
        return None;
    }
    u8::try_from(n.checked_mul(255)?.checked_add(denom / 2)? / denom).ok()
}
pub(crate) fn color(value: &str) -> Option<[u8; 4]> {
    if value.len() > 128 {
        return None;
    }
    let value = value.to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let mut out = [0, 0, 0, 255];
        match hex.len() {
            3 | 4 => {
                for (slot, c) in out.iter_mut().zip(hex.bytes()) {
                    *slot = (c as char).to_digit(16)? as u8 * 17;
                }
            }
            6 | 8 => {
                for (slot, pair) in out.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
                    *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
                }
            }
            _ => return None,
        }
        return Some(out);
    }
    match value.as_str() {
        "transparent" => return Some([0, 0, 0, 0]),
        "black" => return Some([0, 0, 0, 255]),
        "white" => return Some([255, 255, 255, 255]),
        "red" => return Some([255, 0, 0, 255]),
        "green" => return Some([0, 128, 0, 255]),
        "blue" => return Some([0, 0, 255, 255]),
        _ => {}
    }
    let (body, rgba) = if let Some(b) = value.strip_prefix("rgba(") {
        (b, true)
    } else {
        (value.strip_prefix("rgb(")?, false)
    };
    let parts: Vec<_> = body.strip_suffix(')')?.split(',').map(str::trim).collect();
    if parts.len() != if rgba { 4 } else { 3 } {
        return None;
    }
    let mut out = [0, 0, 0, 255];
    for (slot, text) in out.iter_mut().take(3).zip(&parts) {
        *slot = text.parse().ok()?;
    }
    if rgba {
        out[3] = alpha(parts.get(3)?)?;
    }
    Some(out)
}
pub(crate) fn emphasis_css(
    weight: Option<u16>,
    slant: Option<reprise_doc::TextSlant>,
    decoration: reprise_doc::Decoration,
    color: Option<[u8; 4]>,
) -> String {
    let mut out = String::new();
    if let Some(w) = weight {
        out.push_str(&format!("font-weight: {w};"));
    }
    if let Some(s) = slant {
        out.push_str(&format!("font-style: {};", s.keyword()));
    }
    if !decoration.is_empty() {
        let mut lines = Vec::new();
        if decoration.underline == Some(true) {
            lines.push("underline");
        }
        if decoration.strike == Some(true) {
            lines.push("line-through");
        }
        out.push_str(&format!(
            "text-decoration: {};",
            if lines.is_empty() {
                "none".into()
            } else {
                lines.join(" ")
            }
        ));
    }
    if let Some([r, g, b, a]) = color {
        out.push_str(&format!("color: #{r:02x}{g:02x}{b:02x}{a:02x};"));
    }
    out
}
