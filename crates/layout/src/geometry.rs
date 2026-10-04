//! Integer frame geometry (20, 38). Editing uses the resulting ordinary
//! frame transforms, including every spiral strip: no hidden glyph transforms.
use crate::template::ResolvedFrame;
use crate::{Diagnostic, Subject};
use reprise_diag::{Code, Severity};
use reprise_doc::{Dim, FrameTemplate, Rotation, WritingMode};
use reprise_geom::{Length, Matrix, sin_cos_millidegrees};

pub const TRANSFORM_UNUSABLE: Code = Code::new("layout.transform-unusable");
pub const PATH_INVALID: Code = Code::new("layout.path-invalid");
pub const PATH_LIMIT: Code = Code::new("layout.path-limit");
/// Maximum tangent strips per authored path. Excess strips are omitted.
pub const MAX_PATH_SEGMENTS: u32 = 1024;

fn note(notes: &mut Vec<Diagnostic>, code: Code, frame: &FrameTemplate, why: &str) {
    notes.push(Diagnostic::new(
        Severity::Warning,
        code,
        Subject::Document,
        format!("frame `{}`: {why}", frame.name),
    ));
}

pub(crate) fn resolve_frames(
    frame: &FrameTemplate,
    width: Length,
    depth: Length,
    at: &impl Fn(&Dim) -> Length,
    notes: &mut Vec<Diagnostic>,
) -> Vec<ResolvedFrame> {
    let authored = &frame.transform;
    let rotation = match authored.rotation {
        Rotation::None => Some(Matrix::IDENTITY),
        Rotation::Quarter(n) => Some(Matrix::rotate_quarter(n)),
        Rotation::Direction { dx, dy } => Matrix::rotate_toward(dx, dy),
        Rotation::Matrix { xx, yx, xy, yy } => Some(Matrix {
            xx,
            yx,
            xy,
            yy,
            ..Matrix::IDENTITY
        }),
    };
    let mut transform = Matrix::IDENTITY;
    if authored.mirror_x {
        transform = transform.then(&Matrix::mirror_x());
    }
    if authored.mirror_y {
        transform = transform.then(&Matrix::mirror_y());
    }
    let transform = rotation.map(|r| {
        transform
            .then(&r)
            .about(at(&authored.origin_x), at(&authored.origin_y))
    });
    let transform = match transform {
        Some(m) if m.has_usable_inverse() => m,
        _ => {
            note(
                notes,
                TRANSFORM_UNUSABLE,
                frame,
                "transform has no usable inverse; identity is used",
            );
            Matrix::IDENTITY
        }
    };
    let (logical_width, logical_depth, mode) = match frame.writing_mode {
        WritingMode::HorizontalTb => (width, depth, Matrix::IDENTITY),
        WritingMode::VerticalRl => (
            depth,
            width,
            Matrix::rotate_quarter(1).then(&Matrix::translate(width, Length::ZERO)),
        ),
        WritingMode::VerticalLr => (
            depth,
            width,
            Matrix::rotate_quarter(3).then(&Matrix::translate(Length::ZERO, depth)),
        ),
    };
    let ordinary = || ResolvedFrame {
        name: frame.name.clone(),
        role: frame.role.clone(),
        x: at(&frame.x),
        y: at(&frame.y),
        width: logical_width,
        depth: logical_depth,
        transform: mode.then(&transform),
    };
    let Some(path) = &frame.path else {
        return vec![ordinary()];
    };
    if frame.writing_mode != WritingMode::HorizontalTb {
        note(
            notes,
            PATH_INVALID,
            frame,
            "path strips require horizontal logical axes; ordinary frame is used",
        );
        return vec![ordinary()];
    }
    let radius = at(&path.radius);
    let growth = at(&path.growth);
    if radius < Length::ZERO || path.segments == 0 || path.sweep_millidegrees == 0 {
        note(
            notes,
            PATH_INVALID,
            frame,
            "negative radius, zero samples or zero sweep; ordinary frame is used",
        );
        return vec![ordinary()];
    }
    let count = path.segments.min(MAX_PATH_SEGMENTS);
    if count != path.segments {
        note(
            notes,
            PATH_LIMIT,
            frame,
            "path exceeds 1024 strips; only its first 1024 strips are used",
        );
    }
    let sample = |i: u32| {
        let offset = rounded(
            path.sweep_millidegrees as i128 * i as i128,
            path.segments as i128,
        );
        let angle = (path.start_millidegrees as i128 + offset).rem_euclid(360_000) as i64;
        let radial = rounded(growth.0 as i128 * offset.abs(), 360_000);
        let r =
            Length((radius.0 as i128 + radial).clamp(i32::MIN as i128, i32::MAX as i128) as i32);
        let (c, s) = sin_cos_millidegrees(angle);
        (r, r.mul_ratio(c, 1 << 30), r.mul_ratio(s, 1 << 30))
    };
    let mut out = Vec::new();
    let mut invalid = false;
    for i in 0..count {
        let (r0, x0, y0) = sample(i);
        let (r1, x1, y1) = sample(i + 1);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let Some(tangent) = Matrix::rotate_toward(dx.0, dy.0) else {
            invalid = true;
            continue;
        };
        if r0 < Length::ZERO || r1 < Length::ZERO {
            invalid = true;
            continue;
        }
        // Each strip is an ordinary frame. Its start corner follows the
        // sampled path; baseline offset stays explicit in LineLayout.
        let m = tangent.then(&Matrix::translate(x0, y0)).then(&transform);
        out.push(ResolvedFrame {
            name: format!("{}#{}", frame.name, i),
            role: frame.role.clone(),
            x: at(&frame.x),
            y: at(&frame.y),
            width: Length::hypot(dx, dy),
            depth,
            transform: m,
        });
    }
    if invalid {
        note(
            notes,
            PATH_INVALID,
            frame,
            "negative radii or coincident samples were skipped",
        );
    }
    if out.is_empty() {
        vec![ordinary()]
    } else {
        out
    }
}

fn rounded(n: i128, d: i128) -> i128 {
    let q = (n.abs() + d / 2) / d;
    if n < 0 { -q } else { q }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_doc::{FrameRole, Medium, Spiral};
    fn frame() -> FrameTemplate {
        FrameTemplate::new(
            "f",
            FrameRole::Flow("main".into()),
            (Dim::pt(10), Dim::pt(20)),
            (Dim::pt(100), Dim::pt(200)),
        )
    }
    fn resolve(f: &FrameTemplate) -> (Vec<ResolvedFrame>, Vec<Diagnostic>) {
        let mut notes = Vec::new();
        let at = |d: &Dim| {
            d.resolve(&Medium::new(Length::MAX, Length::MIN), None)
                .unwrap_or(Length::ZERO)
        };
        let frames = resolve_frames(
            f,
            Length::from_pt(100),
            Length::from_pt(200),
            &at,
            &mut notes,
        );
        (frames, notes)
    }
    #[test]
    fn modes_use_physical_dimensions_and_exact_corner_maps() {
        for (mode, start, end) in [
            (WritingMode::HorizontalTb, (10, 20), (110, 220)),
            (WritingMode::VerticalRl, (110, 20), (10, 220)),
            (WritingMode::VerticalLr, (10, 220), (110, 20)),
        ] {
            let mut f = frame();
            f.writing_mode = mode;
            let (frames, notes) = resolve(&f);
            let r = &frames[0];
            assert!(notes.is_empty());
            let m = r.to_page();
            assert_eq!(
                m.apply(reprise_geom::Point::origin()),
                reprise_geom::Point::new(Length::from_pt(start.0), Length::from_pt(start.1))
            );
            assert_eq!(
                m.apply(reprise_geom::Point::new(r.width, r.depth)),
                reprise_geom::Point::new(Length::from_pt(end.0), Length::from_pt(end.1))
            );
            let bounds = m.bounds(&r.rect());
            assert_eq!(
                (bounds.width, bounds.height),
                (Length::from_pt(100), Length::from_pt(200))
            );
        }
    }
    #[test]
    fn invalid_transforms_keep_text_and_report_the_fallback() {
        for rotation in [
            Rotation::Direction { dx: 0, dy: 0 },
            Rotation::Matrix {
                xx: reprise_geom::Fixed::ZERO,
                yx: reprise_geom::Fixed::ZERO,
                xy: reprise_geom::Fixed::ZERO,
                yy: reprise_geom::Fixed::ONE,
            },
            Rotation::Matrix {
                xx: reprise_geom::Fixed(1),
                yx: reprise_geom::Fixed::ZERO,
                xy: reprise_geom::Fixed::ZERO,
                yy: reprise_geom::Fixed::ONE,
            },
        ] {
            let mut f = frame();
            f.transform.rotation = rotation;
            let (frames, notes) = resolve(&f);
            assert_eq!(frames[0].transform, Matrix::IDENTITY);
            assert_eq!(notes[0].code, TRANSFORM_UNUSABLE);
        }
    }
    #[test]
    fn path_expansion_is_bounded_even_at_integer_limits() {
        let mut f = frame();
        f.path = Some(Spiral {
            radius: Dim::Pt(Length::MAX),
            growth: Dim::Pt(Length::MAX),
            start_millidegrees: i64::MIN,
            sweep_millidegrees: i64::MAX,
            segments: u32::MAX,
        });
        let (frames, notes) = resolve(&f);
        assert!(frames.len() <= MAX_PATH_SEGMENTS as usize);
        assert!(notes.iter().any(|d| d.code == PATH_LIMIT));
        for frame in frames {
            assert!(frame.transform.has_usable_inverse());
        }
        for (segments, sweep, radius) in [(0, 1, 10), (10, 0, 10), (10, 90_000, -1)] {
            f.path = Some(Spiral {
                radius: Dim::pt(radius),
                growth: Dim::pt(1),
                start_millidegrees: 0,
                sweep_millidegrees: sweep,
                segments,
            });
            let (frames, notes) = resolve(&f);
            assert_eq!(frames.len(), 1);
            assert_eq!(notes[0].code, PATH_INVALID);
        }
    }
    #[test]
    fn negative_sweeps_reverse_tangents_and_shrinking_spirals_report() {
        let mut f = frame();
        f.path = Some(Spiral {
            radius: Dim::pt(30),
            growth: Dim::pt(-40),
            start_millidegrees: 0,
            sweep_millidegrees: -720_000,
            segments: 64,
        });
        let (frames, notes) = resolve(&f);
        assert!(frames.len() < 64);
        assert!(notes.iter().any(|d| d.code == PATH_INVALID));
        assert!(frames[0].transform.yx < reprise_geom::Fixed::ZERO);
    }
}
