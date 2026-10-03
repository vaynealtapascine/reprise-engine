//! SVG backend. Glyphs are drawn from their outlines, so the output needs no
//! fonts installed and looks the same everywhere.

use std::collections::BTreeSet;
use std::fmt::Write;

use reprise_font::{FontError, FontStore, PathCmd};
use reprise_geom::{Length, Matrix};

use crate::{Color, DisplayList, GlyphRun, Item, Path, Segment, Stroke};

fn pt(l: Length) -> f32 {
    l.to_pt_f32()
}

struct Svg<'a> {
    fonts: &'a FontStore,
    defs: String,
    body: String,
    defined: BTreeSet<String>,
    clips: usize,
}

pub fn render(list: &DisplayList, fonts: &FontStore) -> Result<String, FontError> {
    let mut svg = Svg {
        fonts,
        defs: String::new(),
        body: String::new(),
        defined: BTreeSet::new(),
        clips: 0,
    };
    svg.items(&list.items)?;
    let (w, h) = (pt(list.width), pt(list.height));
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}pt" height="{h}pt" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#fff"/><defs>{}</defs>{}</svg>"##,
        svg.defs, svg.body
    ))
}

impl Svg<'_> {
    fn items(&mut self, items: &[Item]) -> Result<(), FontError> {
        for item in items {
            match item {
                Item::Glyphs(run) => self.glyphs(run)?,
                Item::Path {
                    path, fill, stroke, ..
                } => {
                    let _ = write!(
                        self.body,
                        r#"<path d="{}" {} {}/>"#,
                        path_data(path),
                        fill_attrs(*fill),
                        stroke_attrs(*stroke)
                    );
                }
                Item::Group {
                    transform,
                    clip,
                    items,
                } => {
                    let clip_attr = match clip {
                        Some(path) => {
                            self.clips += 1;
                            let id = format!("clip{}", self.clips);
                            let _ = write!(
                                self.defs,
                                r#"<clipPath id="{id}"><path d="{}"/></clipPath>"#,
                                path_data(path)
                            );
                            format!(r#" clip-path="url(#{id})""#)
                        }
                        None => String::new(),
                    };
                    // The clip is in the children's space, so it goes on an
                    // inner group, inside the transform.
                    let _ = write!(
                        self.body,
                        r#"<g transform="{}"><g{clip_attr}>"#,
                        matrix(transform)
                    );
                    self.items(items)?;
                    self.body.push_str("</g></g>");
                }
            }
        }
        Ok(())
    }

    fn glyphs(&mut self, run: &GlyphRun) -> Result<(), FontError> {
        let font = self.fonts.get(&run.face)?;
        let scale = pt(run.size) / font.metrics().units_per_em as f32;
        let _ = write!(
            self.body,
            r#"<g fill="{}" fill-opacity="{}">"#,
            run.color.hex(),
            run.color.alpha()
        );
        for g in &run.glyphs {
            let key = format!("g{}-{}", &run.face.hash[..8], g.id);
            if self.defined.insert(key.clone()) {
                let _ = write!(
                    self.defs,
                    r#"<path id="{key}" d="{}"/>"#,
                    outline_data(&font.outline(g.id))
                );
            }
            let _ = write!(
                self.body,
                r##"<use href="#{key}" transform="translate({} {}) scale({scale} {})"/>"##,
                pt(g.x),
                pt(g.y),
                -scale
            );
        }
        self.body.push_str("</g>");
        Ok(())
    }
}

fn matrix(m: &Matrix) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        m.xx.to_f32(),
        m.yx.to_f32(),
        m.xy.to_f32(),
        m.yy.to_f32(),
        pt(m.tx),
        pt(m.ty)
    )
}

fn fill_attrs(fill: Option<Color>) -> String {
    match fill {
        Some(c) => format!(r#"fill="{}" fill-opacity="{}""#, c.hex(), c.alpha()),
        None => r#"fill="none""#.into(),
    }
}

fn stroke_attrs(stroke: Option<Stroke>) -> String {
    match stroke {
        Some(s) => format!(
            r#"stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
            s.color.hex(),
            s.color.alpha(),
            pt(s.width)
        ),
        None => String::new(),
    }
}

fn path_data(path: &Path) -> String {
    let mut d = String::new();
    for s in &path.0 {
        let _ = match *s {
            Segment::Move(p) => write!(d, "M{} {}", pt(p.x), pt(p.y)),
            Segment::Line(p) => write!(d, "L{} {}", pt(p.x), pt(p.y)),
            Segment::Quad(c, p) => write!(d, "Q{} {} {} {}", pt(c.x), pt(c.y), pt(p.x), pt(p.y)),
            Segment::Cubic(a, b, p) => write!(
                d,
                "C{} {} {} {} {} {}",
                pt(a.x),
                pt(a.y),
                pt(b.x),
                pt(b.y),
                pt(p.x),
                pt(p.y)
            ),
            Segment::Close => write!(d, "Z"),
        };
    }
    d
}

fn outline_data(cmds: &[PathCmd]) -> String {
    let mut d = String::new();
    for c in cmds {
        let _ = match *c {
            PathCmd::Move(x, y) => write!(d, "M{x} {y}"),
            PathCmd::Line(x, y) => write!(d, "L{x} {y}"),
            PathCmd::Quad(a, b, x, y) => write!(d, "Q{a} {b} {x} {y}"),
            PathCmd::Cubic(a, b, c, e, x, y) => write!(d, "C{a} {b} {c} {e} {x} {y}"),
            PathCmd::Close => write!(d, "Z"),
        };
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    #[test]
    fn renders_groups_with_transforms_and_clips() {
        let (list, fonts) = sample::list();
        let svg = render(&list, &fonts).unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("<use href=\"#g"), "glyphs are drawn");
        assert!(
            svg.contains(r#"<g transform="matrix(0 1 -1 0 95 20)"><g clip-path="url(#clip1)">"#)
        );
        assert!(svg.contains(r#"<clipPath id="clip1">"#));
        assert_eq!(render(&list, &fonts).unwrap(), svg, "deterministic");
    }
}
