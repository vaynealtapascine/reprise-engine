//! Page-owned physical coordinate planes for float constraints. Their geometry
//! is available before body flow and independent of every band's trial depth.
use super::*;
use crate::FrameLayout;
use reprise_geom::{FrameSpace, Point, Transform};

type Group = (String, bool, i32, i32, u8);
#[derive(Clone, Debug, Default)]
pub(crate) struct FloatSpaces(BTreeMap<Group, Vec<FrameLayout>>);

fn mode(mode: WritingMode) -> u8 {
    match mode {
        WritingMode::HorizontalTb => 0,
        WritingMode::VerticalRl => 1,
        WritingMode::VerticalLr => 2,
        WritingMode::SidewaysLr => 3,
    }
}
fn group(section: &Section) -> Group {
    (
        section.template.name.clone(),
        section.template.source == TemplateSource::Document,
        section.template.width.0,
        section.template.height.0,
        mode(section.geometry.writing_mode),
    )
}
pub(crate) fn name(frame: &FrameLayout) -> String {
    let m = frame.to_page.matrix();
    format!(
        "reprise.float.{}.{}.{}.{}.{}",
        mode(frame.writing_mode),
        m.xx.0,
        m.yx.0,
        m.xy.0,
        m.yy.0
    )
}
impl FloatSpaces {
    pub(crate) fn resolve(sections: &[Section], enabled: bool) -> Self {
        if !enabled {
            return Self::default();
        }
        let mut groups: BTreeMap<Group, BTreeMap<String, FrameLayout>> = BTreeMap::new();
        for section in sections {
            let classes = groups.entry(group(section)).or_default();
            for frame in section.template.frames.iter().filter(|f| f.is_main_flow()) {
                let frame = frame.layout(0);
                let name = name(&frame);
                if classes.contains_key(&name) {
                    continue;
                }
                let mut linear = *frame.to_page.matrix();
                linear.tx = Length::ZERO;
                linear.ty = Length::ZERO;
                let transform = Transform::<FrameSpace, PageSpace>::new(linear);
                let Some(inverse) = transform.inverse() else {
                    continue;
                };
                let points = [
                    Point::origin(),
                    Point::new(section.template.width, Length::ZERO),
                    Point::new(Length::ZERO, section.template.height),
                    Point::new(section.template.width, section.template.height),
                ]
                .map(|p| inverse.apply(p));
                let left = points.iter().map(|p| p.x).min().unwrap_or_default();
                let right = points.iter().map(|p| p.x).max().unwrap_or_default();
                let top = points.iter().map(|p| p.y).min().unwrap_or_default();
                let bottom = points.iter().map(|p| p.y).max().unwrap_or_default();
                let (x, y) = linear.apply(left, top);
                linear.tx = x;
                linear.ty = y;
                classes.insert(
                    name.clone(),
                    FrameLayout {
                        name: name.clone(),
                        role: FrameRole::Flow(name),
                        page: 0,
                        to_page: Transform::new(linear),
                        rect: Rect::new(
                            Point::origin(),
                            (right - left).max(Length(1)),
                            (bottom - top).max(Length(1)),
                        ),
                        writing_mode: frame.writing_mode,
                    },
                );
            }
        }
        Self(
            groups
                .into_iter()
                .map(|(key, classes)| (key, classes.into_values().collect()))
                .collect(),
        )
    }
    pub(crate) fn for_section(&self, section: &Section) -> &[FrameLayout] {
        self.0
            .get(&group(section))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}
