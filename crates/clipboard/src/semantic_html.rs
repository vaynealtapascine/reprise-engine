//! Roles and lists in HTML (35; see `docs/roles-and-lists.md`).
//!
//! Export: a role picks the paragraph's element (`h1`–`h6`, `blockquote`,
//! `pre`, or `p` with a class), and consecutive list items of one list become
//! nested `ol`/`ul` with `li`, `start`, `value` and `list-style-type`. Import
//! reads the same back. Adding a role or numbering format: extend
//! [`element`], [`role_of`], [`list_style`] and [`format_of`].

use reprise_doc::{Document, ListId, NodeId, NumberFormat, Role};

use crate::ClipboardError;

/// The element a role exports as, and its class.
pub(crate) fn element(role: Role) -> (&'static str, Option<&'static str>) {
    match role {
        Role::Body => ("p", None),
        Role::Heading(1) => ("h1", None),
        Role::Heading(2) => ("h2", None),
        Role::Heading(3) => ("h3", None),
        Role::Heading(4) => ("h4", None),
        Role::Heading(5) => ("h5", None),
        Role::Heading(_) => ("h6", None),
        Role::Title => ("h1", Some("reprise-title")),
        Role::Subtitle => ("p", Some("reprise-subtitle")),
        Role::Quote => ("blockquote", None),
        Role::Code => ("pre", None),
        Role::Verse => ("p", Some("reprise-verse")),
    }
}

/// The role an element with an optional class imports as, if it has one.
pub(crate) fn role_of(name: &str, class: Option<&str>) -> Option<Role> {
    let classes = |c: &str| class.is_some_and(|all| all.split_ascii_whitespace().any(|x| x == c));
    if classes("reprise-title") {
        return Some(Role::Title);
    }
    if classes("reprise-subtitle") {
        return Some(Role::Subtitle);
    }
    if classes("reprise-verse") {
        return Some(Role::Verse);
    }
    Some(match name {
        "h1" => Role::Heading(1),
        "h2" => Role::Heading(2),
        "h3" => Role::Heading(3),
        "h4" => Role::Heading(4),
        "h5" => Role::Heading(5),
        "h6" => Role::Heading(6),
        "blockquote" => Role::Quote,
        "pre" => Role::Code,
        _ => return None,
    })
}

/// A format's CSS `list-style-type` and `ol type`, for an item's marker.
pub(crate) fn list_style(format: &NumberFormat, marker: &str) -> (String, Option<&'static str>) {
    match format {
        NumberFormat::Bullet => (
            match marker {
                "•" => "disc".into(),
                "◦" => "circle".into(),
                "▪" | "■" => "square".into(),
                other => css_string(other),
            },
            None,
        ),
        NumberFormat::Decimal | NumberFormat::Unknown(_) => ("decimal".into(), Some("1")),
        NumberFormat::DecimalLeadingZero => ("decimal-leading-zero".into(), None),
        NumberFormat::LowerAlpha => ("lower-alpha".into(), Some("a")),
        NumberFormat::UpperAlpha => ("upper-alpha".into(), Some("A")),
        NumberFormat::LowerRoman => ("lower-roman".into(), Some("i")),
        NumberFormat::UpperRoman => ("upper-roman".into(), Some("I")),
        NumberFormat::None => ("none".into(), None),
    }
}

/// The format of an imported `ol`/`ul` from its `type` attribute or its CSS
/// `list-style-type`.
pub(crate) fn format_of(ordered: bool, kind: Option<&str>, css: Option<&str>) -> NumberFormat {
    let by_css = css.map(|c| match c.trim().to_ascii_lowercase().as_str() {
        "decimal" => NumberFormat::Decimal,
        "decimal-leading-zero" => NumberFormat::DecimalLeadingZero,
        "lower-alpha" | "lower-latin" => NumberFormat::LowerAlpha,
        "upper-alpha" | "upper-latin" => NumberFormat::UpperAlpha,
        "lower-roman" => NumberFormat::LowerRoman,
        "upper-roman" => NumberFormat::UpperRoman,
        "none" => NumberFormat::None,
        _ if ordered => NumberFormat::Decimal,
        _ => NumberFormat::Bullet,
    });
    let by_type = kind.map(|t| match t.trim() {
        "a" => NumberFormat::LowerAlpha,
        "A" => NumberFormat::UpperAlpha,
        "i" => NumberFormat::LowerRoman,
        "I" => NumberFormat::UpperRoman,
        _ => NumberFormat::Decimal,
    });
    by_css
        .or(if ordered { by_type } else { None })
        .unwrap_or(if ordered {
            NumberFormat::Decimal
        } else {
            NumberFormat::Bullet
        })
}

fn css_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        if c.is_control() {
            out.push_str(&format!("\\{:x} ", c as u32));
        } else {
            if matches!(c, '\\' | '"') {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push(' ');
    out.push('"');
    out
}

struct Open {
    list: ListId,
    level: u8,
    ordered: bool,
    /// The value the next item would have without a `value` attribute.
    next: i64,
}

/// The `ol`/`ul` elements open around a sequence of sibling blocks.
#[derive(Default)]
pub(crate) struct ListWriter {
    open: Vec<Open>,
}

impl ListWriter {
    /// Opens or closes lists before `node` is written, so that it lands in
    /// the right `li`. A block that is not an item closes every open list.
    pub(crate) fn before(
        &mut self,
        doc: &Document,
        node: NodeId,
        out: &mut String,
    ) -> Result<(), ClipboardError> {
        let Some(item) = doc.list_item(node) else {
            return self.close(out);
        };
        if self.open.first().is_some_and(|o| o.list != item.list) {
            self.close(out)?;
        }
        while self.open.last().is_some_and(|o| o.level > item.level) {
            self.pop(out)?;
        }
        let ordered = item.format.is_ordered();
        match self.open.last_mut() {
            Some(o) if o.level == item.level && o.ordered == ordered => {
                crate::export::append(out, "</li>")?;
                let value = if o.ordered && item.value != o.next {
                    format!(" value=\"{}\"", item.value)
                } else {
                    String::new()
                };
                o.next = item.value.saturating_add(1);
                crate::export::append(out, &format!("<li{value}>"))?;
            }
            Some(o) if o.level == item.level => {
                // Same level, other kind of marker: a new sibling list.
                self.pop(out)?;
                self.push(&item, out)?;
            }
            _ => self.push(&item, out)?,
        }
        Ok(())
    }

    fn push(&mut self, item: &reprise_doc::ListItem, out: &mut String) -> Result<(), ClipboardError> {
        let ordered = item.format.is_ordered();
        let (style, kind) = list_style(&item.format, &item.marker);
        let mut attrs = format!(
            " style=\"list-style-type: {}\"",
            crate::export::escape(&style)
        );
        if ordered {
            if let Some(kind) = kind {
                attrs.push_str(&format!(" type=\"{kind}\""));
            }
            if item.value != 1 {
                attrs.push_str(&format!(" start=\"{}\"", item.value));
            }
        }
        let tag = if ordered { "ol" } else { "ul" };
        crate::export::append(out, &format!("<{tag}{attrs}><li>"))?;
        self.open.push(Open {
            list: item.list,
            level: item.level,
            ordered,
            next: item.value.saturating_add(1),
        });
        Ok(())
    }

    fn pop(&mut self, out: &mut String) -> Result<(), ClipboardError> {
        if let Some(o) = self.open.pop() {
            crate::export::append(out, if o.ordered { "</li></ol>" } else { "</li></ul>" })?;
        }
        Ok(())
    }

    /// Closes every open list.
    pub(crate) fn close(&mut self, out: &mut String) -> Result<(), ClipboardError> {
        while !self.open.is_empty() {
            self.pop(out)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_round_trips_through_its_element() {
        for role in [
            Role::Body,
            Role::Heading(1),
            Role::Heading(2),
            Role::Heading(3),
            Role::Heading(4),
            Role::Heading(5),
            Role::Heading(6),
            Role::Title,
            Role::Subtitle,
            Role::Quote,
            Role::Code,
            Role::Verse,
        ] {
            let (name, class) = element(role);
            assert_eq!(role_of(name, class).unwrap_or(Role::Body), role, "{role}");
        }
    }

    #[test]
    fn every_format_round_trips_through_css() {
        for f in [
            NumberFormat::Bullet,
            NumberFormat::Decimal,
            NumberFormat::DecimalLeadingZero,
            NumberFormat::LowerAlpha,
            NumberFormat::UpperAlpha,
            NumberFormat::LowerRoman,
            NumberFormat::UpperRoman,
            NumberFormat::None,
        ] {
            let (css, kind) = list_style(&f, "•");
            assert_eq!(format_of(f.is_ordered(), kind, Some(&css)), f, "{f}");
        }
        assert_eq!(format_of(true, Some("I"), None), NumberFormat::UpperRoman);
        assert_eq!(format_of(false, Some("I"), None), NumberFormat::Bullet);
    }
}
