//! Bounded interpretation of physical geometry and exact inline apportionment.
use crate::template::ResolvedFrame;
use crate::{Diagnostic, Subject};
use reprise_diag::{Code, Severity};
use reprise_doc::page_setup::{MAX_PAGE_LENGTH, PageDimensions};
use reprise_doc::sections::{SectionId, SectionProperties};
use reprise_doc::{Dim, FrameRole, FrameTemplate, MAIN_FLOW, WritingMode};
use reprise_geom::{Length, PageSpace, Point, Rect};
use std::collections::BTreeMap;

pub(super) const MIN_COLUMN: Length = Length::from_pt(1);
pub(super) const MAX_COLUMNS: usize = 64;

#[derive(Clone, Debug)]
pub(super) struct Warnings {
    section: SectionId,
    entries: BTreeMap<(String, &'static str), Diagnostic>,
}
impl Warnings {
    pub fn new(section: SectionId) -> Self {
        Self {
            section,
            entries: BTreeMap::new(),
        }
    }
    pub fn warn(&mut self, key: &str, code: &'static str, why: &str) {
        let subject = match self.section {
            SectionId::First => Subject::Document,
            SectionId::Block(node) => Subject::Node(node),
        };
        self.entries.entry((key.into(), code)).or_insert_with(|| {
            Diagnostic::new(
                Severity::Warning,
                Code::new(code),
                subject,
                format!("{key}: {why}"),
            )
        });
    }
    pub fn finish(self) -> Vec<Diagnostic> {
        self.entries.into_values().collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub dimensions: PageDimensions,
    pub provenance: BTreeMap<String, String>,
    pub header_distance: Length,
    pub footer_distance: Length,
    pub rtl: bool,
    pub writing_mode: WritingMode,
}
impl Geometry {
    pub fn inline_extent(&self) -> Length {
        let d = self.dimensions;
        if self.writing_mode == WritingMode::HorizontalTb {
            d.width - d.left - d.right
        } else {
            d.height - d.top - d.bottom
        }
    }
    pub fn block_extent(&self) -> Length {
        let d = self.dimensions;
        if self.writing_mode == WritingMode::HorizontalTb {
            d.height - d.top - d.bottom
        } else {
            d.width - d.left - d.right
        }
    }
    pub fn body_rect(&self) -> Rect<PageSpace> {
        let d = self.dimensions;
        Rect::new(
            Point::new(d.left, d.top),
            d.width - d.left - d.right,
            d.height - d.top - d.bottom,
        )
    }
}

fn length(
    own: &SectionProperties,
    key: &str,
    positive: bool,
    notes: &mut Warnings,
) -> Option<Length> {
    let value = own.get(key)?;
    let minimum = if positive { 1 } else { 0 };
    let result = value
        .as_i64()
        .filter(|v| *v >= minimum && *v <= i64::from(MAX_PAGE_LENGTH.0))
        .and_then(|v| i32::try_from(v).ok())
        .map(Length);
    if result.is_none() {
        notes.warn(
            key,
            "section.property",
            "invalid length; the default is used",
        );
    }
    result
}
fn token<'a>(
    own: &'a SectionProperties,
    key: &str,
    accepted: &[&str],
    notes: &mut Warnings,
) -> Option<&'a str> {
    let value = own.get(key)?;
    let result = value.as_str().filter(|s| accepted.contains(s));
    if result.is_none() {
        notes.warn(
            key,
            "section.property",
            "unknown or unreadable value; the default is used",
        );
    }
    result
}

pub(super) fn resolve(
    own: &SectionProperties,
    base: PageDimensions,
    fallback: PageDimensions,
    base_mode: WritingMode,
    notes: &mut Warnings,
) -> Geometry {
    let mut dimensions = base;
    let mut provenance = BTreeMap::new();
    let keys = ["width", "height", "top", "right", "bottom", "left"];
    let mut values = [
        base.width,
        base.height,
        base.top,
        base.right,
        base.bottom,
        base.left,
    ];
    for (i, key) in keys.iter().enumerate() {
        if let Some(value) = length(own, key, i < 2, notes) {
            values[i] = value;
            provenance.insert((*key).into(), "section".into());
        } else {
            provenance.insert((*key).into(), "default".into());
        }
    }
    dimensions.width = values[0];
    dimensions.height = values[1];
    dimensions.top = values[2];
    dimensions.right = values[3];
    dimensions.bottom = values[4];
    dimensions.left = values[5];
    if let Some(orientation) = token(own, "orientation", &["portrait", "landscape"], notes)
        && ((orientation == "portrait" && dimensions.width > dimensions.height)
            || (orientation == "landscape" && dimensions.width < dimensions.height))
    {
        std::mem::swap(&mut dimensions.width, &mut dimensions.height);
        provenance.insert("width".into(), "section".into());
        provenance.insert("height".into(), "section".into());
    }
    if (keys.iter().any(|k| own.contains_key(*k)) || own.contains_key("orientation"))
        && dimensions.validate().is_err()
    {
        dimensions = fallback;
        for source in provenance.values_mut() {
            *source = "default".into();
        }
        notes.warn(
            "geometry",
            "section.geometry",
            "merged dimensions have no positive text area; the whole document default is used",
        );
    }
    let writing_mode = match token(
        own,
        "writing-mode",
        &["horizontal-tb", "vertical-rl", "vertical-lr"],
        notes,
    ) {
        Some("horizontal-tb") => WritingMode::HorizontalTb,
        Some("vertical-rl") => WritingMode::VerticalRl,
        Some("vertical-lr") => WritingMode::VerticalLr,
        _ => base_mode,
    };
    Geometry {
        dimensions,
        provenance,
        writing_mode,
        header_distance: length(own, "header.distance", false, notes)
            .unwrap_or(Length::from_pt(36)),
        footer_distance: length(own, "footer.distance", false, notes)
            .unwrap_or(Length::from_pt(36)),
        rtl: token(own, "direction", &["ltr", "rtl"], notes) == Some("rtl"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Columns {
    pub widths: Vec<Length>,
    pub gap: Length,
    pub rule: Length,
    pub thread: Vec<usize>,
}
fn weights(own: &SectionProperties, notes: &mut Warnings) -> Option<Vec<i64>> {
    let value = own.get("columns.widths")?;
    let result = value.as_str().and_then(|s| {
        if s.is_empty() {
            return Some(Vec::new());
        }
        if s.len() > 1024 {
            return None;
        }
        let mut result = Vec::new();
        for part in s.split(',') {
            if result.len() >= MAX_COLUMNS {
                return None;
            }
            let value = part.trim().parse::<i64>().ok()?;
            if !(0..=i64::from(MAX_PAGE_LENGTH.0)).contains(&value) {
                return None;
            }
            result.push(value);
        }
        Some(result)
    });
    if result.is_none() {
        notes.warn(
            "columns.widths",
            "section.property",
            "invalid or excessive weight CSV; equal columns are used",
        );
    }
    result.filter(|v| !v.is_empty())
}

pub(super) fn columns(
    own: &SectionProperties,
    inline: Length,
    default_count: usize,
    notes: &mut Warnings,
) -> Option<Columns> {
    if !own.keys().any(|k| {
        k.starts_with("columns.")
            && matches!(
                k.as_str(),
                "columns.count" | "columns.gap" | "columns.widths" | "columns.rule"
            )
    }) {
        return None;
    }
    let mut count = default_count.clamp(1, MAX_COLUMNS);
    if let Some(value) = own.get("columns.count") {
        match value.as_i64() {
            Some(n) if n > i64::try_from(MAX_COLUMNS).unwrap_or(64) => {
                count = MAX_COLUMNS;
                notes.warn(
                    "columns.count",
                    "section.columns-limit",
                    "column count is clamped to 64",
                );
            }
            Some(n) if n > 0 => count = usize::try_from(n).unwrap_or(MAX_COLUMNS),
            _ => notes.warn(
                "columns.count",
                "section.property",
                "invalid count; the template count is used",
            ),
        }
    }
    let weights = weights(own, notes);
    if let Some(weights) = &weights {
        if own.contains_key("columns.count") && count != weights.len() {
            notes.warn(
                "columns.count",
                "section.property",
                "valid widths set the count instead",
            );
        }
        count = weights.len();
    }
    let gap = length(own, "columns.gap", false, notes).unwrap_or(Length::from_pt(18));
    let mut rule = length(own, "columns.rule", false, notes).unwrap_or(Length::ZERO);
    if rule > gap {
        rule = gap;
        notes.warn(
            "columns.rule",
            "section.property",
            "rule thickness is clamped to the gap",
        );
    }
    let fallback = || Columns {
        widths: vec![inline.max(Length::ZERO)],
        gap: Length::ZERO,
        rule: Length::ZERO,
        thread: vec![0],
    };
    let apportioned = (|| {
        let n = i64::try_from(count).ok()?;
        let gaps = n.checked_sub(1)?.checked_mul(i64::from(gap.0))?;
        let total = i64::from(inline.0).checked_sub(gaps)?;
        if total < n.checked_mul(i64::from(MIN_COLUMN.0))? {
            return None;
        }
        let sum = match weights.as_ref() {
            Some(weights) => Some(
                weights
                    .iter()
                    .try_fold(0i64, |sum, v| sum.checked_add(*v))?,
            ),
            None => None,
        };
        let mut widths = Vec::with_capacity(count);
        if let (Some(weights), Some(sum)) = (&weights, sum.filter(|s| *s > 0)) {
            for weight in weights {
                widths.push(total.checked_mul(*weight)?.checked_div(sum)?);
            }
        } else {
            if weights.is_some() {
                notes.warn(
                    "columns.widths",
                    "section.property",
                    "all weights are zero; equal columns are used",
                );
            }
            widths.resize(count, total.checked_div(n)?);
        }
        let allocated = widths.iter().try_fold(0i64, |sum, v| sum.checked_add(*v))?;
        let remainder = usize::try_from(total.checked_sub(allocated)?).ok()?;
        if remainder >= count {
            return None;
        }
        for width in widths.iter_mut().take(remainder) {
            *width = width.checked_add(1)?;
        }
        widths
            .into_iter()
            .map(|v| i32::try_from(v).ok().map(Length))
            .collect::<Option<Vec<_>>>()
    })();
    let Some(widths) = apportioned else {
        notes.warn(
            "columns",
            "section.columns-fit",
            "columns and gaps cannot fit; one column is used",
        );
        return Some(fallback());
    };
    let thread: Vec<_> = widths
        .iter()
        .enumerate()
        .filter_map(|(i, width)| (*width >= MIN_COLUMN).then_some(i))
        .collect();
    if thread.len() < widths.len() {
        notes.warn(
            "columns.widths",
            "section.column-narrow",
            "columns narrower than one point are skipped by the thread",
        );
    }
    Some(if thread.is_empty() {
        fallback()
    } else {
        Columns {
            widths,
            gap,
            rule,
            thread,
        }
    })
}

pub(super) fn frames(
    geometry: &Geometry,
    columns: &Columns,
) -> (Vec<ResolvedFrame>, Vec<Rect<PageSpace>>) {
    let d = geometry.dimensions;
    let horizontal = geometry.writing_mode == WritingMode::HorizontalTb;
    let mut offset = if horizontal && geometry.rtl {
        geometry.inline_extent()
    } else {
        Length::ZERO
    };
    let mut frames = Vec::new();
    let mut rules = Vec::new();
    for (i, width) in columns.widths.iter().enumerate() {
        if horizontal && geometry.rtl {
            offset -= *width;
        }
        let (x, y, physical_width, physical_height) = if horizontal {
            (d.left + offset, d.top, *width, geometry.block_extent())
        } else {
            (d.left, d.top + offset, geometry.block_extent(), *width)
        };
        let mut frame = FrameTemplate::new(
            format!("main.column.{i}"),
            FrameRole::Flow(MAIN_FLOW.into()),
            (Dim::Pt(x), Dim::Pt(y)),
            (Dim::Pt(physical_width), Dim::Pt(physical_height)),
        );
        frame.writing_mode = geometry.writing_mode;
        frames.extend(crate::geometry::resolve_frames(
            &frame,
            physical_width,
            physical_height,
            &|value| {
                value
                    .resolve(
                        &reprise_doc::Medium::new(d.width, d.height),
                        Some((d.width, d.height)),
                    )
                    .unwrap_or_default()
            },
            &mut Vec::new(),
        ));
        if i + 1 < columns.widths.len() && columns.rule > Length::ZERO {
            let centered = Length((columns.gap.0 - columns.rule.0) / 2);
            let position = if horizontal && geometry.rtl {
                offset - columns.gap + centered
            } else {
                offset + *width + centered
            };
            rules.push(if horizontal {
                Rect::new(
                    Point::new(d.left + position, d.top),
                    columns.rule,
                    geometry.block_extent(),
                )
            } else {
                Rect::new(
                    Point::new(d.left, d.top + position),
                    geometry.block_extent(),
                    columns.rule,
                )
            });
        }
        if horizontal && geometry.rtl {
            offset -= columns.gap;
        } else {
            offset += *width + columns.gap;
        }
    }
    (frames, rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn own(values: &[(&str, Value)]) -> SectionProperties {
        values
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect()
    }
    fn base() -> PageDimensions {
        PageDimensions {
            width: Length(60_000),
            height: Length(80_000),
            left: Length(4_000),
            right: Length(5_000),
            top: Length(6_000),
            bottom: Length(7_000),
        }
    }
    fn notes() -> Warnings {
        Warnings::new(SectionId::First)
    }

    #[test]
    fn exact_equal_and_weighted_totals_distribute_remainders_in_thread_order() {
        let mut warnings = notes();
        let equal = columns(
            &own(&[("columns.count", json!(3)), ("columns.gap", json!(10))]),
            Length(10_003),
            1,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(equal.widths, [Length(3328), Length(3328), Length(3327)]);
        let props = own(&[
            ("columns.widths", json!("1,2,3")),
            ("columns.gap", json!(10)),
            ("columns.rule", json!(7)),
            ("direction", json!("rtl")),
        ]);
        let columns = columns(&props, Length(10_003), 1, &mut warnings).unwrap();
        assert_eq!(columns.widths, [Length(1664), Length(3328), Length(4991)]);
        assert_eq!(
            columns.widths.iter().map(|v| i64::from(v.0)).sum::<i64>() + 20,
            10_003
        );
        let dims = PageDimensions {
            width: Length(10_003),
            height: Length(10_000),
            top: Length::ZERO,
            right: Length::ZERO,
            bottom: Length::ZERO,
            left: Length::ZERO,
        };
        let geometry = resolve(&props, dims, dims, WritingMode::HorizontalTb, &mut warnings);
        let (frames, rules) = frames(&geometry, &columns);
        assert_eq!(
            frames.iter().map(|f| f.x).collect::<Vec<_>>(),
            [Length(8339), Length(5001), Length::ZERO]
        );
        assert_eq!(rules[0].origin.x, Length(8330));
        assert_eq!(rules[0].width, Length(7));
        assert!(warnings.finish().is_empty());
    }

    #[test]
    fn a_lone_column_key_and_empty_csv_take_the_selected_templates_defaults() {
        let mut warnings = notes();
        let lone = columns(
            &own(&[("columns.gap", json!(10))]),
            Length(100_003),
            1,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(lone.widths, [Length(100_003)]);
        let empty = columns(
            &own(&[("columns.widths", json!(""))]),
            Length(100_003),
            3,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(empty.widths.len(), 3);
        assert_eq!(empty.gap, Length::from_pt(18));
        assert_eq!(
            empty.widths.iter().map(|v| i64::from(v.0)).sum::<i64>() + 2 * i64::from(empty.gap.0),
            100_003
        );
        assert!(warnings.finish().is_empty());
        assert!(columns(&SectionProperties::new(), Length(100_003), 3, &mut notes()).is_none());
    }

    #[test]
    fn bounds_and_narrow_columns_have_deterministic_fallbacks() {
        let mut warnings = notes();
        let props = own(&[("columns.count", json!(1000)), ("columns.gap", json!(0))]);
        let many = columns(&props, MAX_PAGE_LENGTH, 1, &mut warnings).unwrap();
        assert_eq!(many.widths.len(), 64);
        assert!(
            warnings
                .finish()
                .iter()
                .any(|n| n.code == Code::new("section.columns-limit"))
        );
        let mut warnings = notes();
        let zero = columns(
            &own(&[("columns.widths", json!("0,0")), ("columns.gap", json!(0))]),
            Length(10_000),
            1,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(zero.widths, [Length(5000), Length(5000)]);
        assert!(
            warnings
                .finish()
                .iter()
                .any(|n| n.code == Code::new("section.property"))
        );
        let mut warnings = notes();
        let narrow = columns(
            &own(&[("columns.widths", json!("0,1")), ("columns.gap", json!(0))]),
            Length(10_000),
            1,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(narrow.widths, [Length::ZERO, Length(10_000)]);
        assert_eq!(narrow.thread, [1]);
        assert!(
            warnings
                .finish()
                .iter()
                .any(|n| n.code == Code::new("section.column-narrow"))
        );
        let fallback = columns(
            &own(&[
                ("columns.count", json!(64)),
                ("columns.gap", json!(MAX_PAGE_LENGTH.0)),
            ]),
            Length(1),
            1,
            &mut notes(),
        )
        .unwrap();
        assert_eq!(fallback.widths, [Length(1)]);
        assert_eq!(fallback.thread, [0]);
        let huge = own(&[("columns.widths", json!("1,".repeat(10_000)))]);
        assert_eq!(
            columns(&huge, Length(100_003), 1, &mut notes())
                .unwrap()
                .widths
                .len(),
            1
        );
        for invalid in ["-1,2", "1,,2", "9223372036854775807,1", "1,2,"] {
            assert_eq!(
                columns(
                    &own(&[("columns.widths", json!(invalid))]),
                    Length(100_003),
                    1,
                    &mut notes()
                )
                .unwrap()
                .widths
                .len(),
                1
            );
        }
    }

    #[test]
    fn merged_margins_fall_back_whole_and_orientation_keeps_physical_margins() {
        let mut warnings = notes();
        let d = base();
        let bad = own(&[("left", json!(40_000)), ("right", json!(30_000))]);
        assert_eq!(
            resolve(&bad, d, d, WritingMode::HorizontalTb, &mut warnings).dimensions,
            d
        );
        assert!(
            warnings
                .finish()
                .iter()
                .any(|n| n.code == Code::new("section.geometry"))
        );
        let oriented = resolve(
            &own(&[("orientation", json!("landscape"))]),
            d,
            d,
            WritingMode::HorizontalTb,
            &mut notes(),
        );
        assert_eq!(oriented.dimensions.width, d.height);
        assert_eq!(oriented.dimensions.height, d.width);
        assert_eq!(oriented.dimensions.left, d.left);
        assert_eq!(oriented.dimensions.top, d.top);
        for bad in [
            json!(-1),
            json!(0),
            json!(i64::MAX),
            json!(i64::MIN),
            json!("100"),
        ] {
            let mut warnings = notes();
            assert_eq!(
                resolve(
                    &own(&[("width", bad)]),
                    d,
                    d,
                    WritingMode::HorizontalTb,
                    &mut warnings
                )
                .dimensions,
                d
            );
            assert!(
                warnings
                    .finish()
                    .iter()
                    .any(|n| n.code == Code::new("section.property"))
            );
        }
    }

    #[test]
    fn vertical_columns_partition_top_bottom_and_advance_on_the_physical_block_axis() {
        for (mode, origin_x) in [("vertical-rl", 55_000), ("vertical-lr", 4_000)] {
            let props = own(&[
                ("writing-mode", json!(mode)),
                ("columns.count", json!(2)),
                ("columns.rule", json!(4096)),
            ]);
            let geometry = resolve(
                &props,
                base(),
                base(),
                WritingMode::HorizontalTb,
                &mut notes(),
            );
            assert_eq!(geometry.inline_extent(), Length(67_000));
            assert_eq!(geometry.block_extent(), Length(51_000));
            let columns = columns(&props, geometry.inline_extent(), 1, &mut notes()).unwrap();
            assert_eq!(columns.widths, [Length(24_284), Length(24_284)]);
            let (frames, rules) = frames(&geometry, &columns);
            assert_eq!(frames[0].width, Length(24_284));
            assert_eq!(frames[0].depth, Length(51_000));
            assert_eq!(
                frames[0].to_page().apply(Point::origin()),
                Point::new(Length(origin_x), Length(6_000))
            );
            assert_eq!(frames[1].y, Length(48_716));
            assert_eq!(rules[0].origin, Point::new(Length(4_000), Length(37_452)));
            assert_eq!(rules[0].width, Length(51_000));
            assert_eq!(rules[0].height, Length(4096));
        }
    }
}
