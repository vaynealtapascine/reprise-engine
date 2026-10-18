//! Generators for the wave-1 command vocabulary, through the facade.
//!
//! A generator is a function of the scenario bytes and a snapshot of the
//! document's `State`, so every choice is a selector ("the block that has an
//! inline object, number `n` modulo how many there are") and stays meaningful
//! whatever earlier ops did. Most choices are valid; a fixed share is
//! deliberately wrong (a stale ID, an offset inside a code point, a level of
//! nine, a javascript: link) so refusals are exercised as hard as successes.
//!
//! Generators lean on the *interactions* the brief names: an atom inside a
//! link, a field in a list item, a link across a split, a style applied to a
//! list item, a format across an atom. [`Gen::recipe`] builds those as
//! batches whose later commands depend on the earlier ones.

use std::collections::BTreeMap;

use reprise::{
    Alignment, AnnotationPayload, Block, BlockRole, Command, Decoration, FontStyle, InlineAlign,
    InlineObject, InlinePatch, LinkDestination, ListDefinition, ListLevel, ListRef, State, Style,
    StyleProperty, TextFeature, TextStyle,
};

use crate::input::Input;

/// The state a generator reads.
pub struct Snap {
    pub state: State,
}

impl Snap {
    pub fn blocks(&self) -> &[Block] {
        &self.state.blocks
    }
}

/// What kind of block a selector prefers. Falls back to any block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want {
    Any,
    /// Has text.
    Text,
    /// A list item.
    Listed,
    /// Holds an inline object.
    Inline,
    /// Some annotation covers part of it.
    Annotated,
    /// A role other than body.
    Roled,
}

pub const TEXTS: [&str; 14] = [
    "",
    "alpha ",
    "office ffi \u{5d0}\u{5d1}\u{5d2} ",
    "e\u{301}\u{302} ",
    "\u{1F468}\u{200D}\u{1F469} family ",
    "Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious ",
    "x",
    "line\u{2028}break ",
    "a b c d e f g h i j k l m n o p q r s t u v w x y z ",
    "tab\there ",
    "<b>&amp;</b> ",
    // Reserved: the kernel refuses it (`Reason::Inline`).
    "\u{FFFC}",
    "\n",
    "word ",
];

const STYLE_NAMES: [&str; 12] = [
    "",
    "Normal",
    "Heading 1",
    "Heading 2",
    "Title",
    "S0",
    "S1",
    "S2",
    "a/b",
    "with spaces",
    "\u{fc}n\u{ef}c\u{f6}d\u{e9}",
    "Quote",
];

const LENGTHS: [&str; 14] = [
    "0pt",
    "6pt",
    "12pt",
    "-6pt",
    "1.5em",
    "1lh",
    "5%",
    "100%",
    "9999999pt",
    "-9999999pt",
    "garbage",
    "",
    "12",
    "2em + 3pt",
];

const URLS: [&str; 9] = [
    "https://example.com/a",
    "http://example.com/",
    "mailto:a@example.com",
    "tel:+15551234",
    "javascript:alert(1)",
    "file:///etc/passwd",
    "",
    "https://example.com/\u{202e}x",
    "not a url",
];

const LIST_FORMATS: [&str; 10] = [
    "bullet",
    "decimal",
    "decimal-leading-zero",
    "lower-alpha",
    "upper-alpha",
    "lower-roman",
    "upper-roman",
    "none",
    "bogus",
    "",
];

const LIST_TEXTS: [&str; 9] = [
    "%1.",
    "\u{2022}",
    "%1.%2",
    "%%",
    "(%1)",
    "%9",
    "%0",
    "",
    "%1.%2.%3.%4.%5.%6.%7.%8.%9",
];

const STARTS: [&str; 9] = [
    "1",
    "0",
    "-5",
    "7",
    "9223372036854775807",
    "-9223372036854775808",
    "99999999999999999999",
    "abc",
    "",
];

const ROLES: [BlockRole; 12] = [
    BlockRole::Body,
    BlockRole::Heading1,
    BlockRole::Heading2,
    BlockRole::Heading3,
    BlockRole::Heading4,
    BlockRole::Heading5,
    BlockRole::Heading6,
    BlockRole::Title,
    BlockRole::Subtitle,
    BlockRole::Quote,
    BlockRole::Code,
    BlockRole::Verse,
];

/// A UTF-8 boundary of `text` chosen by `selector`.
pub fn boundary(text: &str, selector: u16) -> u32 {
    let want = usize::from(selector) % (text.chars().count() + 1);
    text.char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .nth(want)
        .map_or(0, |i| i as u32)
}

pub struct Gen<'a, 'b> {
    pub input: &'a mut Input<'b>,
    pub snap: &'a Snap,
    /// Hashes of the registered image assets.
    pub assets: &'a [String],
    /// `field_target` strings the world has collected.
    pub targets: &'a [String],
}

impl<'a> Gen<'a, '_> {
    fn blocks(&self) -> &'a [Block] {
        self.snap.blocks()
    }

    fn matches(&self, b: &Block, want: Want) -> bool {
        match want {
            Want::Any => true,
            Want::Text => !b.text.is_empty(),
            Want::Listed => b.list.is_some(),
            Want::Inline => !b.inline.is_empty(),
            Want::Roled => b.role != BlockRole::Body,
            Want::Annotated => self
                .snap
                .state
                .annotations
                .iter()
                .any(|a| a.spans.iter().any(|s| s.node == b.id)),
        }
    }

    /// A block, preferring `want`.
    pub fn block(&mut self, want: Want) -> Option<&'a Block> {
        let all = self.blocks();
        let n = self.input.u16();
        let preferred: Vec<&Block> = all.iter().filter(|b| self.matches(b, want)).collect();
        if preferred.is_empty() {
            all.get(usize::from(n) % all.len().max(1))
        } else {
            preferred.get(usize::from(n) % preferred.len()).copied()
        }
    }

    /// A kind of block drawn from the interaction mix.
    pub fn any_block(&mut self) -> Option<&'a Block> {
        let want = match self.input.below(8) {
            0 | 1 => Want::Any,
            2 => Want::Listed,
            3 => Want::Inline,
            4 | 5 => Want::Annotated,
            6 => Want::Text,
            _ => Want::Roled,
        };
        self.block(want)
    }

    fn id_of(b: Option<&Block>) -> String {
        b.map_or_else(|| "0@0".to_owned(), |b| b.id.clone())
    }

    /// A node ID: usually a live block, sometimes a dead or malformed one.
    fn node(&mut self, want: Want) -> (Option<&'a Block>, String) {
        if self.input.chance(10) {
            let bogus = ["0@0", "", "999@999", "not an id", "1@1"];
            return (None, bogus[self.input.below(bogus.len())].to_owned());
        }
        let b = self.block(want);
        (b, Self::id_of(b))
    }

    /// A byte offset in `b`: mostly a boundary, sometimes inside a code
    /// point, past the end, or absurd.
    pub fn offset(&mut self, b: Option<&Block>) -> u32 {
        let text = b.map_or("", |b| b.text.as_str());
        let selector = self.input.u16();
        match self.input.u8() {
            0..=219 => boundary(text, selector),
            220..=239 => {
                // Inside a multi-byte character when there is one.
                let at = boundary(text, selector);
                if (at as usize) < text.len() {
                    at + 1
                } else {
                    at
                }
            }
            240..=249 => (text.len() as u32).saturating_add(1 + u32::from(selector % 8)),
            _ => u32::MAX,
        }
    }

    /// A valid range in `b`, ordered.
    fn range(&mut self, b: Option<&Block>) -> (u32, u32) {
        let text = b.map_or("", |b| b.text.as_str());
        match self.input.u8() {
            // The whole text.
            0..=59 => (0, text.len() as u32),
            // Around an inline object.
            60..=99 => {
                if let Some(entry) = b.and_then(|b| b.inline.first()) {
                    (entry.at, entry.at + 3)
                } else {
                    (0, text.len() as u32)
                }
            }
            // Around the edge of an annotation.
            100..=139 => {
                let id = b.map(|b| b.id.as_str()).unwrap_or_default();
                let span = self
                    .snap
                    .state
                    .annotations
                    .iter()
                    .flat_map(|a| a.spans.iter())
                    .find(|s| s.node == id);
                match span {
                    Some(s) => {
                        let at = if self.input.chance(128) {
                            s.start
                        } else {
                            s.end
                        };
                        let other = boundary(text, self.input.u16());
                        (at.min(other), at.max(other))
                    }
                    None => (0, text.len() as u32),
                }
            }
            _ => {
                let (x, y) = (
                    boundary(text, self.input.u16()),
                    boundary(text, self.input.u16()),
                );
                (x.min(y), x.max(y))
            }
        }
    }

    fn text(&mut self) -> String {
        let mut text = TEXTS[self.input.below(TEXTS.len())].to_owned();
        if self.input.chance(40) {
            text.push_str(TEXTS[self.input.below(TEXTS.len())]);
        }
        text
    }

    fn style_name(&mut self) -> String {
        if self.input.chance(50) {
            let names: Vec<&str> = self
                .snap
                .state
                .styles
                .iter()
                .map(|s| s.name.as_str())
                .collect();
            if let Some(name) = names.get(self.input.below(names.len().max(1))) {
                return (*name).to_owned();
            }
        }
        if self.input.chance(4) {
            return "n".repeat(2000);
        }
        STYLE_NAMES[self.input.below(STYLE_NAMES.len())].to_owned()
    }

    fn length(&mut self) -> Option<String> {
        if self.input.chance(160) {
            None
        } else {
            Some(LENGTHS[self.input.below(LENGTHS.len())].to_owned())
        }
    }

    /// A paragraph style with a few fields set.
    pub fn style(&mut self) -> Style {
        let mut style = Style::default();
        let fields = 1 + self.input.below(4);
        for _ in 0..fields {
            match self.input.below(16) {
                0 => {
                    style.alignment = Some(
                        [
                            Alignment::Start,
                            Alignment::Centre,
                            Alignment::End,
                            Alignment::Justify,
                        ][self.input.below(4)],
                    );
                }
                1 => style.weight = Some([0, 1, 400, 700, 1000, 1001][self.input.below(6)]),
                2 => style.size = self.length(),
                3 => style.line_height = self.length(),
                4 => style.indent_start = self.length(),
                5 => style.indent_end = self.length(),
                6 => style.indent_first = self.length(),
                7 => style.space_before = self.length(),
                8 => style.space_after = self.length(),
                9 => style.keep_with_next = Some(self.input.chance(128)),
                10 => style.keep_together = Some(self.input.chance(128)),
                11 => style.widows = Some([0, 1, 2, 32, 33][self.input.below(5)]),
                12 => style.orphans = Some([0, 1, 2, 32, 33][self.input.below(5)]),
                13 => style.background = Some([200, 200, 255, 255]),
                14 => {
                    style.families = Some(
                        [
                            vec!["Source Serif Pro".to_owned()],
                            vec!["serif".to_owned()],
                            vec!["Nonexistent".to_owned(), "Source Serif Pro".to_owned()],
                            vec![],
                        ][self.input.below(4)]
                        .clone(),
                    );
                }
                _ => style.slant = Some(FontStyle::Italic),
            }
        }
        style
    }

    fn clear_list(&mut self) -> Vec<StyleProperty> {
        const ALL: [StyleProperty; 14] = [
            StyleProperty::Parent,
            StyleProperty::Families,
            StyleProperty::Size,
            StyleProperty::LineHeight,
            StyleProperty::Alignment,
            StyleProperty::Weight,
            StyleProperty::Slant,
            StyleProperty::IndentStart,
            StyleProperty::IndentFirst,
            StyleProperty::SpaceBefore,
            StyleProperty::KeepWithNext,
            StyleProperty::Widows,
            StyleProperty::Background,
            StyleProperty::Underline,
        ];
        (0..self.input.below(4))
            .map(|_| ALL[self.input.below(ALL.len())])
            .collect()
    }

    fn text_style(&mut self) -> TextStyle {
        let mut s = TextStyle::default();
        for _ in 0..1 + self.input.below(3) {
            match self.input.below(9) {
                0 => s.weight = Some([100, 400, 700, 1000][self.input.below(4)]),
                1 => s.slant = Some(FontStyle::Italic),
                2 => {
                    s.decoration = Decoration {
                        underline: Some(self.input.chance(200)),
                        strike: Some(self.input.chance(100)),
                    }
                }
                3 => s.color = Some([200, 20, 20, 255]),
                4 => s.size = Some([1024, 12 * 1024, 40 * 1024, 0, -5][self.input.below(5)]),
                5 => s.language = Some(["en", "ar", "he", "ja", "x"][self.input.below(5)].into()),
                6 => {
                    s.features = Some(vec![TextFeature {
                        tag: ["liga", "smcp", "bad", "\u{e9}\u{e9}\u{e9}\u{e9}"]
                            [self.input.below(4)]
                        .into(),
                        value: 1,
                    }])
                }
                7 => s.reset = true,
                _ => s.clear = self.clear_list(),
            }
        }
        s
    }

    fn list_definition(&mut self) -> ListDefinition {
        let ids: Vec<&str> = self
            .snap
            .state
            .lists
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        let base = if self.input.chance(50) {
            ids.get(self.input.below(ids.len().max(1)))
                .map(|id| (*id).to_owned())
        } else {
            None
        };
        // Most definitions are sound (distinct levels, known formats, usable
        // numbers), so updating and enrolling succeed often; one in six may
        // carry any bad field, which is refused whole.
        let hostile = self.input.chance(40);
        let count = self.input.below(4);
        let levels = (0..count)
            .map(|n| {
                let mut level = ListLevel {
                    level: n as u8,
                    ..ListLevel::default()
                };
                if hostile && self.input.chance(100) {
                    level.level = [0, 9, 255][self.input.below(3)];
                }
                // LIST_FORMATS[..8] are the known formats.
                let formats = if hostile { LIST_FORMATS.len() } else { 8 };
                if self.input.chance(200) {
                    level.format = Some(LIST_FORMATS[self.input.below(formats)].into());
                }
                let texts = if hostile { LIST_TEXTS.len() } else { 5 };
                if self.input.chance(200) {
                    level.text = Some(LIST_TEXTS[self.input.below(texts)].into());
                }
                // STARTS[..5] are numbers a counter takes.
                let starts = if hostile { STARTS.len() } else { 5 };
                if self.input.chance(80) {
                    level.start = Some(STARTS[self.input.below(starts)].into());
                }
                let lengths: &[i32] = if hostile {
                    &[0, 18 * 1024, -1, i32::MAX, i32::MIN]
                } else {
                    &[0, 18 * 1024, 36 * 1024]
                };
                if self.input.chance(100) {
                    level.indent = Some(lengths[self.input.below(lengths.len())]);
                }
                if self.input.chance(100) {
                    level.hanging = Some(lengths[self.input.below(lengths.len())]);
                }
                if self.input.chance(40) {
                    level.marker = Some(self.text_style());
                }
                level
            })
            .collect();
        ListDefinition { base, levels }
    }

    fn list_ref(&mut self) -> ListRef {
        let ids: Vec<&str> = self
            .snap
            .state
            .lists
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        match self.input.below(10) {
            0 => ListRef::New { index: 0 },
            1 => ListRef::Existing {
                id: "0@0".to_owned(),
            },
            _ => match ids.get(self.input.below(ids.len().max(1))) {
                Some(id) => ListRef::Existing {
                    id: (*id).to_owned(),
                },
                None => ListRef::New { index: 0 },
            },
        }
    }

    fn list_id(&mut self) -> String {
        let ids: Vec<&str> = self
            .snap
            .state
            .lists
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        if self.input.chance(15) {
            return "0@0".to_owned();
        }
        ids.get(self.input.below(ids.len().max(1)))
            .map_or_else(|| "0@0".to_owned(), |id| (*id).to_owned())
    }

    fn level(&mut self) -> u8 {
        [0, 0, 0, 1, 1, 2, 3, 8, 9, 255][self.input.below(10)]
    }

    pub fn inline_object(&mut self) -> InlineObject {
        let align = [
            InlineAlign::Baseline,
            InlineAlign::Middle,
            InlineAlign::TextTop,
            InlineAlign::TextBottom,
        ][self.input.below(4)];
        let mut params = BTreeMap::new();
        let size = |g: &mut Self| {
            [
                "pt:36864",
                "pt:0",
                "pt:-1024",
                "em:1000",
                "em:20000",
                "pct:500",
                "pct:5000000",
                "garbage",
                "",
                "pt:2147483647",
            ][g.input.below(10)]
            .to_owned()
        };
        let kind = match self.input.below(10) {
            0..=3 => {
                params.insert(
                    "asset".to_owned(),
                    if self.assets.is_empty() || self.input.chance(25) {
                        ["00", "", &"a".repeat(64)][self.input.below(3)].to_owned()
                    } else {
                        self.assets[self.input.below(self.assets.len())].clone()
                    },
                );
                if self.input.chance(180) {
                    let v = size(self);
                    params.insert("width".to_owned(), v);
                }
                if self.input.chance(180) {
                    let v = size(self);
                    params.insert("height".to_owned(), v);
                }
                if self.input.chance(120) {
                    params.insert("alt".to_owned(), self.text());
                }
                "image"
            }
            4..=7 => {
                let function = ["page-number", "page-count", "page-of", "text", "bogus", ""]
                    [self.input.below(6)];
                params.insert("field".to_owned(), function.to_owned());
                match function {
                    "page-of" => {
                        let target = if self.targets.is_empty() || self.input.chance(30) {
                            ["", "bogus", "copy:0@0 0"][self.input.below(3)].to_owned()
                        } else {
                            self.targets[self.input.below(self.targets.len())].clone()
                        };
                        if self.input.chance(230) {
                            params.insert("target".to_owned(), target);
                        }
                    }
                    "text" => {
                        params.insert("text".to_owned(), self.text());
                    }
                    _ => {}
                }
                "field"
            }
            8 => {
                params.insert("x".to_owned(), "y".to_owned());
                "future-kind"
            }
            _ => "",
        };
        InlineObject {
            kind: kind.to_owned(),
            align,
            params,
        }
    }

    fn inline_patch(&mut self) -> InlinePatch {
        let mut params = BTreeMap::new();
        for _ in 0..self.input.below(3) {
            let name = [
                "width", "height", "alt", "field", "text", "asset", "target", "zz",
            ][self.input.below(8)];
            let value = if self.input.chance(60) {
                None
            } else if name == "width" || name == "height" {
                Some(
                    ["pt:72000", "em:2000", "pct:100", "garbage", "pt:-5"][self.input.below(5)]
                        .to_owned(),
                )
            } else {
                Some(self.text())
            };
            params.insert(name.to_owned(), value);
        }
        InlinePatch {
            align: if self.input.chance(100) {
                Some(InlineAlign::Middle)
            } else {
                None
            },
            params,
        }
    }

    fn annotation_payload(&mut self) -> AnnotationPayload {
        match self.input.below(8) {
            0..=2 => AnnotationPayload::Link {
                target: LinkDestination::Url {
                    url: URLS[self.input.below(URLS.len())].to_owned(),
                },
            },
            3 => AnnotationPayload::Link {
                target: LinkDestination::Node {
                    node: Self::id_of(self.block(Want::Any)),
                },
            },
            4 => AnnotationPayload::Link {
                target: LinkDestination::Heading {
                    node: Self::id_of(self.block(Want::Roled)),
                },
            },
            5 => {
                let ids: Vec<&str> = self
                    .snap
                    .state
                    .annotations
                    .iter()
                    .map(|a| a.id.as_str())
                    .collect();
                AnnotationPayload::Link {
                    target: LinkDestination::Range {
                        range: ids
                            .get(self.input.below(ids.len().max(1)))
                            .map_or_else(|| "0@0".to_owned(), |id| (*id).to_owned()),
                    },
                }
            }
            _ => AnnotationPayload::Comment {
                thread: ["t1", "t2", "t3", "", "thread with spaces"][self.input.below(5)]
                    .to_owned(),
            },
        }
    }

    fn annotation_id(&mut self) -> String {
        let ids: Vec<&str> = self
            .snap
            .state
            .annotations
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        if self.input.chance(10) {
            return "0@0".to_owned();
        }
        ids.get(self.input.below(ids.len().max(1)))
            .map_or_else(|| "0@0".to_owned(), |id| (*id).to_owned())
    }

    /// One command, weighted toward the wave-1 vocabulary. A quarter of the
    /// time it is drawn evenly from the commands the weights make rare (list
    /// numbering and removal, style rename and delete, annotation update and
    /// removal, object update, moves, Enter and Backspace), so a short run
    /// still attempts every command many times. One byte picks either way.
    pub fn command(&mut self) -> Command {
        let slot = if self.input.chance(64) {
            RARE_SLOTS[self.input.below(RARE_SLOTS.len())]
        } else {
            self.input.below(100) as u8
        };
        self.command_in(slot)
    }

    /// The command at percent slot `slot` of the weight table.
    fn command_in(&mut self, slot: u8) -> Command {
        match slot {
            0..=9 => {
                let (b, node) = self.node(Want::Any);
                let at = self.offset(b);
                let text = self.text();
                Command::InsertText { node, at, text }
            }
            10..=14 => {
                let (b, node) = self.node(Want::Text);
                let (start, end) = self.range(b);
                Command::DeleteText { node, start, end }
            }
            15..=19 => {
                let b = self.any_block();
                let at = if self.input.chance(128) {
                    self.range(b).0
                } else {
                    self.offset(b)
                };
                Command::SplitBlock {
                    node: Self::id_of(b),
                    at,
                }
            }
            20..=22 => {
                let all = self.blocks();
                let first = self.any_block();
                let second = match all
                    .iter()
                    .position(|b| first.is_some_and(|f| f.id == b.id))
                    .and_then(|i| all.get(i + 1))
                {
                    Some(next) if self.input.chance(230) => Some(next),
                    // Not an adjacent paragraph: refused, or an unordered join
                    // that drops the link coverage of the text it appends.
                    _ => first,
                };
                Command::JoinBlocks {
                    first: Self::id_of(first),
                    second: Self::id_of(second),
                }
            }
            23..=24 => Command::InsertBlock {
                parent: None,
                index: self.input.below(self.blocks().len() + 2) as u32,
                block_kind: reprise::BlockKind::Paragraph,
                text: self.text(),
                style: Style {
                    families: Some(vec!["Source Serif Pro".into()]),
                    ..Style::default()
                },
            },
            25..=26 => Command::DeleteBlock {
                node: Self::id_of(self.any_block()),
            },
            27 => {
                let node = Self::id_of(self.any_block());
                Command::MoveBlock {
                    node,
                    parent: None,
                    index: self.input.below(self.blocks().len().max(1)) as u32,
                }
            }
            28..=30 => {
                let node = Self::id_of(self.any_block());
                let style = self.style();
                let clear = if self.input.chance(100) {
                    Some(self.clear_list())
                } else {
                    None
                };
                Command::SetStyle { node, style, clear }
            }
            31..=34 => {
                let (b, node) = self.node(Want::Any);
                let (start, end) = self.range(b);
                let style = self.text_style();
                Command::FormatText {
                    node,
                    start,
                    end,
                    style,
                }
            }
            35..=38 => {
                let name = self.style_name();
                let parent = if self.input.chance(100) {
                    Some(self.style_name())
                } else {
                    None
                };
                Command::DefineStyle {
                    name,
                    parent,
                    style: self.style(),
                }
            }
            39..=41 => Command::UpdateStyle {
                name: self.style_name(),
                parent: if self.input.chance(60) {
                    Some(self.style_name())
                } else {
                    None
                },
                style: self.style(),
                clear: self.clear_list(),
            },
            42 => Command::RenameStyle {
                from: self.style_name(),
                to: self.style_name(),
            },
            43 => {
                // Mostly a style the document defined; built-ins refuse.
                let custom: Vec<&str> = self
                    .snap
                    .state
                    .styles
                    .iter()
                    .filter(|s| !s.builtin && !s.name.is_empty())
                    .map(|s| s.name.as_str())
                    .collect();
                let name = match custom.get(self.input.below(custom.len().max(1))) {
                    Some(name) if self.input.chance(220) => (*name).to_owned(),
                    _ => self.style_name(),
                };
                Command::DeleteStyle { name }
            }
            44..=48 => {
                let b = self.any_block();
                let style = if self.input.chance(50) {
                    None
                } else {
                    Some(self.style_name())
                };
                Command::ApplyStyle {
                    node: Self::id_of(b),
                    style,
                }
            }
            49..=52 => Command::SetRole {
                node: Self::id_of(self.any_block()),
                role: ROLES[self.input.below(ROLES.len())],
            },
            53 => Command::CreateList {
                definition: self.list_definition(),
            },
            54 => Command::UpdateList {
                list: self.list_id(),
                definition: self.list_definition(),
            },
            55 => Command::DeleteList {
                list: self.list_ref(),
            },
            56..=59 => {
                let node = Self::id_of(self.any_block());
                Command::SetListItem {
                    node,
                    list: self.list_ref(),
                    level: self.level(),
                }
            }
            60 => Command::RemoveListItem {
                node: Self::id_of(self.block(Want::Listed)),
            },
            61 => Command::SetListLevel {
                node: Self::id_of(self.block(Want::Listed)),
                level: self.level(),
            },
            62 => Command::IndentListItem {
                node: Self::id_of(self.block(Want::Listed)),
                by: [1, -1, 1, 2, -8, 127, -128][self.input.below(7)],
            },
            63 => Command::RestartList {
                node: Self::id_of(self.block(Want::Listed)),
                value: if self.input.chance(60) {
                    None
                } else {
                    // Mostly numbers a counter takes (STARTS[..5]).
                    let known = if self.input.chance(160) {
                        5
                    } else {
                        STARTS.len()
                    };
                    Some(STARTS[self.input.below(known)].to_owned())
                },
            },
            64 => Command::ContinueList {
                node: Self::id_of(self.block(Want::Listed)),
            },
            65..=67 => {
                let b = self.block(Want::Listed);
                let at = self.offset(b);
                Command::Enter {
                    node: Self::id_of(b),
                    at,
                }
            }
            68..=69 => Command::BackspaceAtStart {
                node: Self::id_of(self.block(Want::Listed)),
            },
            70..=79 => {
                let (b, node) = self.node(Want::Annotated);
                let at = self.offset(b);
                Command::InsertInline {
                    node,
                    at,
                    object: self.inline_object(),
                }
            }
            80..=83 => {
                let b = self.block(Want::Inline);
                let at = match b.and_then(|b| b.inline.get(self.input.below(b.inline.len().max(1))))
                {
                    Some(entry) if self.input.chance(230) => entry.at,
                    _ => self.offset(b),
                };
                Command::UpdateInline {
                    node: Self::id_of(b),
                    at,
                    patch: self.inline_patch(),
                }
            }
            84..=89 => {
                let (b, node) = self.node(Want::Text);
                let (start, end) = self.range(b);
                Command::AddAnnotation {
                    node,
                    start,
                    end,
                    annotation: self.annotation_payload(),
                }
            }
            90..=92 => Command::UpdateAnnotation {
                id: self.annotation_id(),
                annotation: self.annotation_payload(),
            },
            93..=94 => Command::RemoveAnnotation {
                id: self.annotation_id(),
            },
            95 => {
                let (b, node) = self.node(Want::Any);
                let at = self.offset(b);
                Command::InsertLineBreak { node, at }
            }
            96 => {
                let (b, node) = self.node(Want::Any);
                let at = self.offset(b);
                Command::InsertTab { node, at }
            }
            _ => {
                let node = Self::id_of(self.any_block());
                Command::SetAlignment {
                    node,
                    at: None,
                    alignment: if self.input.chance(60) {
                        None
                    } else {
                        Some(Alignment::Centre)
                    },
                }
            }
        }
    }

    /// A batch of interacting commands: the seams the brief names.
    pub fn recipe(&mut self) -> Vec<Command> {
        let b = self.block(Want::Text);
        let node = Self::id_of(b);
        let len = b.map_or(0, |b| b.text.len() as u32);
        let mid = b.map_or(0, |b| boundary(&b.text, self.input.u16()));
        let link = AnnotationPayload::Link {
            target: LinkDestination::Url {
                url: "https://example.com/r".into(),
            },
        };
        let field = |name: &str| InlineObject {
            kind: "field".into(),
            align: InlineAlign::Baseline,
            params: BTreeMap::from([("field".to_owned(), name.to_owned())]),
        };
        let new_list = |g: &mut Self| Command::CreateList {
            definition: g.list_definition(),
        };
        match self.input.below(11) {
            // An atom inside a link.
            0 => vec![
                Command::AddAnnotation {
                    node: node.clone(),
                    start: 0,
                    end: len,
                    annotation: link,
                },
                Command::InsertInline {
                    node,
                    at: mid,
                    object: self.inline_object(),
                },
            ],
            // A field in a list item.
            1 => vec![
                new_list(self),
                Command::SetListItem {
                    node: node.clone(),
                    list: ListRef::New { index: 0 },
                    level: 0,
                },
                Command::InsertInline {
                    node,
                    at: 0,
                    object: field("page-number"),
                },
            ],
            // A link across a split, then an atom in the second half.
            2 => vec![
                Command::AddAnnotation {
                    node: node.clone(),
                    start: 0,
                    end: len,
                    annotation: link,
                },
                Command::SplitBlock {
                    node: node.clone(),
                    at: mid,
                },
                Command::InsertInline {
                    node,
                    at: 0,
                    object: self.inline_object(),
                },
            ],
            // A style applied to a list item, with its role.
            3 => vec![
                new_list(self),
                Command::SetListItem {
                    node: node.clone(),
                    list: ListRef::New { index: 0 },
                    level: 1,
                },
                Command::SetRole {
                    node: node.clone(),
                    role: BlockRole::Heading2,
                },
                Command::ApplyStyle {
                    node,
                    style: Some("Heading 2".into()),
                },
            ],
            // A list built and split with Enter, one level in, then backed out.
            4 => vec![
                new_list(self),
                Command::SetListItem {
                    node: node.clone(),
                    list: ListRef::New { index: 0 },
                    level: 0,
                },
                Command::Enter {
                    node: node.clone(),
                    at: mid,
                },
                Command::IndentListItem {
                    node: node.clone(),
                    by: 1,
                },
                Command::BackspaceAtStart { node },
            ],
            // A format across an atom and a link, then a partial delete.
            5 => vec![
                Command::InsertInline {
                    node: node.clone(),
                    at: mid,
                    object: self.inline_object(),
                },
                Command::AddAnnotation {
                    node: node.clone(),
                    start: 0,
                    end: len.saturating_add(3),
                    annotation: link,
                },
                Command::FormatText {
                    node: node.clone(),
                    start: 0,
                    end: len.saturating_add(3),
                    style: self.text_style(),
                },
                Command::DeleteText {
                    node,
                    start: mid,
                    end: mid.saturating_add(3),
                },
            ],
            // Define a style, apply it to a listed block, update it, rename it.
            6 => {
                let name = format!("R{}", self.input.below(3));
                vec![
                    Command::DefineStyle {
                        name: name.clone(),
                        parent: Some("Normal".into()),
                        style: self.style(),
                    },
                    Command::ApplyStyle {
                        node: Self::id_of(self.block(Want::Listed)),
                        style: Some(name.clone()),
                    },
                    Command::UpdateStyle {
                        name: name.clone(),
                        parent: None,
                        style: self.style(),
                        clear: self.clear_list(),
                    },
                    Command::RenameStyle {
                        from: name,
                        to: format!("R{}", self.input.below(5)),
                    },
                ]
            }
            // Heading role plus its default style, as the docs recommend.
            7 => vec![
                Command::SetRole {
                    node: node.clone(),
                    role: ROLES[1 + self.input.below(6)],
                },
                Command::ApplyStyle {
                    node,
                    style: Some(format!("Heading {}", 1 + self.input.below(6))),
                },
            ],
            // A comment and a link over the same text, an atom between.
            8 => vec![
                Command::AddAnnotation {
                    node: node.clone(),
                    start: 0,
                    end: len,
                    annotation: AnnotationPayload::Comment {
                        thread: "t-r".into(),
                    },
                },
                Command::AddAnnotation {
                    node: node.clone(),
                    start: 0,
                    end: len,
                    annotation: link,
                },
                Command::InsertInline {
                    node,
                    at: mid,
                    object: field("page-count"),
                },
            ],
            // A list's life: make it, enrol the paragraph, then one of the
            // numbering commands, in one batch (a `New` reference reaches the
            // list before it has an ID).
            9 => {
                let act = match self.input.below(6) {
                    0 => Command::RestartList {
                        node: node.clone(),
                        value: Some(STARTS[self.input.below(4)].to_owned()),
                    },
                    1 => Command::ContinueList { node: node.clone() },
                    2 => Command::SetListLevel {
                        node: node.clone(),
                        level: 1 + self.input.below(3) as u8,
                    },
                    3 => Command::IndentListItem {
                        node: node.clone(),
                        by: 1,
                    },
                    4 => Command::DeleteList {
                        list: ListRef::New { index: 0 },
                    },
                    _ => Command::RemoveListItem { node: node.clone() },
                };
                vec![
                    new_list(self),
                    Command::SetListItem {
                        node,
                        list: ListRef::New { index: 0 },
                        level: 0,
                    },
                    act,
                ]
            }
            // Annotation churn: two over overlapping text, text deleted
            // between their ends, one updated and one removed by ID.
            _ => {
                let ids: Vec<String> = self
                    .snap
                    .state
                    .annotations
                    .iter()
                    .map(|a| a.id.clone())
                    .collect();
                let mut batch = vec![
                    Command::AddAnnotation {
                        node: node.clone(),
                        start: 0,
                        end: mid.max(len / 2),
                        annotation: link,
                    },
                    Command::AddAnnotation {
                        node: node.clone(),
                        start: mid.min(len / 2),
                        end: len,
                        annotation: AnnotationPayload::Comment {
                            thread: "t-c".into(),
                        },
                    },
                    Command::DeleteText {
                        node,
                        start: mid.min(len / 2),
                        end: mid.max(len / 2),
                    },
                ];
                if let Some(id) = ids.get(self.input.below(ids.len().max(1))) {
                    batch.push(if self.input.chance(128) {
                        Command::UpdateAnnotation {
                            id: id.clone(),
                            annotation: self.annotation_payload(),
                        }
                    } else {
                        Command::RemoveAnnotation { id: id.clone() }
                    });
                }
                batch
            }
        }
    }
}

/// The weight-table slots of the commands that are rare there: MoveBlock,
/// RenameStyle, DeleteStyle, UpdateList, DeleteList, the numbering commands
/// (RemoveListItem to ContinueList), Enter, BackspaceAtStart, UpdateInline,
/// UpdateAnnotation and RemoveAnnotation.
const RARE_SLOTS: [u8; 15] = [27, 42, 43, 54, 55, 60, 61, 62, 63, 64, 65, 68, 80, 90, 93];

/// A numbering command aimed at a paragraph that is not a list item is
/// refused. Most of the time give it a list first: a new list, and the
/// paragraph enrolled in it, ahead of the command in the same transaction
/// (`ListRef::New` reaches the list before it has an ID). Returns how many
/// commands it added.
pub fn enrol_list_targets(
    snap: &Snap,
    commands: &mut Vec<Command>,
    input: &mut Input<'_>,
) -> usize {
    let listed = |node: &str| {
        snap.state
            .blocks
            .iter()
            .any(|b| b.id == node && b.list.is_some())
    };
    let mut out = Vec::with_capacity(commands.len() + 2);
    let mut creates = 0u32;
    let mut enrolled: Vec<String> = Vec::new();
    let mut added = 0;
    for command in commands.drain(..) {
        let target = match &command {
            Command::RemoveListItem { node }
            | Command::SetListLevel { node, .. }
            | Command::IndentListItem { node, .. }
            | Command::RestartList { node, .. }
            | Command::ContinueList { node } => Some(node.clone()),
            _ => None,
        };
        if let Some(node) = target
            && !listed(&node)
            && !enrolled.contains(&node)
            && snap.state.blocks.iter().any(|b| b.id == node)
            && input.chance(220)
        {
            out.push(Command::CreateList {
                definition: ListDefinition {
                    base: None,
                    levels: (0..3)
                        .map(|level| ListLevel {
                            level,
                            format: Some(
                                ["decimal", "lower-alpha", "bullet"][usize::from(level)].into(),
                            ),
                            ..ListLevel::default()
                        })
                        .collect(),
                },
            });
            out.push(Command::SetListItem {
                node: node.clone(),
                list: ListRef::New { index: creates },
                level: 0,
            });
            creates += 1;
            enrolled.push(node);
            added += 2;
        }
        if matches!(command, Command::CreateList { .. }) {
            creates += 1;
        }
        out.push(command);
    }
    *commands = out;
    added
}
