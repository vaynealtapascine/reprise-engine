//! Versioned, flat authored page properties. No defaults are written on first
//! edit: independent peers must not compete to create a nested container.
use loro::{LoroValue, ValueOrContainer};
use reprise_geom::Length;

use crate::{Dim, DocError, Document, FrameRole, MAIN_FLOW, Medium, PageTemplate};

const ROOT: &str = "page-setup1";
const KEYS: [&str; 6] = ["width", "height", "top", "right", "bottom", "left"];
/// Maximum page dimension/margin: 200 inches (14,400 pt), in layout units.
pub const MAX_PAGE_LENGTH: Length = Length::from_pt(14_400);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageSetupPatch {
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub top: Option<Length>,
    pub right: Option<Length>,
    pub bottom: Option<Length>,
    pub left: Option<Length>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_empty_patches_do_not_create_authored_roots() {
        let doc = Document::new(1).unwrap();
        let revision = doc.revision();
        let medium = Medium::new(Length::MAX, Length::MIN);
        assert_eq!(doc.page_setup_patch().unwrap(), PageSetupPatch::default());
        assert!(
            doc.patch_page_template(&PageTemplate::builtin(), &medium)
                .unwrap()
                .is_none()
        );
        doc.set_page_setup_patch(PageSetupPatch::default()).unwrap();
        assert!(!doc.has_page_setup());
        assert_eq!(revision, doc.revision());
    }

    #[test]
    fn malformed_scalars_containers_and_unknown_versions_are_refused_without_repair() {
        for value in [
            LoroValue::Null,
            LoroValue::String("{".into()),
            LoroValue::Double(1.5),
            LoroValue::I64(i64::MAX),
            LoroValue::I64(i64::MIN),
        ] {
            let doc = Document::new(1).unwrap();
            doc.doc.get_map(ROOT).insert("width", value).unwrap();
            doc.commit();
            let revision = doc.revision();
            assert_eq!(doc.page_setup_patch(), Err(PageSetupError::Unreadable));
            assert_eq!(doc.revision(), revision);
            assert!(doc.has_page_setup());
        }
        let doc = Document::new(1).unwrap();
        doc.doc
            .get_text(ROOT)
            .insert(0, "future root type")
            .unwrap();
        doc.commit();
        let revision = doc.revision();
        assert_eq!(doc.page_setup_patch(), Err(PageSetupError::Unreadable));
        assert_eq!(revision, doc.revision());
        let doc = Document::new(1).unwrap();
        doc.doc
            .get_map(ROOT)
            .insert_container("width", loro::LoroMap::new())
            .unwrap();
        assert_eq!(doc.page_setup_patch(), Err(PageSetupError::Unreadable));
        let doc = Document::new(1).unwrap();
        doc.doc.get_map(ROOT).insert("future", 1).unwrap();
        assert_eq!(doc.page_setup_patch(), Err(PageSetupError::Unreadable));
        // Cardinality is checked before walking keys, so huge unknown maps
        // do not introduce unbounded interpretation of page properties.
        for i in 0..10_000 {
            doc.doc
                .get_map(ROOT)
                .insert(&format!("future-{i}"), i)
                .unwrap();
        }
        assert_eq!(doc.page_setup_patch(), Err(PageSetupError::Unreadable));
    }

    #[test]
    fn extreme_dimensions_never_overflow_and_scalar_refusal_writes_nothing() {
        let base = PageDimensions {
            width: Length::MAX,
            height: Length::MIN,
            top: Length::MAX,
            right: Length::MAX,
            bottom: Length::MIN,
            left: Length::MIN,
        };
        assert_eq!(
            PageSetupPatch::default().apply(base),
            Err(PageSetupError::Size)
        );
        let doc = Document::new(1).unwrap();
        for bad in [Length::MIN, Length(-1), Length::ZERO, Length::MAX] {
            assert!(
                doc.set_page_setup_patch(PageSetupPatch {
                    width: Some(Length::from_pt(600)),
                    height: Some(bad),
                    ..Default::default()
                })
                .is_err()
            );
            assert!(!doc.has_page_setup());
        }
        let exact = PageDimensions {
            width: MAX_PAGE_LENGTH,
            height: MAX_PAGE_LENGTH,
            top: Length::ZERO,
            right: Length::ZERO,
            bottom: Length::ZERO,
            left: Length::ZERO,
        };
        assert_eq!(exact.validate(), Ok(()));
        assert_eq!(
            PageSetupPatch {
                left: Some(Length::MAX),
                right: Some(Length::MAX),
                ..Default::default()
            }
            .apply(exact),
            Err(PageSetupError::Margins)
        );
    }

    #[test]
    fn patch_retains_frame_identity_writing_mode_transform_and_other_regions() {
        let doc = Document::new(1).unwrap();
        let mut base = PageTemplate::builtin();
        let frame = base.frames.first_mut().unwrap();
        frame.writing_mode = crate::WritingMode::VerticalRl;
        frame.transform.mirror_x = true;
        let other = base.frames.get(1).unwrap().clone();
        doc.set_page_setup_patch(PageSetupPatch {
            width: Some(Length::from_pt(612)),
            ..Default::default()
        })
        .unwrap();
        let patched = doc
            .patch_page_template(&base, &Medium::new(Length::ZERO, Length::ZERO))
            .unwrap()
            .unwrap();
        let main = patched.frames.first().unwrap();
        assert_eq!(main.name, "main");
        assert_eq!(main.writing_mode, crate::WritingMode::VerticalRl);
        assert!(main.transform.mirror_x);
        assert_eq!(patched.frames.get(1), Some(&other));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageDimensions {
    pub width: Length,
    pub height: Length,
    pub top: Length,
    pub right: Length,
    pub bottom: Length,
    pub left: Length,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PageSetupError {
    #[error("page dimensions must be positive and at most 14,400 pt")]
    Size,
    #[error("margins must be nonnegative, at most 14,400 pt, and leave a positive text area")]
    Margins,
    #[error("page setup properties are unreadable or have unknown keys")]
    Unreadable,
    #[error("the base template has no resolvable main text frame")]
    Template,
}

impl PageSetupPatch {
    pub fn is_empty(self) -> bool {
        self.values().into_iter().all(|v| v.is_none())
    }
    fn values(self) -> [Option<Length>; 6] {
        [
            self.width,
            self.height,
            self.top,
            self.right,
            self.bottom,
            self.left,
        ]
    }
    /// Later supplied properties replace earlier ones; absent fields retain them.
    pub fn overlay(self, patch: Self) -> Self {
        Self {
            width: patch.width.or(self.width),
            height: patch.height.or(self.height),
            top: patch.top.or(self.top),
            right: patch.right.or(self.right),
            bottom: patch.bottom.or(self.bottom),
            left: patch.left.or(self.left),
        }
    }
    pub fn apply(self, base: PageDimensions) -> Result<PageDimensions, PageSetupError> {
        let result = PageDimensions {
            width: self.width.unwrap_or(base.width),
            height: self.height.unwrap_or(base.height),
            top: self.top.unwrap_or(base.top),
            right: self.right.unwrap_or(base.right),
            bottom: self.bottom.unwrap_or(base.bottom),
            left: self.left.unwrap_or(base.left),
        };
        result.validate()?;
        Ok(result)
    }
}

impl PageDimensions {
    pub fn validate(self) -> Result<(), PageSetupError> {
        if [self.width, self.height]
            .into_iter()
            .any(|v| v <= Length::ZERO || v > MAX_PAGE_LENGTH)
        {
            return Err(PageSetupError::Size);
        }
        if [self.top, self.right, self.bottom, self.left]
            .into_iter()
            .any(|v| v < Length::ZERO || v > MAX_PAGE_LENGTH)
            || i64::from(self.left.0) + i64::from(self.right.0) >= i64::from(self.width.0)
            || i64::from(self.top.0) + i64::from(self.bottom.0) >= i64::from(self.height.0)
        {
            return Err(PageSetupError::Margins);
        }
        Ok(())
    }

    /// Physical rectangle before the frame's authored transform. With several
    /// main frames, the first in threading order is the page-setup frame.
    pub fn from_template(template: &PageTemplate, medium: &Medium) -> Result<Self, PageSetupError> {
        let width = template
            .width
            .resolve(medium, None)
            .ok_or(PageSetupError::Template)?;
        let height = template
            .height
            .resolve(medium, None)
            .ok_or(PageSetupError::Template)?;
        let frame = template
            .frames
            .iter()
            .find(|f| f.role == FrameRole::Flow(MAIN_FLOW.into()))
            .ok_or(PageSetupError::Template)?;
        let resolve = |dim: Dim| {
            dim.resolve(medium, Some((width, height)))
                .ok_or(PageSetupError::Template)
        };
        let left = resolve(frame.x)?;
        let top = resolve(frame.y)?;
        Ok(Self {
            width,
            height,
            top,
            left,
            right: width - left - resolve(frame.width)?,
            bottom: height - top - resolve(frame.height)?,
        })
    }
}

impl Document {
    /// Presence includes an emptied root's retained undo/sync history. Reads
    /// never create this root, so untouched documents need no feature bit.
    pub fn has_page_setup(&self) -> bool {
        match self.doc.get_value() {
            LoroValue::Map(roots) => roots.contains_key(ROOT),
            _ => true,
        }
    }

    pub fn page_setup_patch(&self) -> Result<PageSetupPatch, PageSetupError> {
        let LoroValue::Map(roots) = self.doc.get_value() else {
            return Err(PageSetupError::Unreadable);
        };
        match roots.get(ROOT) {
            None => return Ok(PageSetupPatch::default()),
            Some(LoroValue::Container(id)) if id.container_type() == loro::ContainerType::Map => {}
            Some(_) => return Err(PageSetupError::Unreadable),
        }
        let map = self.doc.get_map(ROOT);
        if map.len() > KEYS.len() || map.keys().any(|k| !KEYS.contains(&k.as_ref())) {
            return Err(PageSetupError::Unreadable);
        }
        let read = |key| match map.get(key) {
            None => Ok(None),
            Some(ValueOrContainer::Value(LoroValue::I64(v))) => i32::try_from(v)
                .map(|v| Some(Length(v)))
                .map_err(|_| PageSetupError::Unreadable),
            Some(_) => Err(PageSetupError::Unreadable),
        };
        Ok(PageSetupPatch {
            width: read("width")?,
            height: read("height")?,
            top: read("top")?,
            right: read("right")?,
            bottom: read("bottom")?,
            left: read("left")?,
        })
    }

    /// Interpret patches without rewriting merged authored data. Invalid
    /// combinations are refused as a whole; layout reports the fallback.
    pub fn patch_page_template(
        &self,
        base: &PageTemplate,
        medium: &Medium,
    ) -> Result<Option<PageTemplate>, PageSetupError> {
        let patch = self.page_setup_patch()?;
        if patch.is_empty() {
            return Ok(None);
        }
        let setup = patch.apply(PageDimensions::from_template(base, medium)?)?;
        let mut template = base.clone();
        template.width = Dim::Pt(setup.width);
        template.height = Dim::Pt(setup.height);
        let frame = template
            .frames
            .iter_mut()
            .find(|f| f.role == FrameRole::Flow(MAIN_FLOW.into()))
            .ok_or(PageSetupError::Template)?;
        frame.x = Dim::Pt(setup.left);
        frame.y = Dim::Pt(setup.top);
        frame.width = Dim::Pt(setup.width - setup.left - setup.right);
        frame.height = Dim::Pt(setup.height - setup.top - setup.bottom);
        Ok(Some(template))
    }

    /// The editing kernel validates against its sequential model first. This
    /// method additionally checks each scalar, and writes only supplied keys.
    pub fn set_page_setup_patch(&self, patch: PageSetupPatch) -> Result<(), DocError> {
        for (key, value) in KEYS.into_iter().zip(patch.values()) {
            if let Some(value) = value {
                let minimum = if key == "width" || key == "height" {
                    Length(1)
                } else {
                    Length::ZERO
                };
                if value < minimum || value > MAX_PAGE_LENGTH {
                    return Err(DocError::Store("invalid page setup scalar".into()));
                }
            }
        }
        if !patch.is_empty() {
            let map = self.doc.get_map(ROOT);
            for (key, value) in KEYS.into_iter().zip(patch.values()) {
                if let Some(value) = value {
                    map.insert(key, i64::from(value.0))?;
                }
            }
        }
        Ok(())
    }
}
