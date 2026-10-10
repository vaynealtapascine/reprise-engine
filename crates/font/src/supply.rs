//! Frontend declarations, CSS-inspired matching, and reproducible generic defaults.
use crate::{Face, FaceId, FontError, FontStore};
use reprise_diag::{Code, Note};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
pub mod codes {
    use super::Code;
    pub const UNREADABLE: Code = Code::new("font.unreadable");
    pub const NEAREST: Code = Code::new("font.nearest");
    pub const SYNTHETIC: Code = Code::new("font.synthetic");
    pub const CONFIG: Code = Code::new("font.config");
}
impl FontError {
    pub fn note(&self) -> Note {
        match self {
            Self::Unreadable(_) => Note::error(codes::UNREADABLE, self.to_string()),
            Self::Missing(_) => Note::warning(Code::new("font.missing"), self.to_string()),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}
/// Weight 1..=1000; stretch is positive permille (1000 = normal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Descriptors {
    pub weight: u16,
    pub style: FontStyle,
    pub stretch: u16,
}
impl Default for Descriptors {
    fn default() -> Self {
        Self {
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontDeclaration {
    pub family: String,
    pub descriptors: Descriptors,
    pub face_index: u32,
}
impl FontDeclaration {
    pub fn validate(&self) -> Result<(), FontError> {
        if self.family.trim().is_empty()
            || self.family.len() > 1024
            || !(1..=1000).contains(&self.descriptors.weight)
            || self.descriptors.stretch == 0
        {
            return Err(FontError::Unreadable(
                "invalid font declaration descriptors or family".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenericFamily {
    Serif,
    SansSerif,
    Monospace,
    Script,
}
impl GenericFamily {
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Serif, Self::SansSerif, Self::Monospace, Self::Script]
            .into_iter()
            .find(|generic| s.eq_ignore_ascii_case(generic.name()))
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Serif => "serif",
            Self::SansSerif => "sans-serif",
            Self::Monospace => "monospace",
            Self::Script => "script",
        }
    }
}
pub struct FontMatch<'a> {
    pub face: &'a Arc<Face>,
    pub notes: Vec<Note>,
}

/// Host-owned settings, never authored document state. Empty settings preserve
/// legacy matching. Family names are literal declaration aliases, not chains.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontSettings {
    pub generics: std::collections::BTreeMap<GenericFamily, String>,
    pub fallbacks: Vec<String>,
    pub synthetic_oblique: bool,
}
impl FontSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self) -> Result<(), Note> {
        if self.fallbacks.len() > 64
            || self
                .generics
                .values()
                .chain(&self.fallbacks)
                .any(|name| name.trim().is_empty() || name.len() > 1024)
        {
            return Err(Note::error(
                codes::CONFIG,
                "font settings require at most 64 fallback families and nonempty names of at most 1024 bytes",
            ));
        }
        Ok(())
    }
}

pub(crate) fn face_hash(bytes: &[u8], index: u32) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    // Keep existing identities for standalone fonts and collection face zero.
    if index != 0 {
        digest.update(b"reprise-face-index");
        digest.update(index.to_le_bytes());
    }
    digest
        .finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn bundled() -> &'static [Arc<Face>; 4] {
    static FACES: OnceLock<[Arc<Face>; 4]> = OnceLock::new();
    FACES.get_or_init(|| {
        [
            include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf").as_slice(),
            include_bytes!("../../../fixtures/fonts/SourceSans3-Regular.otf").as_slice(),
            include_bytes!("../../../fixtures/fonts/SourceCodePro-Regular.otf").as_slice(),
            include_bytes!("../../../fixtures/fonts/DancingScript-wght.ttf").as_slice(),
        ]
        .map(|bytes| {
            Arc::new(Face::from_bytes(bytes).expect("pinned bundled font verified by tests"))
        })
    })
}
impl FontStore {
    pub fn settings(&self) -> &FontSettings {
        &self.settings
    }
    /// Atomic replacement; declarations may arrive before or after settings.
    pub fn set_settings(&mut self, settings: FontSettings) -> Result<(), Note> {
        settings.validate()?;
        self.settings = settings;
        Ok(())
    }
    pub fn coverage_fallback(&self) -> bool {
        !self.settings.generics.is_empty() || !self.settings.fallbacks.is_empty()
    }
    /// Fixed shear x += y / 4 in design coordinates (y up), about each glyph
    /// origin. The exact quarter shear is representable in integer 16.16.
    pub fn synthesizes(&self, face: &Face, wanted: Descriptors) -> bool {
        self.settings.synthetic_oblique
            && wanted.style != FontStyle::Normal
            && face.declaration().descriptors.style == FontStyle::Normal
            && !self.faces.values().any(|candidate| {
                candidate
                    .id()
                    .family
                    .eq_ignore_ascii_case(&face.id().family)
                    && candidate.declaration().descriptors.style != FontStyle::Normal
            })
    }
    pub fn match_notes(&self, face: &Face, wanted: Descriptors, family: &str) -> Vec<Note> {
        let actual = face.declaration().descriptors;
        let synthetic = self.synthesizes(face, wanted);
        let mut notes = Vec::new();
        if synthetic {
            notes.push(Note::info(
                codes::SYNTHETIC,
                format!("synthetic oblique (x += y/4) in {family:?}"),
            ));
        }
        if actual.weight != wanted.weight
            || actual.stretch != wanted.stretch
            || (actual.style != wanted.style && !synthetic)
        {
            notes.push(Note::warning(
                codes::NEAREST,
                format!("requested {wanted:?} in {family:?}; used {actual:?}"),
            ));
        }
        notes
    }
    pub fn register(
        &mut self,
        bytes: impl Into<Arc<[u8]>>,
        declaration: FontDeclaration,
    ) -> Result<FaceId, FontError> {
        Ok(self.add(Face::declared(bytes, Some(declaration))?))
    }
    pub fn generic(&self, generic: GenericFamily) -> &Arc<Face> {
        if let Some(face) = self
            .settings
            .generics
            .get(&generic)
            .and_then(|family| self.match_family(family, Descriptors::default()))
        {
            return face.face;
        }
        if let Some(face) = self
            .defaults
            .get(&generic)
            .and_then(|id| self.faces.get(id))
        {
            return face;
        }
        self.builtin_generic(generic)
    }
    pub fn builtin_generic(&self, generic: GenericFamily) -> &Arc<Face> {
        match generic {
            GenericFamily::Serif => &bundled()[0],
            GenericFamily::SansSerif => &bundled()[1],
            GenericFamily::Monospace => &bundled()[2],
            GenericFamily::Script => &bundled()[3],
        }
    }
    /// Overrides are pinned IDs of registered faces; unavailable overrides are refused atomically.
    pub fn set_generic(&mut self, generic: GenericFamily, face: FaceId) -> Result<(), FontError> {
        self.get(&face)?;
        // Builtin identities are accepted too. Store the face so the configured map is total.
        let value = self.get(&face)?.clone();
        self.faces.entry(face.clone()).or_insert(value);
        self.defaults.insert(generic, face);
        Ok(())
    }
    pub fn generic_ids(&self) -> Vec<(GenericFamily, FaceId)> {
        [
            GenericFamily::Serif,
            GenericFamily::SansSerif,
            GenericFamily::Monospace,
            GenericFamily::Script,
        ]
        .into_iter()
        .map(|g| (g, self.generic(g).id().clone()))
        .collect()
    }
    pub fn match_generic(&self, generic: GenericFamily, wanted: Descriptors) -> FontMatch<'_> {
        if let Some(family) = self.settings.generics.get(&generic) {
            if let Some(matched) = self.match_family(family, wanted) {
                return matched;
            }
            let face = self.builtin_generic(generic);
            let mut notes = self.match_notes(face, wanted, generic.name());
            notes.push(Note::warning(
                Code::new("font.fallback"),
                format!(
                    "configured family {family:?} unavailable; used built-in {}",
                    generic.name()
                ),
            ));
            return FontMatch { face, notes };
        }
        let base = self.generic(generic);
        let face = self
            .faces
            .values()
            .filter(|f| f.id().family.eq_ignore_ascii_case(&base.id().family))
            .chain(std::iter::once(base))
            .min_by_key(|f| {
                let d = f.declaration().descriptors;
                (
                    stretch_rank(d.stretch, wanted.stretch),
                    style_rank(d.style, wanted.style),
                    weight_rank(d.weight, wanted.weight),
                    f.id().clone(),
                )
            })
            .unwrap_or(base);
        let notes = if self.settings.synthetic_oblique {
            self.match_notes(face, wanted, generic.name())
        } else if face.declaration().descriptors == wanted {
            Vec::new()
        } else {
            vec![Note::warning(
                codes::NEAREST,
                format!(
                    "requested {wanted:?} in {}; used {:?}",
                    generic.name(),
                    face.declaration().descriptors
                ),
            )]
        };
        FontMatch { face, notes }
    }
    /// CSS order: stretch first, then style, then weight. Directional stretch
    /// preference follows CSS; weight 400..500 searches toward 500 before lower
    /// weights, then above 500. Other weights search lower first below 400 and
    /// higher first above 500. Final ties use FaceId, never registration order.
    pub fn match_family(&self, family: &str, wanted: Descriptors) -> Option<FontMatch<'_>> {
        let face = self
            .faces
            .values()
            .filter(|f| f.id().family.eq_ignore_ascii_case(family))
            .min_by_key(|f| {
                let d = f.declaration().descriptors;
                (
                    stretch_rank(d.stretch, wanted.stretch),
                    style_rank(d.style, wanted.style),
                    weight_rank(d.weight, wanted.weight),
                    f.id().clone(),
                )
            })?;
        let notes = if self.settings.synthetic_oblique {
            self.match_notes(face, wanted, family)
        } else if face.declaration().descriptors == wanted {
            Vec::new()
        } else {
            vec![Note::warning(
                codes::NEAREST,
                format!(
                    "requested {wanted:?} in {family:?}; used {:?}",
                    face.declaration().descriptors
                ),
            )]
        };
        Some(FontMatch { face, notes })
    }
}
fn stretch_rank(actual: u16, wanted: u16) -> (u8, u16) {
    let preferred = if wanted <= 1000 {
        actual <= wanted
    } else {
        actual >= wanted
    };
    (u8::from(!preferred), actual.abs_diff(wanted))
}
fn style_rank(actual: FontStyle, wanted: FontStyle) -> u8 {
    if actual == wanted {
        return 0;
    }
    match (wanted, actual) {
        (FontStyle::Normal, FontStyle::Oblique)
        | (FontStyle::Italic, FontStyle::Oblique)
        | (FontStyle::Oblique, FontStyle::Italic) => 1,
        _ => 2,
    }
}
fn weight_rank(actual: u16, wanted: u16) -> (u8, u16) {
    if (400..=500).contains(&wanted) {
        if (wanted..=500).contains(&actual) {
            (0, actual.abs_diff(wanted))
        } else if actual < wanted {
            (1, wanted.abs_diff(actual))
        } else {
            (2, actual.abs_diff(500))
        }
    } else {
        let preferred = if wanted < 400 {
            actual <= wanted
        } else {
            actual >= wanted
        };
        (u8::from(!preferred), actual.abs_diff(wanted))
    }
}

#[cfg(test)]
mod tests;
