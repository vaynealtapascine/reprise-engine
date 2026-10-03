//! Styles (decisions 08, 17 and 18): named styles with inheritance, direct
//! overrides, and the values they resolve to.

use std::collections::BTreeMap;

use loro::{LoroMap, LoroValue};
use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::get_str;

/// A length as authored: a literal or relative to the font size (17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LengthExpr {
    Pt(Length),
    /// Thousandths of an em.
    Em(i32),
}

impl LengthExpr {
    pub fn resolve(self, em: Length) -> Length {
        match self {
            LengthExpr::Pt(l) => l,
            LengthExpr::Em(permille) => em.mul_ratio(permille, 1000),
        }
    }

    fn to_loro(self) -> LoroValue {
        match self {
            LengthExpr::Pt(l) => format!("pt:{}", l.0).into(),
            LengthExpr::Em(p) => format!("em:{p}").into(),
        }
    }

    fn from_loro(v: &str) -> Option<LengthExpr> {
        let (unit, n) = v.split_once(':')?;
        let n: i32 = n.parse().ok()?;
        match unit {
            "pt" => Some(LengthExpr::Pt(Length(n))),
            "em" => Some(LengthExpr::Em(n)),
            _ => None,
        }
    }
}

/// Style properties as authored, in a named style or as direct overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub parent: Option<String>,
    pub family: Option<String>,
    pub size: Option<LengthExpr>,
    pub line_height: Option<LengthExpr>,
}

impl Style {
    pub(crate) fn write(&self, map: &LoroMap) -> loro::LoroResult<()> {
        if let Some(p) = &self.parent {
            map.insert("parent", p.as_str())?;
        }
        if let Some(f) = &self.family {
            map.insert("family", f.as_str())?;
        }
        if let Some(s) = self.size {
            map.insert("size", s.to_loro())?;
        }
        if let Some(l) = self.line_height {
            map.insert("line-height", l.to_loro())?;
        }
        Ok(())
    }

    pub(crate) fn read(map: &LoroMap) -> Style {
        Style {
            parent: get_str(map, "parent"),
            family: get_str(map, "family"),
            size: get_str(map, "size").and_then(|v| LengthExpr::from_loro(&v)),
            line_height: get_str(map, "line-height").and_then(|v| LengthExpr::from_loro(&v)),
        }
    }
}

/// Used values after inheritance and overrides (08), with where each came from (39).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub family: String,
    pub size: Length,
    pub line_height: Length,
    pub explain: BTreeMap<String, String>,
    /// Properties whose resolved value was negative and was clamped to zero.
    /// Layout reports each one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clamped: Vec<String>,
}

/// Engine defaults, the bottom of every style chain.
pub fn default_style() -> Style {
    Style {
        parent: None,
        family: Some("Source Serif Pro".into()),
        size: Some(LengthExpr::Pt(Length::from_pt(10))),
        line_height: Some(LengthExpr::Em(1200)),
    }
}
