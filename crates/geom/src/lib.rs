//! Fixed-point layout units and typed coordinate spaces (decisions 19 and 20).
//!
//! Every length that composition produces is a [`Length`]: an integer count of
//! 1/1024 pt. Integer arithmetic keeps layout identical on every platform (38).
//! Floating point is allowed only when a backend renders the result.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Sub-units per point.
pub const UNITS_PER_PT: i32 = 1024;

/// A resolved length in 1/1024 pt.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Length(pub i32);

impl Length {
    pub const ZERO: Length = Length(0);
    pub const MAX: Length = Length(i32::MAX);

    pub const fn from_pt(pt: i32) -> Length {
        Length(pt * UNITS_PER_PT)
    }

    /// Scales a value in font design units to this size: `units * size / upem`,
    /// rounded half away from zero in integer arithmetic.
    pub fn from_font_units(units: i32, size: Length, units_per_em: u16) -> Length {
        Length(div_round(units as i64 * size.0 as i64, units_per_em as i64) as i32)
    }

    /// Multiplies by `num / den`, rounding half away from zero.
    pub fn mul_ratio(self, num: i32, den: i32) -> Length {
        Length(div_round(self.0 as i64 * num as i64, den as i64) as i32)
    }

    /// For rendering only; never feed the result back into layout.
    pub fn to_pt_f32(self) -> f32 {
        self.0 as f32 / UNITS_PER_PT as f32
    }
}

fn div_round(num: i64, den: i64) -> i64 {
    let half = den.abs() / 2;
    if (num < 0) != (den < 0) {
        (num - half) / den
    } else {
        (num + half) / den
    }
}

impl fmt::Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}pt", self.0 as f64 / UNITS_PER_PT as f64)
    }
}

impl Add for Length {
    type Output = Length;
    fn add(self, rhs: Length) -> Length {
        Length(self.0 + rhs.0)
    }
}

impl Sub for Length {
    type Output = Length;
    fn sub(self, rhs: Length) -> Length {
        Length(self.0 - rhs.0)
    }
}

impl Neg for Length {
    type Output = Length;
    fn neg(self) -> Length {
        Length(-self.0)
    }
}

impl AddAssign for Length {
    fn add_assign(&mut self, rhs: Length) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Length {
    fn sub_assign(&mut self, rhs: Length) {
        self.0 -= rhs.0;
    }
}

impl std::iter::Sum for Length {
    fn sum<I: Iterator<Item = Length>>(iter: I) -> Length {
        iter.fold(Length::ZERO, Add::add)
    }
}

/// Marker for a coordinate space. Points in different spaces don't mix without
/// an explicit [`Transform`].
pub trait Space: Copy + fmt::Debug + Default + 'static {
    const NAME: &'static str;
}

macro_rules! spaces {
    ($($(#[$m:meta])* $name:ident),*) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name;
        impl Space for $name {
            const NAME: &'static str = stringify!($name);
        }
    )*};
}

spaces!(
    /// A page; the origin is its top-left corner and y grows downwards.
    PageSpace,
    /// A frame that content flows into.
    FrameSpace,
    /// A line box; the origin is the start of the line on its baseline.
    LineSpace
);

/// A point in space `S`, on physical axes (x right, y down).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Point<S: Space> {
    pub x: Length,
    pub y: Length,
    #[serde(skip)]
    _space: PhantomData<S>,
}

impl<S: Space> Point<S> {
    pub const fn new(x: Length, y: Length) -> Self {
        Point {
            x,
            y,
            _space: PhantomData,
        }
    }

    pub const fn origin() -> Self {
        Point::new(Length::ZERO, Length::ZERO)
    }
}

impl<S: Space> fmt::Debug for Point<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({:?}, {:?})", S::NAME, self.x, self.y)
    }
}

/// An axis-aligned rectangle in space `S`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Rect<S: Space> {
    pub origin: Point<S>,
    pub width: Length,
    pub height: Length,
}

impl<S: Space> Rect<S> {
    pub const fn new(origin: Point<S>, width: Length, height: Length) -> Self {
        Rect {
            origin,
            width,
            height,
        }
    }

    pub fn max_y(&self) -> Length {
        self.origin.y + self.height
    }
}

impl<S: Space> fmt::Debug for Rect<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} {:?}x{:?}", self.origin, self.width, self.height)
    }
}

/// A transform from space `From` into space `To`.
///
/// The spike only translates. Rotation and mirroring come next; the north star
/// needs both.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Transform<From: Space, To: Space> {
    pub dx: Length,
    pub dy: Length,
    #[serde(skip)]
    _spaces: PhantomData<(From, To)>,
}

impl<From: Space, To: Space> Transform<From, To> {
    pub const fn translate(dx: Length, dy: Length) -> Self {
        Transform {
            dx,
            dy,
            _spaces: PhantomData,
        }
    }

    pub fn apply(&self, p: Point<From>) -> Point<To> {
        Point::new(p.x + self.dx, p.y + self.dy)
    }

    pub fn then<Next: Space>(&self, next: &Transform<To, Next>) -> Transform<From, Next> {
        Transform::translate(self.dx + next.dx, self.dy + next.dy)
    }
}

impl<From: Space, To: Space> fmt::Debug for Transform<From, To> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}->{} +({:?}, {:?})",
            From::NAME,
            To::NAME,
            self.dx,
            self.dy
        )
    }
}

/// Logical inline direction (20). The block direction is top to bottom in the spike.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InlineDirection {
    #[default]
    Ltr,
    Rtl,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_units_round_half_away_from_zero() {
        // 500 units at 12pt with 1000 upem is exactly 6pt.
        assert_eq!(
            Length::from_font_units(500, Length::from_pt(12), 1000),
            Length::from_pt(6)
        );
        // 1 unit at 1/1024pt over 2 upem is half a sub-unit, rounded away from zero.
        assert_eq!(Length::from_font_units(1, Length(1), 2), Length(1));
        assert_eq!(Length::from_font_units(-1, Length(1), 2), Length(-1));
    }

    #[test]
    fn transforms_compose() {
        let a: Transform<LineSpace, FrameSpace> =
            Transform::translate(Length::from_pt(1), Length::from_pt(2));
        let b: Transform<FrameSpace, PageSpace> =
            Transform::translate(Length::from_pt(10), Length::from_pt(20));
        let p = a.then(&b).apply(Point::origin());
        assert_eq!(p, Point::new(Length::from_pt(11), Length::from_pt(22)));
    }
}
