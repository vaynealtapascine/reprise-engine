//! SVG backend. Glyphs are drawn from their outlines, so the output needs no
//! fonts installed and looks the same everywhere.

use std::collections::BTreeSet;
use std::fmt::Write;

use reprise_font::{FontError, FontStore, PathCmd};
use reprise_geom::Length;

use crate::{DisplayList, Item};

fn pt(l: Length) -> f32 {
    l.to_pt_f32()
}

pub fn render(list: &DisplayList, fonts: &FontStore) -> Result<String, FontError> {
    let mut defs = String::new();
    let mut body = String::new();
    let mut defined = BTreeSet::new();

    for item in &list.items {
        match item {
            Item::Glyphs {
                face,
                size,
                color,
                glyphs,
            } => {
                let font = fonts.get(face)?;
                let scale = pt(*size) / font.metrics().units_per_em as f32;
                let _ = write!(
                    body,
                    r#"<g fill="{}" fill-opacity="{}">"#,
                    color.hex(),
                    color.alpha()
                );
                for g in glyphs {
                    let key = format!("g{}-{}", &face.hash[..8], g.id);
                    if defined.insert(key.clone()) {
                        let _ = write!(
                            defs,
                            r#"<path id="{key}" d="{}"/>"#,
                            path_data(&font.outline(g.id))
                        );
                    }
                    let _ = write!(
                        body,
                        r##"<use href="#{key}" transform="translate({} {}) scale({scale} {})"/>"##,
                        pt(g.x),
                        pt(g.y),
                        -scale
                    );
                }
                body.push_str("</g>");
            }
            Item::Rect {
                rect, fill, stroke, ..
            } => {
                let _ = write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" fill-opacity="{}" stroke="{}" stroke-opacity="{}" stroke-width="0.25"/>"#,
                    pt(rect.origin.x),
                    pt(rect.origin.y),
                    pt(rect.width),
                    pt(rect.height),
                    fill.map_or("none".into(), |c| c.hex()),
                    fill.map_or(0.0, |c| c.alpha()),
                    stroke.map_or("none".into(), |c| c.hex()),
                    stroke.map_or(0.0, |c| c.alpha()),
                );
            }
            Item::Line {
                from, to, color, ..
            } => {
                let _ = write!(
                    body,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-opacity="{}" stroke-width="0.5"/>"#,
                    pt(from.x),
                    pt(from.y),
                    pt(to.x),
                    pt(to.y),
                    color.hex(),
                    color.alpha()
                );
            }
        }
    }

    let (w, h) = (pt(list.width), pt(list.height));
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}pt" height="{h}pt" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#fff"/><defs>{defs}</defs>{body}</svg>"##
    ))
}

fn path_data(cmds: &[PathCmd]) -> String {
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
