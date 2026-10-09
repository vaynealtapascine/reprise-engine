//! A conservative clipboard reader, not a browser. It never fetches or executes anything.
use std::collections::BTreeMap;

use reprise_diag::Note;
use reprise_doc::{BlockKind, Document, LengthExpr, NodeId, SchemaRegistry, Style, TableColumns};
use reprise_geom::Length;

use crate::{ClipboardError, NativeFragment, codes};

#[derive(Clone, Copy, Debug)]
pub struct ImportLimits {
    pub bytes: usize,
    pub tokens: usize,
    pub depth: usize,
    pub blocks: usize,
}
impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            bytes: 8 << 20,
            tokens: 100_000,
            depth: 64,
            blocks: 4096,
        }
    }
}
pub struct Import {
    pub fragment: NativeFragment,
    pub notes: Vec<Note>,
}

fn fragment(doc: Document, notes: Vec<Note>) -> Result<Import, ClipboardError> {
    let fragment = NativeFragment {
        fragment: doc.copy_all_fragment("clipboard-import", &SchemaRegistry::builtin())?,
        resources: BTreeMap::new(),
        notes: notes.clone(),
    };
    Ok(Import { fragment, notes })
}
fn store(error: reprise_doc::DocError) -> ClipboardError {
    ClipboardError::Invalid(error.to_string())
}

/// CRLF/CR become LF. Two LFs split paragraphs; individual LFs stay authored line breaks.
pub fn import_plain(text: &str) -> Result<Import, ClipboardError> {
    if text.len() > ImportLimits::default().bytes {
        return Err(ClipboardError::Limit("plain text bytes"));
    }
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let doc = Document::new(1).map_err(store)?;
    for (i, part) in text.split("\n\n").enumerate() {
        if i >= ImportLimits::default().blocks {
            return Err(ClipboardError::Limit("plain paragraphs"));
        }
        doc.append_block(BlockKind::Paragraph, "", part)
            .map_err(store)?;
    }
    fragment(doc, Vec::new())
}

fn entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(rest.get(..pos).unwrap_or_default());
        rest = rest.get(pos..).unwrap_or_default();
        let end = rest.bytes().take(18).position(|b| b == b';');
        let decoded = end
            .and_then(|end| rest.get(1..end))
            .and_then(|entity| match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|n| u32::from_str_radix(n, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                    .and_then(char::from_u32),
            });
        if let (Some(end), Some(c)) = (end, decoded) {
            out.push(c);
            rest = rest.get(end.saturating_add(1)..).unwrap_or_default();
        } else {
            out.push('&');
            rest = rest.get(1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out
}

// ASCII HTML names and quotes, with no slices at unverified Unicode boundaries.
fn attributes(raw: &str) -> Result<BTreeMap<String, String>, ClipboardError> {
    let mut out = BTreeMap::new();
    let mut rest = raw;
    for _ in 0..128 {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        if end == 0 {
            rest = rest.get(1..).unwrap_or_default();
            continue;
        }
        let name = rest.get(..end).unwrap_or_default().to_ascii_lowercase();
        rest = rest.get(end..).unwrap_or_default().trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        rest = rest.get(1..).unwrap_or_default().trim_start();
        let (value, consumed) = if let Some(quote @ ('"' | '\'')) = rest.chars().next() {
            let tail = rest.get(1..).unwrap_or_default();
            let end = tail.find(quote).unwrap_or(tail.len());
            (
                tail.get(..end).unwrap_or_default(),
                1usize
                    .saturating_add(end)
                    .saturating_add(usize::from(end < tail.len())),
            )
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (rest.get(..end).unwrap_or_default(), end)
        };
        out.entry(name).or_insert_with(|| entities(value));
        rest = rest.get(consumed..).unwrap_or_default();
    }
    if !rest.trim().is_empty() && rest.trim() != "/" {
        return Err(ClipboardError::Limit("HTML attributes"));
    }
    Ok(out)
}

pub(crate) fn css_length(text: &str) -> Option<Length> {
    let text = text.trim().strip_suffix("pt")?.trim();
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.len() > 10
        || fraction.len() > 10
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (whole.is_empty() && fraction.is_empty())
    {
        return None;
    }
    let denominator = 10i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<i128>().ok()?
    };
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i128>().ok()?
    };
    let units = whole
        .checked_mul(denominator)?
        .checked_add(fraction)?
        .checked_mul(1024)?;
    let units = units.checked_add(denominator / 2)? / denominator;
    let signed = if negative {
        units.checked_neg()?
    } else {
        units
    };
    Some(Length(i32::try_from(signed).ok()?))
}
fn css(attrs: &BTreeMap<String, String>, notes: &mut Vec<Note>) -> Result<Style, ClipboardError> {
    let mut style = Style::default();
    if let Some(raw) = attrs.get("style") {
        for (index, declaration) in raw.split(';').enumerate() {
            if index >= 128 {
                return Err(ClipboardError::Limit("CSS declarations"));
            }
            let Some((key, value)) = declaration.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "text-align" => {
                    style.alignment = match value {
                        "start" | "left" => Some(reprise_doc::marks::Alignment::Start),
                        "center" => Some(reprise_doc::marks::Alignment::Centre),
                        "end" | "right" => Some(reprise_doc::marks::Alignment::End),
                        _ => None,
                    };
                    if style.alignment.is_none() {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported paragraph alignment omitted",
                            false,
                        );
                    }
                }
                "font-family" => {
                    let value = value.trim_matches(['\'', '"']);
                    if !value.contains(['\\', ',']) {
                        style.family = Some(value.into());
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "complex CSS font lists/escapes omitted",
                            false,
                        );
                    }
                }
                "font-size" | "line-height" => {
                    if let Some(length) = css_length(value) {
                        if key == "font-size" {
                            style.size = Some(LengthExpr::Pt(length));
                        } else {
                            style.line_height = Some(LengthExpr::Pt(length));
                        }
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported CSS length omitted",
                            false,
                        );
                    }
                }
                "font-weight" => {
                    if let Some(v) = crate::text_css::weight(value) {
                        style.weight = Some(v);
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported weight omitted",
                            false,
                        );
                    }
                }
                "font-style" => {
                    if let Some(v) = crate::text_css::slant(value) {
                        style.slant = Some(v);
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported slant omitted",
                            false,
                        );
                    }
                }
                "text-decoration" | "text-decoration-line" => {
                    if let Some(v) = crate::text_css::decoration(value) {
                        style.decoration = v;
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported decoration omitted",
                            false,
                        );
                    }
                }
                "color" => {
                    if let Some(v) = crate::text_css::color(value) {
                        style.color = Some(v);
                    } else {
                        once(
                            notes,
                            codes::HTML_APPROXIMATED,
                            "unsupported colour omitted",
                            false,
                        );
                    }
                }
                "white-space" if matches!(value, "pre-wrap" | "normal") => {}
                _ => once(
                    notes,
                    codes::HTML_APPROXIMATED,
                    "unsupported CSS declarations omitted",
                    false,
                ),
            }
        }
    }
    Ok(style)
}
fn once(notes: &mut Vec<Note>, code: reprise_diag::Code, message: &str, omitted: bool) {
    if !notes.iter().any(|n| n.code == code) {
        notes.push(if omitted {
            Note::error(code, message)
        } else {
            Note::warning(code, message)
        });
    }
}

struct Reader {
    doc: Document,
    notes: Vec<Note>,
    text: String,
    style: Style,
    inline: Vec<(usize, reprise_doc::formatting::TextStyle)>,
    formats: Vec<reprise_doc::formatting::FormatRun>,
    format_count: usize,
    preserve_space: bool,
    direction: Option<u8>,
    cell: Option<NodeId>,
    table: Option<NodeId>,
    row: Option<NodeId>,
    column: u32,
    max_columns: u32,
    count: usize,
    limits: ImportLimits,
    /// Rows started in this table; the open row is `rows_started - 1`.
    rows_started: u32,
    /// Grid positions claimed by row spans of earlier rows (row, column).
    occupied: std::collections::BTreeSet<(u32, u32)>,
    /// Header (`th`) and data (`td`) cells seen in the open row, and whether
    /// the reader is inside `thead`.
    row_th: bool,
    row_td: bool,
    in_head: bool,
}

/// Largest spans an import accepts: the table-wide column limit, and a
/// bounded number of claimed grid positions so spans cannot multiply input.
const MAX_HTML_COLSPAN: u32 = 128;
const MAX_HTML_ROWSPAN: u32 = 4096;
const MAX_HTML_SPAN_CELLS: usize = 65536;

fn span_attribute(
    attrs: &BTreeMap<String, String>,
    name: &str,
    max: u32,
    notes: &mut Vec<Note>,
) -> u32 {
    let Some(raw) = attrs.get(name) else {
        return 1;
    };
    match raw.trim().parse::<u32>() {
        Ok(n) if (1..=max).contains(&n) => n,
        Ok(n) if n > max => {
            once(
                notes,
                codes::HTML_APPROXIMATED,
                "an oversized table span was clamped",
                false,
            );
            max
        }
        _ => {
            once(
                notes,
                codes::HTML_APPROXIMATED,
                "a zero or malformed table span was read as one",
                false,
            );
            1
        }
    }
}

impl Reader {
    /// Marks the open row as a header row when it holds only `th` cells or sits
    /// in `thead`. A row mixing `th` and `td` stays a body row, approximated.
    fn finish_row(&mut self) -> Result<(), ClipboardError> {
        if let Some(row) = self.row.take() {
            if self.in_head || (self.row_th && !self.row_td) {
                self.doc.set_table_row_header(row, true).map_err(store)?;
            } else if self.row_th && self.row_td {
                once(
                    &mut self.notes,
                    codes::HTML_APPROXIMATED,
                    "a row mixing th and td cells became a body row",
                    false,
                );
            }
        }
        self.row_th = false;
        self.row_td = false;
        Ok(())
    }
    fn finish_table(&mut self) -> Result<(), ClipboardError> {
        if let Some(table) = self.table {
            let count = usize::try_from(self.max_columns.max(1))
                .map_err(|_| ClipboardError::Limit("HTML columns"))?;
            self.doc
                .set_fragment_table_columns(
                    table,
                    TableColumns {
                        columns: vec![
                            reprise_doc::Column {
                                width: reprise_doc::ColumnWidth::Proportional(1)
                            };
                            count
                        ],
                    },
                )
                .map_err(store)?;
        }
        Ok(())
    }
    fn count(&mut self) -> Result<(), ClipboardError> {
        self.count = self.count.saturating_add(1);
        if self.count > self.limits.blocks.min(4096) {
            return Err(ClipboardError::Limit("HTML blocks"));
        }
        Ok(())
    }
    fn flush(&mut self, force: bool) -> Result<(), ClipboardError> {
        if self.text.is_empty() && !force {
            return Ok(());
        }
        self.count()?;
        if let Some(direction) = self.direction {
            let inferred = crate::export::inferred_base(&self.text);
            if direction != inferred {
                once(
                    &mut self.notes,
                    codes::HTML_APPROXIMATED,
                    "explicit HTML direction differs from Unicode inference; the authored model has no paragraph direction property",
                    false,
                );
            }
        }
        let node = if let Some(cell) = self.cell {
            self.doc
                .append_cell_block(cell, BlockKind::Paragraph, "", &self.text)
        } else {
            self.doc.append_block(BlockKind::Paragraph, "", &self.text)
        }
        .map_err(store)?;
        self.doc.set_overrides(node, &self.style).map_err(store)?;
        for run in self.formats.drain(..) {
            if run.style == reprise_doc::formatting::TextStyle::default() {
                continue;
            }
            self.format_count += 1;
            if self.format_count > reprise_doc::formatting::MAX_FORMATS_PER_HOST {
                return Err(ClipboardError::Limit("HTML formatting"));
            }
            self.doc
                .format_text(
                    node,
                    run.bytes,
                    &run.style,
                    reprise_doc::text::RangePolicy::EXPANDING,
                )
                .map_err(store)?;
        }
        self.text.clear();
        Ok(())
    }
    fn text(&mut self, raw: &str) -> Result<(), ClipboardError> {
        let start = self.text.len();
        // HTML whitespace collapses. A single pending space survives token seams.
        for c in entities(raw).chars() {
            if c.is_ascii_whitespace() && !self.preserve_space {
                if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
                    self.text.push(' ');
                }
            } else {
                self.text.push(c);
            }
        }
        if self.text.len() > self.limits.bytes.min(8 << 20) {
            return Err(ClipboardError::Limit("HTML text"));
        }
        self.record_style(start)?;
        Ok(())
    }
    fn record_style(&mut self, start: usize) -> Result<(), ClipboardError> {
        if start == self.text.len() {
            return Ok(());
        }
        let style = self
            .inline
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        if let Some(last) = self.formats.last_mut()
            && last.bytes.end == start
            && last.style == style
        {
            last.bytes.end = self.text.len();
            return Ok(());
        }
        if self.formats.len() >= reprise_doc::formatting::MAX_FORMATS_PER_HOST {
            return Err(ClipboardError::Limit("HTML formatting"));
        }
        self.formats.push(reprise_doc::formatting::FormatRun {
            bytes: start..self.text.len(),
            style,
        });
        Ok(())
    }
    fn tag(
        &mut self,
        name: &str,
        closing: bool,
        attrs: &BTreeMap<String, String>,
    ) -> Result<(), ClipboardError> {
        let cell = matches!(name, "td" | "th");
        let known = |k: &String| {
            k == "dir"
                || k == "style"
                || (name == "span" && k == "lang")
                || (cell && (k == "colspan" || k == "rowspan"))
        };
        if !attrs.keys().all(known) {
            once(
                &mut self.notes,
                codes::HTML_APPROXIMATED,
                "HTML attributes omitted",
                false,
            );
        }
        match (name, closing) {
            ("br", false) => {
                let start = self.text.len();
                self.text.push('\n');
                self.record_style(start)?;
            }
            ("span" | "b" | "strong" | "i" | "em" | "u" | "s", _) => {}
            ("p" | "div" | "li" | "h1" | "h2" | "h3" | "blockquote", false) => {
                self.flush(false)?;
                self.style = css(attrs, &mut self.notes)?;
                self.preserve_space = attrs.get("style").is_some_and(|s| {
                    s.split(';').any(|d| {
                        d.split_once(':').is_some_and(|(k, v)| {
                            k.trim().eq_ignore_ascii_case("white-space") && v.trim() == "pre-wrap"
                        })
                    })
                });
                self.direction = match attrs.get("dir").map(String::as_str) {
                    Some("rtl") => Some(1),
                    Some("ltr") => Some(0),
                    _ => None,
                };
            }
            ("p", true) => {
                self.flush(true)?;
                self.style = Style::default();
                self.preserve_space = false;
                self.direction = None;
            }
            ("div" | "li" | "h1" | "h2" | "h3" | "blockquote", true) => {
                self.flush(false)?;
                self.style = Style::default();
                self.preserve_space = false;
                self.direction = None;
            }
            ("table", false) => {
                self.flush(false)?;
                if self.table.is_some() {
                    return Err(ClipboardError::Invalid(
                        "nested tables are unsupported".into(),
                    ));
                }
                self.count()?;
                self.max_columns = 0;
                self.rows_started = 0;
                self.occupied.clear();
                self.table = Some(
                    self.doc
                        .append_table(TableColumns {
                            columns: Vec::new(),
                        })
                        .map_err(store)?,
                );
            }
            ("tr", false) => {
                self.flush(false)?;
                self.cell = None;
                self.finish_row()?;
                if let Some(table) = self.table {
                    self.count()?;
                    self.row = Some(self.doc.append_table_row(table, false).map_err(store)?);
                    self.rows_started = self.rows_started.saturating_add(1);
                    self.column = 0;
                } else {
                    once(
                        &mut self.notes,
                        codes::HTML_APPROXIMATED,
                        "unbalanced table tags flattened",
                        false,
                    );
                }
            }
            ("td" | "th", false) => {
                self.flush(false)?;
                if let Some(row) = self.row {
                    self.count()?;
                    let r = self.rows_started.saturating_sub(1);
                    // Skip positions claimed by an earlier row's span.
                    while self.occupied.contains(&(r, self.column)) {
                        self.column = self.column.saturating_add(1);
                        if self.column > MAX_HTML_COLSPAN {
                            return Err(ClipboardError::Limit("HTML table columns"));
                        }
                    }
                    let colspan =
                        span_attribute(attrs, "colspan", MAX_HTML_COLSPAN, &mut self.notes);
                    let mut rowspan =
                        span_attribute(attrs, "rowspan", MAX_HTML_ROWSPAN, &mut self.notes);
                    let area = (colspan as usize).saturating_mul(rowspan as usize);
                    if rowspan > 1 && self.occupied.len().saturating_add(area) > MAX_HTML_SPAN_CELLS
                    {
                        once(
                            &mut self.notes,
                            codes::HTML_APPROXIMATED,
                            "row spans beyond the import bound were read as one row",
                            false,
                        );
                        rowspan = 1;
                    }
                    for dr in 1..rowspan {
                        for dc in 0..colspan {
                            self.occupied
                                .insert((r.saturating_add(dr), self.column.saturating_add(dc)));
                        }
                    }
                    self.cell = Some(
                        self.doc
                            .append_table_cell_spanned(row, self.column, colspan, rowspan)
                            .map_err(store)?,
                    );
                    if name == "th" {
                        self.row_th = true;
                    } else {
                        self.row_td = true;
                    }
                    self.column = self.column.saturating_add(colspan);
                    self.max_columns = self.max_columns.max(self.column);
                    if self.max_columns > MAX_HTML_COLSPAN {
                        return Err(ClipboardError::Limit("HTML table columns"));
                    }
                } else {
                    once(
                        &mut self.notes,
                        codes::HTML_APPROXIMATED,
                        "unbalanced table tags flattened",
                        false,
                    );
                }
            }
            ("td" | "th", true) => {
                self.flush(false)?;
                self.cell = None;
            }
            ("tr", true) => {
                self.flush(false)?;
                self.cell = None;
                self.finish_row()?;
            }
            ("thead", closing) => {
                self.flush(false)?;
                self.in_head = !closing;
            }
            ("table", true) => {
                self.flush(false)?;
                self.finish_row()?;
                self.in_head = false;
                self.finish_table()?;
                self.cell = None;
                self.row = None;
                self.table = None;
            }
            ("html" | "body" | "tbody" | "tfoot", _) => {}
            _ => once(
                &mut self.notes,
                codes::HTML_APPROXIMATED,
                "unsupported HTML tags flattened to text",
                false,
            ),
        }
        Ok(())
    }
}

/// Supported: paragraphs, line breaks, basic point CSS and nonnested tables.
/// Unknown markup retains text; script/style/head/template/iframe/object content is omitted.
/// Limits reject the entire import with an Error diagnostic, never a partial silent parse.
pub fn import_html(html: &str, limits: ImportLimits) -> Result<Import, ClipboardError> {
    if html.len() > limits.bytes.min(8 << 20) {
        return Err(ClipboardError::Limit("HTML input bytes"));
    }
    let mut reader = Reader {
        doc: Document::new(1).map_err(store)?,
        notes: Vec::new(),
        text: String::new(),
        style: Style::default(),
        inline: Vec::new(),
        formats: Vec::new(),
        format_count: 0,
        preserve_space: false,
        direction: None,
        cell: None,
        table: None,
        row: None,
        column: 0,
        max_columns: 0,
        count: 0,
        limits,
        rows_started: 0,
        occupied: Default::default(),
        row_th: false,
        row_td: false,
        in_head: false,
    };
    let mut rest = html;
    let mut stack = Vec::<String>::new();
    let mut skipped: Option<String> = None;
    let mut tokens = 0usize;
    while !rest.is_empty() {
        tokens = tokens.saturating_add(1);
        if tokens > limits.tokens.min(100_000) {
            return Err(ClipboardError::Limit("HTML tokens"));
        }
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            if skipped.is_none() {
                reader.text(rest.get(..end).unwrap_or_default())?;
            }
            rest = rest.get(end..).unwrap_or_default();
            continue;
        }
        if rest.starts_with("<!--") {
            if let Some(end) = rest.find("-->") {
                rest = rest.get(end.saturating_add(3)..).unwrap_or_default();
                continue;
            }
            once(
                &mut reader.notes,
                codes::HTML_DROPPED,
                "unclosed comment omitted",
                true,
            );
            break;
        }
        let mut quote = None;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else if c == '\'' || c == '"' {
                quote = Some(c);
            } else if c == '>' {
                end = Some(i);
                break;
            }
        }
        let Some(end) = end else {
            if skipped.is_none() {
                reader.text(rest)?;
            }
            once(
                &mut reader.notes,
                codes::HTML_APPROXIMATED,
                "unclosed tag retained as text",
                false,
            );
            break;
        };
        let raw = rest.get(1..end).unwrap_or_default().trim();
        rest = rest.get(end.saturating_add(1)..).unwrap_or_default();
        if raw.starts_with('!') || raw.starts_with('?') {
            continue;
        }
        let closing = raw.starts_with('/');
        let raw = raw.strip_prefix('/').unwrap_or(raw).trim_start();
        let name_end = raw
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(raw.len());
        let name = raw.get(..name_end).unwrap_or_default().to_ascii_lowercase();
        if let Some(tag) = &skipped {
            if closing && *tag == name {
                skipped = None;
            }
            continue;
        }
        if !closing
            && matches!(
                name.as_str(),
                "script" | "style" | "head" | "template" | "iframe" | "object"
            )
        {
            skipped = Some(name);
            once(
                &mut reader.notes,
                codes::HTML_DROPPED,
                "active or non-content HTML omitted",
                true,
            );
            continue;
        }
        let void = matches!(
            name.as_str(),
            "br" | "img" | "hr" | "meta" | "link" | "input" | "wbr"
        ) || raw.ends_with('/');
        if closing {
            if let Some(at) = stack.iter().rposition(|n| n == &name) {
                if at.saturating_add(1) != stack.len() {
                    once(
                        &mut reader.notes,
                        codes::HTML_APPROXIMATED,
                        "unbalanced tags repaired",
                        false,
                    );
                }
                stack.truncate(at);
            } else {
                once(
                    &mut reader.notes,
                    codes::HTML_APPROXIMATED,
                    "unmatched closing tag ignored",
                    false,
                );
                continue;
            }
        } else if !void {
            if stack.len() >= limits.depth.min(64) {
                return Err(ClipboardError::Limit("HTML nesting"));
            }
            stack.push(name.clone());
        }
        reader.inline.retain(|(depth, _)| *depth <= stack.len());
        let attrs = attributes(raw.get(name_end..).unwrap_or_default())?;
        if matches!(
            name.as_str(),
            "span" | "b" | "strong" | "i" | "em" | "u" | "s"
        ) && !closing
            && !void
        {
            let parent = reader
                .inline
                .last()
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            let mut parent = parent;
            match name.as_str() {
                "b" | "strong" => parent.weight = Some(700),
                "i" | "em" => parent.slant = Some(reprise_doc::TextSlant::Italic),
                "u" => parent.decoration.underline = Some(true),
                "s" => parent.decoration.strike = Some(true),
                _ => {}
            }
            let style = crate::text_css::inline(&attrs, &parent, &mut reader.notes)?;
            reader.inline.push((stack.len(), style));
        }
        reader.tag(&name, closing, &attrs)?;
    }
    if !stack.is_empty() {
        once(
            &mut reader.notes,
            codes::HTML_APPROXIMATED,
            "unclosed HTML tags repaired at end of input",
            false,
        );
    }
    if skipped.is_some() {
        once(
            &mut reader.notes,
            codes::HTML_DROPPED,
            "unclosed active HTML content omitted",
            true,
        );
    }
    reader.flush(false)?;
    reader.finish_table()?;
    fragment(reader.doc, reader.notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_css_and_unicode_entities_are_total() {
        for length in [
            Length::MIN,
            Length::ZERO,
            Length::MAX,
            Length(-1),
            Length(1),
        ] {
            assert_eq!(
                css_length(&format!("{}pt", crate::export::points(length))),
                Some(length)
            );
        }
        assert_eq!(
            entities("&#x5d0; &amp; &#x110000; &oops; café"),
            "א & &#x110000; &oops; café"
        );
        assert!(css_length("9999999999.9999999999pt").is_none());
    }
}
