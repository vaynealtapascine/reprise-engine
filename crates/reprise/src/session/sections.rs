//! Section, page, region and field queries, and navigation in running regions
//! (sections §12, §17). Every query reads the current complete layout; none
//! changes the document. A layout with no sections (a document with no breaks,
//! stories or geometry record) is one section, `first`, with decimal numbers.
use super::*;
use reprise_doc::sections::SectionId;
use reprise_geom::Point;
use reprise_layout::{FieldInstance, PageKind};

fn kind(k: PageKind) -> SectionPageKind {
    match k {
        PageKind::Filler => SectionPageKind::Filler,
        PageKind::NoteContinuation => SectionPageKind::NoteContinuation,
        _ => SectionPageKind::Content,
    }
}

fn place(i: FieldInstance) -> FieldPlace {
    match i {
        FieldInstance::Page(page) => FieldPlace::Page { page },
        FieldInstance::Running { page, slot, repeat } => FieldPlace::Running {
            page,
            slot: slot.token().to_owned(),
            repeat,
        },
        _ => FieldPlace::Single,
    }
}

impl DocumentSession {
    fn page_index(&self, page: u32) -> Result<usize> {
        let page = page as usize;
        if page >= self.current()?.pages.len() {
            return Err(Error::Invalid(format!("no page {page}")));
        }
        Ok(page)
    }

    /// A position in a story or the body, checked against the document alone:
    /// a running region's blocks are not in the layout's body blocks.
    fn checked_position(&self, c: &Caret) -> Result<reprise_edit::Caret> {
        let c = cv::caret(c)?;
        let block = self
            .doc()
            .block(c.node)
            .map_err(|_| Error::InvalidId(c.node.to_string()))?;
        let text = block.text.to_string();
        if c.offset > text.len() || !text.is_char_boundary(c.offset) {
            return Err(Error::Invalid(
                "caret is not at a UTF-8 boundary in its block".into(),
            ));
        }
        Ok(c)
    }

    /// A saved running instance as navigation context. Its page is where the
    /// story was drawn when the caller saved it, so it may be past the end of a
    /// layout that has lost pages since: the kernel then answers from the
    /// nearest page that still shows the story. Only its representation and
    /// slot are checked here. Hit queries ask about a page that exists now, and
    /// check it with `page_index`.
    fn instance(&self, i: &RegionInstance) -> Result<reprise_edit::RegionInstance> {
        let page =
            usize::try_from(i.page).map_err(|_| Error::Invalid(format!("no page {}", i.page)))?;
        if reprise_layout::SlotId::parse(&i.slot).is_none() {
            return Err(Error::Invalid(format!("no region slot {}", i.slot)));
        }
        Ok(reprise_edit::RegionInstance {
            page,
            slot: i.slot.clone(),
        })
    }

    /// Who owns page `page`, and the bands of body text on it, each with the
    /// section that owns it.
    pub fn page_sections(&self, page: u32) -> Result<Payload<PageSections>> {
        let page = self.page_index(page)?;
        let snapshot = self.current()?;
        let Some(descriptor) = snapshot.sections.as_ref().and_then(|s| s.pages.get(page)) else {
            return Ok(Payload::new(PageSections {
                page: page as u32,
                kind: SectionPageKind::Content,
                owner: SectionId::First.to_string(),
                bands: Vec::new(),
            }));
        };
        Ok(Payload::new(PageSections {
            page: page as u32,
            kind: kind(descriptor.kind),
            owner: descriptor.section.to_string(),
            bands: descriptor
                .bands
                .iter()
                .map(|b| PageBand {
                    section: b.section.to_string(),
                    life: b.life.clone(),
                    rect: cv::rect(b.body),
                    offset: b.offset.0,
                    depth: b.depth.0,
                    balanced: b.balanced,
                })
                .collect(),
        }))
    }

    /// The pages section `section` made (`owned`) and those with any of its
    /// text (`occupied`), each in page order.
    pub fn section_pages(&self, section: &Payload<String>) -> Result<Payload<SectionPages>> {
        let raw = validate(section)?;
        if raw.len() > 80 {
            return Err(Error::InvalidId(raw.clone()));
        }
        let wanted = SectionId::parse(raw).ok_or_else(|| Error::InvalidId(raw.clone()))?;
        let snapshot = self.current()?;
        if wanted != SectionId::First && !self.doc().section_table().iter().any(|v| v.id == wanted)
        {
            return Err(Error::InvalidId(raw.clone()));
        }
        let Some(sections) = snapshot.sections.as_ref() else {
            let all: Vec<u32> = (0..snapshot.pages.len() as u32).collect();
            return Ok(Payload::new(if wanted == SectionId::First {
                SectionPages {
                    owned: all.clone(),
                    occupied: all,
                }
            } else {
                SectionPages {
                    owned: Vec::new(),
                    occupied: Vec::new(),
                }
            }));
        };
        let mut owned = Vec::new();
        let mut occupied = Vec::new();
        for (page, d) in sections.pages.iter().enumerate() {
            let page = page as u32;
            if d.section == wanted {
                owned.push(page);
            }
            if d.bands.iter().any(|b| b.section == wanted) {
                occupied.push(page);
            }
        }
        Ok(Payload::new(SectionPages { owned, occupied }))
    }

    /// Page `page`'s counter and label.
    pub fn page_label(&self, page: u32) -> Result<Payload<PageLabel>> {
        let index = self.page_index(page)?;
        let snapshot = self.current()?;
        Ok(Payload::new(
            match snapshot.sections.as_ref().and_then(|s| s.pages.get(index)) {
                Some(d) => PageLabel {
                    page,
                    label: d.label.clone(),
                    counter: d.counter.to_string(),
                    kind: kind(d.kind),
                },
                None => PageLabel {
                    page,
                    label: index.saturating_add(1).to_string(),
                    counter: index.saturating_add(1).to_string(),
                    kind: SectionPageKind::Content,
                },
            },
        ))
    }

    /// The running regions shown on page `page`, in the layout's order.
    pub fn page_regions(&self, page: u32) -> Result<Payload<Vec<PageRegion>>> {
        let page = self.page_index(page)?;
        let snapshot = self.current()?;
        Ok(Payload::new(
            snapshot
                .sections
                .iter()
                .flat_map(|s| &s.running)
                .filter(|r| r.page == page)
                .filter_map(|r| {
                    let frame = snapshot.frame(r.frame)?;
                    Some(PageRegion {
                        slot: r.slot.clone(),
                        variant: r.variant.clone(),
                        story: r.story.to_string(),
                        rect: cv::rect(frame.to_page.bounds(&frame.rect)),
                        clipped: r.clipped,
                    })
                })
                .collect(),
        ))
    }

    /// Every page and slot that shows `story`, in page order.
    pub fn region_instances(
        &self,
        story: &Payload<String>,
    ) -> Result<Payload<Vec<RegionInstance>>> {
        let raw = validate(story)?;
        let story = cv::id(raw)?;
        let snapshot = self.current()?;
        Ok(Payload::new(
            snapshot
                .sections
                .iter()
                .flat_map(|s| &s.running)
                .filter(|r| r.story == story)
                .map(|r| RegionInstance {
                    page: r.page as u32,
                    slot: r.slot.clone(),
                })
                .collect(),
        ))
    }

    /// The fields on page `page`, body first and then each running region, with
    /// what each shows there.
    pub fn page_fields(&self, page: u32) -> Result<Payload<Vec<PageField>>> {
        let page = self.page_index(page)?;
        let snapshot = self.current()?;
        let mut out = Vec::new();
        let mut add = |block: &reprise_layout::BlockLayout| {
            for line in &block.lines {
                let on = snapshot.frame(line.frame).map(|f| f.page);
                if on != Some(page) {
                    continue;
                }
                for atom in line
                    .atoms
                    .iter()
                    .filter(|a| a.kind == reprise_doc::inline::FIELD)
                {
                    out.push(PageField {
                        node: block.node.to_string(),
                        atom: atom.id.to_string(),
                        at: atom.at as u32,
                        instance: place(atom.instance),
                        text: atom.text(),
                    });
                }
            }
        };
        for block in &snapshot.blocks {
            add(block);
        }
        for instance in snapshot
            .sections
            .iter()
            .flat_map(|s| &s.running)
            .filter(|r| r.page == page)
        {
            for block in &instance.blocks {
                add(block);
            }
        }
        Ok(Payload::new(out))
    }

    /// What the point on a page means: body text, or a position in a running
    /// region. A header or footer wins over the body inside its rectangle.
    pub fn hit_placed(&self, request: &Payload<HitPlaced>) -> Result<Payload<PlacedHit>> {
        let r = validate(request)?;
        let page = self.page_index(r.page)?;
        let hit = self
            .navigator()?
            .hit_placed(
                page,
                Point::new(cv::length(r.x), cv::length(r.y)),
                match r.mode.unwrap_or_default() {
                    PlacedMode::Normal => reprise_edit::HitMode::Normal,
                    PlacedMode::Background => reprise_edit::HitMode::Background,
                },
            )
            .ok_or_else(|| Error::Invalid("hit page has no caret".into()))?;
        Ok(Payload::new(match hit {
            reprise_edit::PlacedHit::Body(hit) => PlacedHit::Body {
                hit: Hit {
                    caret: cv::caret_out(hit.caret),
                    page: hit.page as u32,
                    inside: hit.inside,
                },
            },
            reprise_edit::PlacedHit::Running { position, instance } => PlacedHit::Running {
                position: cv::caret_out(position),
                instance: RegionInstance {
                    page: instance.page as u32,
                    slot: instance.slot,
                },
            },
            _ => {
                return Err(Error::Invalid(
                    "a placement this binding doesn't know".into(),
                ));
            }
        }))
    }

    /// Where a position is drawn in a running region. If a relayout moved the
    /// story off `instance`, or removed its page, the nearest page that still
    /// shows it answers.
    pub fn caret_in(&self, request: &Payload<CaretIn>) -> Result<Payload<PageRect>> {
        let r = validate(request)?;
        let instance = self.instance(&r.instance)?;
        let position = self.checked_position(&r.position)?;
        let rect = self
            .navigator()?
            .caret_in(&instance, position)
            .ok_or(Error::NoLayout)?;
        Ok(Payload::new(PageRect {
            page: rect.page as u32,
            rect: cv::rect(rect.rect),
        }))
    }

    /// The page rectangles covering a selection in a running region, resolved
    /// like [`DocumentSession::caret_in`] (so `instance.page` may be a page the
    /// layout no longer has); empty when no page shows the story.
    pub fn selection_rects_in(
        &self,
        request: &Payload<SelectionIn>,
    ) -> Result<Payload<Vec<PageRect>>> {
        let r = validate(request)?;
        let instance = self.instance(&r.instance)?;
        let selection = reprise_edit::Selection {
            anchor: self.checked_position(&r.selection.anchor)?,
            focus: self.checked_position(&r.selection.focus)?,
        };
        Ok(Payload::new(
            self.navigator()?
                .selection_rects_in(&instance, &selection)
                .into_iter()
                .map(|r| PageRect {
                    page: r.page as u32,
                    rect: cv::rect(r.rect),
                })
                .collect(),
        ))
    }
}
