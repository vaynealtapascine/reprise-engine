//! Authored emphasis and paint (08), shared by paragraphs and anchored patches.
use serde::{Deserialize, Serialize};
impl crate::Document {
    /// Detect retained paragraph/formatting emphasis, including tombstones.
    /// Unknown envelopes and the scan limit conservatively require support.
    pub fn has_text_emphasis(&self) -> bool {
        use loro::{Container, LoroValue, ValueOrContainer};
        const MAX_RECORDS: usize = 1 << 22;
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return true;
        };
        let mut visited = 0usize;
        let mut charge = || {
            visited = visited.saturating_add(1);
            visited > MAX_RECORDS
        };
        let style = |map: &loro::LoroMap| {
            ["weight", "slant", "underline", "strike", "color"]
                .iter()
                .any(|key| map.get(key).is_some())
        };
        let overrides = |meta: &loro::LoroMap| match meta.get("overrides") {
            Some(ValueOrContainer::Container(Container::Map(map))) => style(&map),
            Some(_) => true,
            None => false,
        };
        // Only authored style/range roots participate; an unrelated parameter or
        // a named style called "weight" must not change package compatibility.
        for name in ["styles", crate::flow::BREAKS] {
            if !roots.contains_key(name) {
                continue;
            }
            if let LoroValue::Map(records) = self.doc.get_map(name).get_value() {
                for value in records.values() {
                    if charge() {
                        return true;
                    }
                    let LoroValue::Container(id) = value else {
                        return true;
                    };
                    let Some(Container::Map(map)) = self.doc.get_container(id.clone()) else {
                        return true;
                    };
                    if if name == "styles" {
                        style(&map)
                    } else {
                        overrides(&map)
                    } {
                        return true;
                    }
                }
            }
        }
        for name in ["content", "ranges"] {
            if !roots.contains_key(name) {
                continue;
            }
            let tree = self.tree(name);
            for id in tree.nodes() {
                if charge() {
                    return true;
                }
                let Ok(meta) = tree.get_meta(id) else {
                    return true;
                };
                if name == "content" {
                    if overrides(&meta) {
                        return true;
                    }
                } else if meta.get("format1").is_some() {
                    let Some(raw) = crate::get_str(&meta, "format1") else {
                        return true;
                    };
                    if raw.len() > 32 * 1024 {
                        return true;
                    }
                    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        return true;
                    };
                    if json.get("version").and_then(|v| v.as_u64()) != Some(1) {
                        return true;
                    }
                    if json
                        .get("style")
                        .and_then(|s| s.as_object())
                        .is_none_or(|s| {
                            ["weight", "slant", "decoration", "color"]
                                .iter()
                                .any(|k| s.contains_key(*k))
                        })
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextSlant {
    #[default]
    Normal,
    Italic,
    Oblique,
}
impl TextSlant {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Italic => "italic",
            Self::Oblique => "oblique",
        }
    }
}
/// Independent patches: absent inherits; false explicitly removes that line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Decoration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
}
impl Decoration {
    pub fn is_empty(&self) -> bool {
        self.underline.is_none() && self.strike.is_none()
    }
    pub fn overlay(&mut self, patch: Self) {
        if patch.underline.is_some() {
            self.underline = patch.underline;
        }
        if patch.strike.is_some() {
            self.strike = patch.strike;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockKind, Document, PersistenceMode, Style, formatting::TextStyle, text::RangePolicy,
    };
    fn doc() -> (Document, crate::NodeId) {
        let doc = Document::new(1).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "abcdef")
            .unwrap();
        doc.commit();
        (doc, node)
    }
    #[test]
    fn concurrent_emphasis_and_decorations_compose_and_reset() {
        let (a, node) = doc();
        let b = a.fork(2).unwrap();
        a.format_text(
            node,
            0..5,
            &TextStyle {
                weight: Some(700),
                decoration: Decoration {
                    underline: Some(true),
                    strike: None,
                },
                ..Default::default()
            },
            RangePolicy::EXPANDING,
        )
        .unwrap();
        b.format_text(
            node,
            2..6,
            &TextStyle {
                slant: Some(TextSlant::Italic),
                decoration: Decoration {
                    underline: None,
                    strike: Some(true),
                },
                color: Some([12, 34, 56, 0]),
                ..Default::default()
            },
            RangePolicy::EXPANDING,
        )
        .unwrap();
        a.commit();
        b.commit();
        a.merge(&b).unwrap();
        b.merge(&a).unwrap();
        let runs = a.text_formats(node).unwrap();
        assert_eq!(runs, b.text_formats(node).unwrap());
        let overlap = &runs.runs[1].style;
        assert_eq!(overlap.weight, Some(700));
        assert_eq!(overlap.slant, Some(TextSlant::Italic));
        assert_eq!(
            overlap.decoration,
            Decoration {
                underline: Some(true),
                strike: Some(true)
            }
        );
        assert_eq!(overlap.color, Some([12, 34, 56, 0]));
        a.format_text(
            node,
            2..4,
            &TextStyle {
                decoration: Decoration {
                    underline: Some(false),
                    strike: None,
                },
                ..Default::default()
            },
            RangePolicy::FIXED,
        )
        .unwrap();
        let current = a.text_formats(node).unwrap();
        let middle = &current.runs[1].style;
        assert_eq!(middle.decoration.underline, Some(false));
        assert_eq!(middle.decoration.strike, Some(true));
        a.format_text(
            node,
            2..4,
            &TextStyle {
                reset: true,
                color: Some([0, 0, 0, 0]),
                ..Default::default()
            },
            RangePolicy::FIXED,
        )
        .unwrap();
        a.commit();
        let reset = a.text_formats(node).unwrap();
        assert_eq!(reset.runs[1].style.weight, None);
        assert_eq!(reset.runs[1].style.decoration, Decoration::default());
        for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
            let reopened = Document::import(&a.try_export(mode).unwrap(), 3).unwrap();
            assert_eq!(reset, reopened.text_formats(node).unwrap());
            assert!(reopened.has_text_emphasis());
        }
    }
    #[test]
    fn bounds_unknown_fields_and_retained_feature_detection() {
        let (doc, node) = doc();
        assert!(!doc.has_text_emphasis());
        doc.define_style("weight", &Style::default()).unwrap();
        doc.commit();
        assert!(!doc.has_text_emphasis());
        for weight in [0, 1001, u16::MAX] {
            let before = doc.revision();
            assert!(
                doc.format_text(
                    node,
                    0..6,
                    &TextStyle {
                        weight: Some(weight),
                        ..Default::default()
                    },
                    RangePolicy::FIXED
                )
                .is_err()
            );
            assert_eq!(before, doc.revision());
        }
        for weight in [1, 1000] {
            assert!(
                TextStyle {
                    weight: Some(weight),
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for raw in [
            r#"{"slant":"future"}"#,
            r#"{"decoration":{"overline":true}}"#,
            r#"{"color":[0,0,0,256]}"#,
        ] {
            assert!(serde_json::from_str::<TextStyle>(raw).is_err());
        }
        let range = doc
            .format_text(
                node,
                0..6,
                &TextStyle {
                    color: Some([0, 0, 0, 0]),
                    ..Default::default()
                },
                RangePolicy::FIXED,
            )
            .unwrap();
        doc.commit();
        assert!(doc.has_text_emphasis());
        doc.remove_text_format(range).unwrap();
        doc.commit();
        assert!(
            doc.has_text_emphasis(),
            "retained tombstone still requires the capability"
        );
    }
    #[test]
    fn paragraph_inheritance_and_unknown_keywords_are_preserved() {
        let (doc, node) = doc();
        doc.define_style(
            "parent",
            &Style {
                weight: Some(700),
                slant: Some(TextSlant::Oblique),
                decoration: Decoration {
                    underline: Some(true),
                    strike: Some(true),
                },
                color: Some([20, 40, 60, 0]),
                ..Default::default()
            },
        )
        .unwrap();
        doc.set_style_name(node, "parent").unwrap();
        doc.set_overrides(
            node,
            &Style {
                weight: Some(400),
                decoration: Decoration {
                    underline: Some(false),
                    strike: None,
                },
                ..Default::default()
            },
        )
        .unwrap();
        doc.commit();
        let used = doc.computed_style(node).unwrap();
        assert_eq!(used.weight, Some(400));
        assert_eq!(used.slant, Some(TextSlant::Oblique));
        assert_eq!(used.decoration.strike, Some(true));
        assert_eq!(used.decoration.underline, Some(false));
        assert_eq!(used.explain["weight"], "direct");
        assert!(doc.has_text_emphasis());
        doc.define_style(
            "future",
            &Style {
                unparsed_keywords: [("slant".into(), "future1:lean".into())].into(),
                ..Default::default()
            },
        )
        .unwrap();
        doc.set_style_name(node, "future").unwrap();
        doc.commit();
        assert_eq!(
            doc.style("future").unwrap().unparsed_keywords["slant"],
            "future1:lean"
        );
        assert!(
            doc.computed_style(node)
                .unwrap()
                .notes
                .iter()
                .any(|n| n.code == crate::codes::STYLE_UNPARSED)
        );
    }
    #[test]
    fn older_format1_reader_refuses_added_fields() {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        #[derive(Default)]
        struct OldStyle {
            reset: bool,
        }
        let _: OldStyle = serde_json::from_str(r#"{"reset":true}"#).unwrap();
        assert!(serde_json::from_str::<OldStyle>(r#"{"weight":700}"#).is_err());
        assert!(serde_json::from_str::<OldStyle>(r#"{"decoration":{"strike":true}}"#).is_err());
        assert!(OldStyle { reset: true }.reset);
    }
}
