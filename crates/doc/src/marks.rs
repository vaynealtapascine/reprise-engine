//! Authored inline positions: paragraph tabs and range-addressed line edges.
use std::borrow::Cow;

use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::relation::*;
use crate::{DocError, Document, NodeId, RelationId};

pub const ALIGNMENT: SchemaId = SchemaId::new("reprise.alignment");
pub(crate) const PATCHES: &str = "marks1";
pub const MAX_TAB_STOPS: usize = 256;
pub const MAX_ALIGNMENT_RELATIONS: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    #[default]
    Start,
    Centre,
    End,
}
impl Alignment {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Centre => "centre",
            Self::End => "end",
        }
    }
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "start" => Some(Self::Start),
            "centre" => Some(Self::Centre),
            "end" => Some(Self::End),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineEdge {
    Start,
    End,
}
impl LineEdge {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

/// A target position can identify either edge of a tab or a visual line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorEdge {
    #[default]
    Position,
    GapStart,
    GapEnd,
    LineStart,
    LineEnd,
}
impl AnchorEdge {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::GapStart => "gap-start",
            Self::GapEnd => "gap-end",
            Self::LineStart => "line-start",
            Self::LineEnd => "line-end",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabStop {
    /// Distance from logical inline start; None means the interval's end.
    pub position: Option<Length>,
    pub alignment: Alignment,
    pub leader: Option<char>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabStops {
    pub interval: Length,
    pub stops: Vec<TabStop>,
}
impl Default for TabStops {
    fn default() -> Self {
        Self {
            interval: Length::from_pt(36),
            stops: Vec::new(),
        }
    }
}
impl TabStops {
    pub fn validate(&self) -> Result<(), DocError> {
        let mut previous = Length::ZERO;
        let mut end_seen = false;
        if self.interval <= Length::ZERO || self.stops.len() > MAX_TAB_STOPS {
            return Err(DocError::Store("invalid tab interval or stop count".into()));
        }
        for stop in &self.stops {
            if end_seen
                || stop
                    .leader
                    .is_some_and(|c| c.is_control() || c.is_whitespace())
            {
                return Err(DocError::Store("invalid tab stop or leader".into()));
            }
            if let Some(position) = stop.position {
                if position <= previous {
                    return Err(DocError::Store("tab stops must increase from zero".into()));
                }
                previous = position;
            } else {
                end_seen = true;
            }
        }
        Ok(())
    }
}

pub fn schema() -> RelationSchema {
    RelationSchema {
        id: ALIGNMENT,
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![
            RoleSpec {
                name: Cow::Borrowed("line"),
                accepts: Cow::Borrowed(&[TargetClass::Range, TargetClass::Layout]),
                min: 1,
                max: Some(1),
            },
            RoleSpec {
                name: Cow::Borrowed("to"),
                accepts: Cow::Borrowed(&[TargetClass::Range, TargetClass::Layout]),
                min: 0,
                max: Some(1),
            },
        ],
        params: ["alignment", "edge", "target-edge"]
            .into_iter()
            .map(|name| ParamSpec {
                name: Cow::Borrowed(name),
                kind: ParamKind::Text,
                required: false,
            })
            .collect(),
        on_target_deleted: OnTargetDeleted::Rebind,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

impl Document {
    /// Flat property keys avoid concurrent first-container creation overwriting
    /// another peer's independent paragraph property.
    pub fn set_alignment(&self, node: NodeId, alignment: Alignment) -> Result<(), DocError> {
        self.block(node)?;
        self.doc
            .get_map(PATCHES)
            .insert(&format!("{node}/alignment"), alignment.keyword())?;
        Ok(())
    }
    pub fn set_tab_stops(&self, node: NodeId, tabs: &TabStops) -> Result<(), DocError> {
        self.block(node)?;
        tabs.validate()?;
        let raw = serde_json::to_string(tabs).map_err(|e| DocError::Store(e.to_string()))?;
        self.doc
            .get_map(PATCHES)
            .insert(&format!("{node}/tabs"), format!("tabs1:{raw}"))?;
        Ok(())
    }
    fn has_mark_patches(&self) -> bool {
        matches!(self.doc.get_value(), loro::LoroValue::Map(roots) if roots.contains_key(PATCHES))
    }
    pub(crate) fn clear_mark_patches(&self, node: NodeId) -> Result<(), DocError> {
        if self.has_mark_patches() {
            let map = self.doc.get_map(PATCHES);
            for property in ["alignment", "tabs"] {
                map.delete(&format!("{node}/{property}"))?;
            }
        }
        Ok(())
    }
    pub(crate) fn read_mark_patches(&self, node: NodeId, style: &mut crate::Style) {
        if !self.has_mark_patches() {
            return;
        }
        let map = self.doc.get_map(PATCHES);
        for property in ["alignment", "tabs"] {
            let key = format!("{node}/{property}");
            if map.get(&key).is_none() {
                continue;
            }
            let raw = crate::get_str(&map, &key).unwrap_or_else(|| "unreadable marks value".into());
            style.unparsed_keywords.remove(property);
            let valid = match property {
                "alignment" => {
                    style.alignment = Alignment::parse(&raw);
                    style.alignment.is_some()
                }
                _ => {
                    style.tabs = (raw.len() <= 32 * 1024)
                        .then(|| {
                            raw.strip_prefix("tabs1:")
                                .and_then(|s| serde_json::from_str::<TabStops>(s).ok())
                                .filter(|t| t.validate().is_ok())
                        })
                        .flatten();
                    style.tabs.is_some()
                }
            };
            if !valid {
                style.unparsed_keywords.insert(property.into(), raw);
            }
        }
    }
    pub fn alignment_count(&self) -> usize {
        self.relations()
            .iter()
            .filter(|(_, r)| r.as_ref().is_ok_and(|r| r.schema == ALIGNMENT))
            .count()
    }
    /// Line defaults and pins are independent authored relations. Commands replace
    /// previously observed defaults on the same authored line, retaining concurrent ones.
    pub fn align_line(
        &self,
        node: NodeId,
        at: usize,
        alignment: Alignment,
    ) -> Result<RelationId, DocError> {
        self.write_alignment(node, at, None, Some(alignment))
    }
    pub fn pin_line(
        &self,
        node: NodeId,
        at: usize,
        edge: LineEdge,
        target: NodeId,
        target_at: usize,
        target_edge: AnchorEdge,
    ) -> Result<RelationId, DocError> {
        self.write_alignment(node, at, Some((edge, target, target_at, target_edge)), None)
    }
    fn write_alignment(
        &self,
        node: NodeId,
        at: usize,
        pin: Option<(LineEdge, NodeId, usize, AnchorEdge)>,
        alignment: Option<Alignment>,
    ) -> Result<RelationId, DocError> {
        let text = self.block(node)?.text;
        text.slice(at..at)?;
        if self.alignment_count() >= MAX_ALIGNMENT_RELATIONS {
            return Err(DocError::Store("alignment relation limit".into()));
        }
        if let Some((_, target, target_at, _)) = pin {
            self.block(target)?.text.slice(target_at..target_at)?;
        }
        let old: Vec<_> = if alignment.is_some() {
            let raw = text.to_string();
            let line_start = authored_start(&raw, at);
            self.relations()
                .into_iter()
                .filter_map(|(id, r)| {
                    let r = r.ok()?;
                    if r.schema != ALIGNMENT || !r.params.contains_key("alignment") {
                        return None;
                    }
                    let Target::Range(range) = r.first("line")? else {
                        return None;
                    };
                    let extent = self.range_extent(*range)?;
                    (extent.start.0 == node && authored_start(&raw, extent.start.1) == line_start)
                        .then_some(id)
                })
                .collect()
        } else {
            Vec::new()
        };
        let policy = crate::text::RangePolicy::POINT;
        let source = self.stage_fragment_range(node, at..at, policy)?;
        let mut relation = Relation::new(ALIGNMENT)
            .owned_by(node)
            .target("line", Target::Range(source));
        let mut ranges = vec![source];
        if let Some(a) = alignment {
            relation = relation.param("alignment", Param::Text(a.keyword().into()));
        }
        if let Some((edge, target, target_at, target_edge)) = pin {
            let to = self.stage_fragment_range(target, target_at..target_at, policy)?;
            ranges.push(to);
            relation = relation
                .target("to", Target::Range(to))
                .param("edge", Param::Text(edge.keyword().into()))
                .param("target-edge", Param::Text(target_edge.keyword().into()));
        }
        let id = self.stage_relation(&SchemaRegistry::builtin(), &relation)?;
        for range in ranges {
            self.activate_fragment_range(range)?;
        }
        for old in old {
            self.delete_relation(old)?;
        }
        self.restore_relation(id)?;
        Ok(id)
    }

    /// Conservative feature detection includes tombstones and unknown values.
    pub fn has_marks(&self) -> bool {
        use loro::{Container, LoroValue, ValueOrContainer};
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return true;
        };
        if roots.contains_key(PATCHES) && !self.doc.get_map(PATCHES).is_empty() {
            return true;
        }
        let style = |map: &loro::LoroMap| {
            ["alignment", "tabs"]
                .iter()
                .any(|key| map.get(key).is_some())
        };
        let overrides = |meta: &loro::LoroMap| match meta.get("overrides") {
            Some(ValueOrContainer::Container(Container::Map(m))) => style(&m),
            Some(_) => true,
            None => false,
        };
        let mut visited = 0usize;
        for name in ["styles", crate::flow::BREAKS] {
            if !roots.contains_key(name) {
                continue;
            }
            if let LoroValue::Map(records) = self.doc.get_map(name).get_value() {
                for value in records.values() {
                    visited = visited.saturating_add(1);
                    if visited > 1 << 22 {
                        return true;
                    }
                    let LoroValue::Container(id) = value else {
                        return true;
                    };
                    let Some(Container::Map(m)) = self.doc.get_container(id.clone()) else {
                        return true;
                    };
                    if if name == "styles" {
                        style(&m)
                    } else {
                        overrides(&m)
                    } {
                        return true;
                    }
                }
            }
        }
        for name in ["content", "relations"] {
            if !roots.contains_key(name) {
                continue;
            }
            let tree = self.tree(name);
            for id in tree.nodes() {
                visited = visited.saturating_add(1);
                if visited > 1 << 22 {
                    return true;
                }
                let Ok(meta) = tree.get_meta(id) else {
                    return true;
                };
                if name == "content" {
                    if overrides(&meta) {
                        return true;
                    }
                    if let Some(ValueOrContainer::Container(Container::Text(t))) = meta.get("text")
                        && t.to_string().chars().any(|c| c == '\t' || is_line_break(c))
                    {
                        return true;
                    }
                } else if crate::get_str(&meta, "json")
                    .is_none_or(|s| s.contains("reprise.alignment"))
                {
                    return true;
                }
            }
        }
        false
    }
}
pub fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}
pub fn authored_start(text: &str, at: usize) -> usize {
    text.get(..at)
        .and_then(|s| {
            s.char_indices()
                .rev()
                .find(|(_, c)| is_line_break(*c))
                .map(|(i, c)| i.saturating_add(c.len_utf8()))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, Style};

    #[test]
    fn independent_first_property_edits_merge_and_report_the_block() {
        let a = Document::new(1).unwrap();
        let node = a.append_block(BlockKind::Paragraph, "", "a\tb").unwrap();
        a.set_overrides(
            node,
            &Style {
                weight: Some(700),
                ..Default::default()
            },
        )
        .unwrap();
        a.commit();
        let b = a.fork(2).unwrap();
        a.set_alignment(node, Alignment::End).unwrap();
        b.set_tab_stops(node, &TabStops::default()).unwrap();
        a.commit();
        b.commit();
        let (result, changes) = a.tracked(|| a.merge(&b));
        result.unwrap();
        assert!(changes.blocks.contains(&node));
        b.merge(&a).unwrap();
        let style = a.block(node).unwrap().overrides;
        assert_eq!(style, b.block(node).unwrap().overrides);
        assert_eq!(style.alignment, Some(Alignment::End));
        assert_eq!(style.tabs, Some(TabStops::default()));
        assert_eq!(style.weight, Some(700));
        a.set_overrides(node, &Style::default()).unwrap();
        assert_eq!(a.block(node).unwrap().overrides, Style::default());
    }

    #[test]
    fn malformed_extreme_and_unbalanced_tabs_are_refused_or_preserved() {
        for interval in [Length::MIN, Length::ZERO, Length(-1)] {
            assert!(
                TabStops {
                    interval,
                    stops: Vec::new()
                }
                .validate()
                .is_err()
            );
        }
        let stop = TabStop {
            position: Some(Length::MAX),
            alignment: Alignment::End,
            leader: None,
        };
        assert!(
            TabStops {
                interval: Length::MAX,
                stops: vec![stop.clone()]
            }
            .validate()
            .is_ok()
        );
        assert!(
            TabStops {
                interval: Length(1),
                stops: vec![stop; MAX_TAB_STOPS + 1]
            }
            .validate()
            .is_err()
        );
        let doc = Document::new(1).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "").unwrap();
        for raw in [
            "tabs1:{",
            "tabs1:[[[[[[[[[[[[[[[[[[[",
            "tabs1:{\"interval\":0,\"stops\":[]}",
        ] {
            doc.doc
                .get_map(PATCHES)
                .insert(&format!("{node}/tabs"), raw)
                .unwrap();
            let style = doc.block(node).unwrap().overrides;
            assert!(style.tabs.is_none());
            assert_eq!(
                style.unparsed_keywords.get("tabs").map(String::as_str),
                Some(raw)
            );
            assert!(doc.has_marks());
        }
    }
}
