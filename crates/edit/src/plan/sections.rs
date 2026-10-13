//! Sequential section validation; all staging happens after this model succeeds.
use super::*;
use crate::{BreakAt, RegionBinding, StoryFrom};
use reprise_doc::sections::{BreakKind, SectionId, SectionProperties};
use reprise_doc::{BlockKind, Domain};
use serde_json::Value;

impl Model<'_> {
    pub(super) fn domain(&self, node: NodeId) -> Result<Option<Domain>, Reason> {
        let mut at = Some(node);
        for _ in 0..MAX_ANCESTORS {
            let Some(n) = at else { return Ok(None) };
            if self.killed.contains(&n) {
                return Ok(None);
            };
            match self.parent_of(n) {
                Some(Some(parent)) => at = Some(parent),
                Some(None) => {
                    return Ok(Some(if self.doc.stories().contains(&n) {
                        Domain::Story(n)
                    } else {
                        Domain::Body
                    }));
                }
                None => return Ok(None),
            }
        }
        Err(Reason::TreeDepthLimit {
            node,
            max: MAX_ANCESTORS,
        })
    }
    fn section_error(s: impl Into<String>) -> Reason {
        Reason::Section(s.into())
    }
    fn properties(&mut self, section: SectionId) -> Result<&mut SectionProperties, Reason> {
        match section {
            SectionId::First => Ok(&mut self.first),
            SectionId::Block(id) => {
                self.require(id)?;
                if !self.kids(None).contains(&Slot::Real(id)) {
                    return Err(Self::section_error(
                        "section is outside body top-level order",
                    ));
                }
                match self.starts.get_mut(&Slot::Real(id)) {
                    Some((BreakKind::Section, properties)) => Ok(properties),
                    _ => Err(Self::section_error("no effective section at this address")),
                }
            }
        }
    }
    pub(super) fn set_section(
        &mut self,
        section: SectionId,
        patch: &SectionProperties,
        clear: &[String],
    ) -> Result<(), Reason> {
        reprise_doc::sections::validate_properties(patch, clear).map_err(Reason::from)?;
        self.add_payload(
            serde_json::to_vec(patch)
                .map_err(|e| Self::section_error(e.to_string()))?
                .len(),
        )?;
        let properties = self.properties(section)?;
        for key in clear {
            properties.remove(key);
        }
        properties.extend(patch.clone());
        let kind = properties
            .get("break")
            .and_then(Value::as_str)
            .and_then(BreakKind::parse);
        if let SectionId::Block(id) = section {
            if let Some(kind) = kind {
                if let Some((old, _)) = self.starts.get_mut(&Slot::Real(id)) {
                    *old = kind;
                }
            } else {
                self.starts.remove(&Slot::Real(id));
            }
        }
        if !patch.is_empty() || !clear.is_empty() {
            self.plan.steps.push(Step::SetSection {
                section,
                patch: patch.clone(),
                clear: clear.to_vec(),
            });
        }
        Ok(())
    }
    pub(super) fn insert_break(
        &mut self,
        at: BreakAt,
        kind: BreakKind,
        patch: &SectionProperties,
    ) -> Result<(), Reason> {
        reprise_doc::sections::validate_properties(patch, &[]).map_err(Reason::from)?;
        let (node, offset) = match at {
            BreakAt::Before(id) => (id, 0),
            BreakAt::Position { node, offset } => (node, offset),
        };
        self.require(node)?;
        if self.domain(node)? != Some(Domain::Body) || !self.kids(None).contains(&Slot::Real(node))
        {
            return Err(Self::section_error("breaks require a body top-level block"));
        }
        if matches!(at, BreakAt::Position { .. })
            && self.doc.kind_of(node) != Some(BlockKind::Paragraph)
        {
            return Err(Reason::NotText(node));
        }
        if offset == 0 && self.starts.contains_key(&Slot::Real(node)) {
            return Err(Reason::BreakExists(node));
        }
        let (_, index) = self.slot_of(node)?;
        let mut properties = self.first.clone();
        for slot in self.kids(None).clone().iter().take(index.saturating_add(1)) {
            if let Some((BreakKind::Section, own)) = self.starts.get(slot) {
                properties = own.clone();
            }
        }
        properties.retain(|k, _| {
            matches!(
                k.as_str(),
                "template"
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
                    | "direction"
                    | "writing-mode"
            ) || k.starts_with("columns.")
        });
        properties.extend(patch.clone());
        reprise_doc::sections::validate_properties(&properties, &[]).map_err(Reason::from)?;
        self.add_payload(
            serde_json::to_vec(&properties)
                .map_err(|e| Self::section_error(e.to_string()))?
                .len(),
        )?;
        let target = if offset == 0 {
            Slot::Real(node)
        } else {
            let new = self.plan.created.len();
            self.split(node, offset)?;
            Slot::New(new)
        };
        properties.insert("break".into(), Value::String(kind.token().into()));
        self.starts.insert(target, (kind, properties.clone()));
        self.plan.steps.push(Step::InsertStart {
            target,
            kind,
            properties,
        });
        Ok(())
    }
    pub(super) fn remove_break(&mut self, block: NodeId) -> Result<(), Reason> {
        self.require(block)?;
        if !self.kids(None).contains(&Slot::Real(block))
            || self.starts.remove(&Slot::Real(block)).is_none()
        {
            return Err(Self::section_error("no effective break at this address"));
        }
        self.plan.steps.push(Step::RemoveStart { block });
        Ok(())
    }
    pub(super) fn require_story(&self, story: NodeId) -> Result<(), Reason> {
        self.require(story)?;
        if !self.doc.stories().contains(&story) {
            return Err(Self::section_error("not a live root story"));
        }
        Ok(())
    }
    fn binding_key(slot: &str, variant: &str) -> Result<String, Reason> {
        // Unknown slots are retained; bound their token without hard-coding a registry.
        if slot.is_empty()
            || slot.len() > 128
            || !slot
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || !matches!(variant, "default" | "first" | "even")
        {
            return Err(Self::section_error("invalid region slot or variant"));
        }
        Ok(format!("region.{slot}.{variant}"))
    }
    pub(super) fn set_region(
        &mut self,
        section: SectionId,
        slot: &str,
        variant: &str,
        binding: RegionBinding,
    ) -> Result<(), Reason> {
        let key = Self::binding_key(slot, variant)?;
        let (patch, clear) = match binding {
            RegionBinding::Inherit => (SectionProperties::new(), vec![key]),
            RegionBinding::None => ([(key, Value::String(String::new()))].into(), vec![]),
            RegionBinding::Story(story) => {
                self.require_story(story)?;
                ([(key, Value::String(story.to_string()))].into(), vec![])
            }
        };
        self.set_section(section, &patch, &clear)
    }
    pub(super) fn create_story(
        &mut self,
        blocks: &[NewBlock],
        binding: Option<(SectionId, String)>,
    ) -> Result<(), Reason> {
        for block in blocks {
            if block.text.contains(reprise_doc::text::BREAK) {
                return Err(Self::section_error("U+FDD0 is not text"));
            }
            self.add_payload(block.text.len())?;
        }
        let new = self.plan.stories.len();
        self.plan
            .stories
            .push(StoryCreation::Blocks(blocks.to_vec()));
        self.plan.steps.push(Step::CreateStory { new, binding });
        Ok(())
    }
    pub(super) fn create_region(
        &mut self,
        section: SectionId,
        slot: &str,
        variant: &str,
        from: StoryFrom,
    ) -> Result<(), Reason> {
        let key = Self::binding_key(slot, variant)?;
        self.properties(section)?;
        match from {
            StoryFrom::Empty => self.create_story(&[], Some((section, key))),
            StoryFrom::CopyOf(story) => {
                self.require_story(story)?;
                // Copy the sequential prefix, including prior text/style/split edits,
                // without writing anything to the author's document.
                let shadow = self.doc.validation_fork().map_err(Reason::from)?;
                let editor = crate::Editor::new(shadow, self.schemas.clone());
                editor.write(self.plan.clone()).map_err(|e| e.reason)?;
                let doc = editor.document();
                let selection: Vec<_> = doc
                    .order(Domain::Story(story))
                    .into_iter()
                    .map(|node| reprise_doc::fragment::CopyBlock { node, bytes: None })
                    .collect();
                let fragment = doc
                    .copy_fragment_scoped(
                        "story-copy",
                        &selection,
                        self.schemas,
                        reprise_doc::fragment::CopyScope::Document,
                    )
                    .map_err(Reason::Fragment)?;
                self.add_payload(
                    serde_json::to_vec(&fragment)
                        .map_err(|e| Self::section_error(e.to_string()))?
                        .len(),
                )?;
                let prepared =
                    crate::paste::prepare(doc, self.schemas, &fragment, None, "story-copy")
                        .map_err(|e| e.reason)?;
                let new = self.plan.stories.len();
                self.plan
                    .stories
                    .push(StoryCreation::Copy(Box::new(prepared)));
                self.plan.steps.push(Step::CreateStory {
                    new,
                    binding: Some((section, key)),
                });
                Ok(())
            }
        }
    }
}
