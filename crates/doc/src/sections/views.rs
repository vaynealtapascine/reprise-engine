//! Authored section ranges and forward binding inheritance for readers.
use super::*;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionView {
    pub id: SectionId,
    pub life: String,
    pub first_block: Option<NodeId>,
    pub end: Option<NodeId>,
    pub own: SectionProperties,
    pub bindings: Vec<BindingView>,
    pub unknown_keys: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingView {
    pub slot: String,
    pub variant: String,
    pub authored: String,
    pub story: Option<NodeId>,
    pub resolved: Option<NodeId>,
    pub from_section: SectionId,
}
impl Document {
    /// Resolves bindings once, in body order, including the empty first section.
    /// It never rewrites malformed values or falls through a missing explicit story.
    pub fn section_table(&self) -> Vec<SectionView> {
        if !self.has_sections() {
            return Vec::new();
        }
        let body = self.top(Domain::Body);
        let mut by_id: BTreeMap<_, _> = self
            .start_records()
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let first = by_id
            .remove(&SectionId::First)
            .map(|r| r.own)
            .unwrap_or_default();
        let mut sequence = vec![(SectionId::First, "0".to_owned(), first)];
        for &node in &body {
            if let Some(r) = by_id.remove(&SectionId::Block(node))
                && r.active
                && r.kind == Some(BreakKind::Section)
                && let Some(life) = r.life
            {
                sequence.push((r.id, life, r.own));
            }
        }
        let live: BTreeSet<_> = self.stories().into_iter().collect();
        let mut inherited: BTreeMap<(String, String), (Option<NodeId>, SectionId)> =
            BTreeMap::new();
        let mut out = Vec::with_capacity(sequence.len());
        for (i, (id, life, own)) in sequence.iter().enumerate() {
            let end = sequence
                .get(i.saturating_add(1))
                .and_then(|(id, _, _)| match id {
                    SectionId::Block(n) => Some(*n),
                    _ => None,
                });
            let first_block = match id {
                SectionId::First => body.first().copied().filter(|n| Some(*n) != end),
                SectionId::Block(n) => Some(*n),
            };
            let mut bindings = Vec::new();
            for slot in ["header", "footer", "background"] {
                for variant in ["default", "first", "even"] {
                    let key = format!("region.{slot}.{variant}");
                    let address = (slot.to_owned(), variant.to_owned());
                    let (authored, story, resolved, from_section) = match own.get(&key) {
                        None => {
                            let (resolved, from) = inherited
                                .get(&address)
                                .copied()
                                .unwrap_or((None, SectionId::First));
                            ("inherit", None, resolved, from)
                        }
                        Some(Value::String(s)) if s.is_empty() => {
                            inherited.insert(address, (None, *id));
                            ("none", None, None, *id)
                        }
                        Some(value) => {
                            let story = value.as_str().and_then(NodeId::parse);
                            let resolved = story.filter(|s| live.contains(s));
                            inherited.insert(address, (resolved, *id));
                            ("story", story, resolved, *id)
                        }
                    };
                    bindings.push(BindingView {
                        slot: slot.into(),
                        variant: variant.into(),
                        authored: authored.into(),
                        story,
                        resolved,
                        from_section,
                    });
                }
            }
            let unknown_keys = own
                .keys()
                .filter(|key| !known_property(key))
                .cloned()
                .collect();
            out.push(SectionView {
                id: *id,
                life: life.clone(),
                first_block,
                end,
                own: own.clone(),
                bindings,
                unknown_keys,
            });
        }
        out
    }
}
pub fn known_property(key: &str) -> bool {
    matches!(
        key,
        "break"
            | "start"
            | "template"
            | "mode"
            | "width"
            | "height"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "orientation"
            | "header.distance"
            | "footer.distance"
            | "columns.count"
            | "columns.gap"
            | "columns.widths"
            | "columns.rule"
            | "direction"
            | "writing-mode"
            | "numbering.restart"
            | "numbering.format"
            | "region.first"
            | "region.even"
            | "filler.regions"
            | "region.header.writing-mode"
            | "region.footer.writing-mode"
            | "region.background.writing-mode"
    ) || binding_key(key)
}
