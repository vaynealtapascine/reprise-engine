//! Fixed-point layout units and typed coordinate spaces (decisions 19 and 20).
//!
//! Every length that composition produces is a [`Length`]: an integer count of
//! 1/1024 pt. Integer arithmetic keeps layout identical on every platform (38).
//! Floating point is allowed only when a backend renders the result.
//!
//! # Numeric rules (19)
//!
//! -   **Rounding:** every division rounds half away from zero.
//! -   **Overflow:** arithmetic saturates at [`Length::MIN`] and [`Length::MAX`]
//!     instead of wrapping or panicking, so hostile document values can't crash
//!     layout (37). A saturated length is a sign of hostile input, not a result
//!     to rely on.
//! -   **Division by zero** saturates by the sign of the numerator, and gives
//!     zero for `0 / 0`.
//! -   **Comparison** is integer comparison; there is no epsilon.
//!
//! # Spaces (20)
//!
//! Points carry their space as a type parameter, and only a [`Transform`]
//! moves a point from one space to another. [`FrameSpace`] and [`LineSpace`]
//! are *logical*: x runs along the inline axis and y along the block axis.
//! [`PageSpace`] is *physical*. Writing modes, rotation and mirroring all live
//! in the transform from a frame to its page, so composition never needs to
//! know about them.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Sub-units per point.
pub const UNITS_PER_PT: i32 = 1024;

/// A resolved length in 1/1024 pt. Arithmetic saturates (see the crate docs).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Length(pub i32);

impl Length {
    pub const ZERO: Length = Length(0);
    pub const MAX: Length = Length(i32::MAX);
    pub const MIN: Length = Length(i32::MIN);

    pub const fn from_pt(pt: i32) -> Length {
        Length(pt.saturating_mul(UNITS_PER_PT))
    }

    /// Scales a value in font design units to this size: `units * size / upem`.
    pub fn from_font_units(units: i32, size: Length, units_per_em: u16) -> Length {
        Length(saturate(div_round(
            units as i64 * size.0 as i64,
            units_per_em as i64,
        )))
    }

    /// Multiplies by `num / den`.
    pub fn mul_ratio(self, num: i32, den: i32) -> Length {
        Length(saturate(div_round(self.0 as i64 * num as i64, den as i64)))
    }

    pub fn abs(self) -> Length {
        Length(self.0.saturating_abs())
    }

    /// For rendering only; never feed the result back into layout.
    pub fn to_pt_f32(self) -> f32 {
        self.0 as f32 / UNITS_PER_PT as f32
    }
}

/// `num / den`, rounding half away from zero. Division by zero saturates by
/// the sign of `num`.
pub fn div_round(num: i64, den: i64) -> i64 {
    if den == 0 {
        return match num.signum() {
            1 => i64::MAX,
            -1 => i64::MIN,
            _ => 0,
        };
    }
    // Round the magnitude, then apply the sign. i128 so nothing can overflow.
    let (n, d) = ((num as i128).abs(), (den as i128).abs());
    let magnitude = (n + d / 2) / d;
    let q = if (num < 0) != (den < 0) {
        -magnitude
    } else {
        magnitude
    };
    q.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn saturate(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

impl fmt::Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}pt", self.0 as f64 / UNITS_PER_PT as f64)
    }
}

impl Add for Length {
    type Output = Length;
    fn add(self, rhs: Length) -> Length {
        Length(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Length {
    type Output = Length;
    fn sub(self, rhs: Length) -> Length {
        Length(self.0.saturating_sub(rhs.0))
    }
}

impl Neg for Length {
    type Output = Length;
    fn neg(self) -> Length {
        Length(self.0.saturating_neg())
    }
}

impl AddAssign for Length {
    fn add_assign(&mut self, rhs: Length) {
        *self = *self + rhs;
    }
}

impl SubAssign for Length {
    fn sub_assign(&mut self, rhs: Length) {
        *self = *self - rhs;
    }
}

impl std::iter::Sum for Length {
    fn sum<I: Iterator<Item = Length>>(iter: I) -> Length {
        iter.fold(Length::ZERO, Add::add)
    }
}

/// Rounds half away from zero and saturates, like every other operation here.
impl std::ops::Mul for Fixed {
    type Output = Fixed;
    fn mul(self, rhs: Fixed) -> Fixed {
        Fixed(saturate(div_round(self.0 as i64 * rhs.0 as i64, 1 << 16)))
    }
}

/// A dimensionless fixed-point scalar with 16 fractional bits, used for
/// transform coefficients. `Fixed::ONE` is 65536.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fixed(pub i32);

impl Fixed {
    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(1 << 16);
    pub const MINUS_ONE: Fixed = Fixed(-(1 << 16));

    pub const fn from_int(n: i16) -> Fixed {
        Fixed((n as i32) << 16)
    }

    /// `num / den`, rounded.
    pub fn from_ratio(num: i32, den: i32) -> Fixed {
        Fixed(saturate(div_round((num as i64) << 16, den as i64)))
    }

    pub fn scale(self, l: Length) -> Length {
        Length(saturate(div_round(self.0 as i64 * l.0 as i64, 1 << 16)))
    }

    fn saturating_add(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.saturating_add(rhs.0))
    }

    /// For rendering only.
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / 65536.0
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0 as f64 / 65536.0)
    }
}

/// A 2D affine map with fixed-point coefficients:
///
/// ```text
/// x2 = xx * x + xy * y + tx
/// y2 = yx * x + yy * y + ty
/// ```
///
/// Applying and composing matrices is integer arithmetic, rounded half away
/// from zero, so results are identical on every platform. Rotations by
/// quarter turns and mirrors are exact.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Matrix {
    pub xx: Fixed,
    pub yx: Fixed,
    pub xy: Fixed,
    pub yy: Fixed,
    pub tx: Length,
    pub ty: Length,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        xx: Fixed::ONE,
        yx: Fixed::ZERO,
        xy: Fixed::ZERO,
        yy: Fixed::ONE,
        tx: Length::ZERO,
        ty: Length::ZERO,
    };

    pub const fn translate(tx: Length, ty: Length) -> Matrix {
        Matrix {
            tx,
            ty,
            ..Matrix::IDENTITY
        }
    }

    pub const fn scale(sx: Fixed, sy: Fixed) -> Matrix {
        Matrix {
            xx: sx,
            yy: sy,
            ..Matrix::IDENTITY
        }
    }

    /// Mirrors along the x axis: x becomes -x.
    pub const fn mirror_x() -> Matrix {
        Matrix::scale(Fixed::MINUS_ONE, Fixed::ONE)
    }

    /// Rotates by `turns` quarter turns clockwise, in a y-down space. Exact.
    pub const fn rotate_quarter(turns: i32) -> Matrix {
        let (c, s) = match turns.rem_euclid(4) {
            0 => (Fixed::ONE, Fixed::ZERO),
            1 => (Fixed::ZERO, Fixed::ONE),
            2 => (Fixed::MINUS_ONE, Fixed::ZERO),
            _ => (Fixed::ZERO, Fixed::MINUS_ONE),
        };
        Matrix {
            xx: c,
            yx: s,
            xy: Fixed(-s.0),
            yy: c,
            tx: Length::ZERO,
            ty: Length::ZERO,
        }
    }

    pub fn is_translation(&self) -> bool {
        self.xx == Fixed::ONE
            && self.yy == Fixed::ONE
            && self.xy == Fixed::ZERO
            && self.yx == Fixed::ZERO
    }

    pub fn apply(&self, x: Length, y: Length) -> (Length, Length) {
        (
            self.xx.scale(x) + self.xy.scale(y) + self.tx,
            self.yx.scale(x) + self.yy.scale(y) + self.ty,
        )
    }

    /// Applies `self` first, then `next`.
    pub fn then(&self, next: &Matrix) -> Matrix {
        let (a, b) = (next, self);
        Matrix {
            xx: (a.xx * b.xx).saturating_add(a.xy * b.yx),
            yx: (a.yx * b.xx).saturating_add(a.yy * b.yx),
            xy: (a.xx * b.xy).saturating_add(a.xy * b.yy),
            yy: (a.yx * b.xy).saturating_add(a.yy * b.yy),
            tx: a.xx.scale(b.tx) + a.xy.scale(b.ty) + a.tx,
            ty: a.yx.scale(b.tx) + a.yy.scale(b.ty) + a.ty,
        }
    }

    /// The inverse map, or `None` when the matrix is singular. Exact for
    /// translations, quarter turns and mirrors; otherwise rounded.
    pub fn inverse(&self) -> Option<Matrix> {
        // The determinant, in 2^32 units.
        let det = self.xx.0 as i64 * self.yy.0 as i64 - self.xy.0 as i64 * self.yx.0 as i64;
        if det == 0 {
            return None;
        }
        // Each coefficient is c / det; in 2^16 units that is c * 2^32 / det.
        let inv = |c: i32| Fixed(saturate(div_round((c as i64) << 32, det)));
        let m = Matrix {
            xx: inv(self.yy.0),
            yx: inv(self.yx.0.saturating_neg()),
            xy: inv(self.xy.0.saturating_neg()),
            yy: inv(self.xx.0),
            tx: Length::ZERO,
            ty: Length::ZERO,
        };
        let (tx, ty) = m.apply(self.tx, self.ty);
        Some(Matrix {
            tx: -tx,
            ty: -ty,
            ..m
        })
    }
}

impl fmt::Debug for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_translation() {
            write!(f, "translate({:?}, {:?})", self.tx, self.ty)
        } else {
            write!(
                f,
                "matrix({:?} {:?} {:?} {:?} {:?} {:?})",
                self.xx, self.yx, self.xy, self.yy, self.tx, self.ty
            )
        }
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
    /// A page. Physical: the origin is its top-left corner, x grows rightwards
    /// and y downwards.
    PageSpace,
    /// A frame that content flows into. Logical: x runs along the inline axis
    /// from the start edge, y along the block axis from the block-start edge.
    FrameSpace,
    /// A line box. Logical: the origin is the line's start edge on its baseline.
    LineSpace
);

/// A point in space `S`.
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

    pub fn max_x(&self) -> Length {
        self.origin.x + self.width
    }

    pub fn max_y(&self) -> Length {
        self.origin.y + self.height
    }

    /// True when `p` is inside, counting the start edges but not the end edges.
    pub fn contains(&self, p: Point<S>) -> bool {
        p.x >= self.origin.x && p.x < self.max_x() && p.y >= self.origin.y && p.y < self.max_y()
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, other: &Rect<S>) -> Rect<S> {
        let x = self.origin.x.min(other.origin.x);
        let y = self.origin.y.min(other.origin.y);
        Rect::new(
            Point::new(x, y),
            self.max_x().max(other.max_x()) - x,
            self.max_y().max(other.max_y()) - y,
        )
    }
}

impl<S: Space> fmt::Debug for Rect<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} {:?}x{:?}", self.origin, self.width, self.height)
    }
}

/// A transform from space `From` into space `To`: a [`Matrix`] with its
/// spaces checked by type.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "", transparent)]
pub struct Transform<From: Space, To: Space> {
    matrix: Matrix,
    #[serde(skip)]
    _spaces: PhantomData<(From, To)>,
}

impl<From: Space, To: Space> Transform<From, To> {
    pub const fn new(matrix: Matrix) -> Self {
        Transform {
            matrix,
            _spaces: PhantomData,
        }
    }

    pub const fn translate(dx: Length, dy: Length) -> Self {
        Transform::new(Matrix::translate(dx, dy))
    }

    pub const fn matrix(&self) -> &Matrix {
        &self.matrix
    }

    pub fn apply(&self, p: Point<From>) -> Point<To> {
        let (x, y) = self.matrix.apply(p.x, p.y);
        Point::new(x, y)
    }

    /// The axis-aligned bounds, in `To`, of a rectangle in `From`.
    pub fn bounds(&self, r: &Rect<From>) -> Rect<To> {
        let corners = [
            (r.origin.x, r.origin.y),
            (r.max_x(), r.origin.y),
            (r.origin.x, r.max_y()),
            (r.max_x(), r.max_y()),
        ]
        .map(|(x, y)| self.matrix.apply(x, y));
        let xs = corners.map(|c| c.0);
        let ys = corners.map(|c| c.1);
        let (min_x, max_x) = (xs.into_iter().min(), xs.into_iter().max());
        let (min_y, max_y) = (ys.into_iter().min(), ys.into_iter().max());
        let (min_x, max_x) = (min_x.unwrap_or_default(), max_x.unwrap_or_default());
        let (min_y, max_y) = (min_y.unwrap_or_default(), max_y.unwrap_or_default());
        Rect::new(Point::new(min_x, min_y), max_x - min_x, max_y - min_y)
    }

    /// Applies `self` first, then `next`.
    pub fn then<Next: Space>(&self, next: &Transform<To, Next>) -> Transform<From, Next> {
        Transform::new(self.matrix.then(&next.matrix))
    }

    pub fn inverse(&self) -> Option<Transform<To, From>> {
        self.matrix.inverse().map(Transform::new)
    }
}

impl<From: Space, To: Space> fmt::Debug for Transform<From, To> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}->{} {:?}", From::NAME, To::NAME, self.matrix)
    }
}

/// Logical inline direction (20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InlineDirection {
    #[default]
    Ltr,
    Rtl,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(n: i32) -> Length {
        Length::from_pt(n)
    }

    #[test]
    fn font_units_round_half_away_from_zero() {
        // 500 units at 12pt with 1000 upem is exactly 6pt.
        assert_eq!(Length::from_font_units(500, pt(12), 1000), pt(6));
        // 1 unit at 1/1024pt over 2 upem is half a sub-unit, rounded away from zero.
        assert_eq!(Length::from_font_units(1, Length(1), 2), Length(1));
        assert_eq!(Length::from_font_units(-1, Length(1), 2), Length(-1));
    }

    #[test]
    fn arithmetic_saturates_instead_of_wrapping() {
        assert_eq!(Length::MAX + Length(1), Length::MAX);
        assert_eq!(Length::MIN - Length(1), Length::MIN);
        assert_eq!(-Length::MIN, Length::MAX);
        assert_eq!(Length::from_pt(i32::MAX), Length::MAX);
        assert_eq!(Length::MAX.mul_ratio(3, 1), Length::MAX);
        assert_eq!(Length::MIN.mul_ratio(-1, 1), Length::MAX);
    }

    #[test]
    fn division_by_zero_saturates_by_sign() {
        assert_eq!(pt(1).mul_ratio(1, 0), Length::MAX);
        assert_eq!(pt(-1).mul_ratio(1, 0), Length::MIN);
        assert_eq!(Length::ZERO.mul_ratio(1, 0), Length::ZERO);
        assert_eq!(Length::from_font_units(500, pt(12), 0), Length::MAX);
    }

    #[test]
    fn transforms_compose() {
        let a: Transform<LineSpace, FrameSpace> = Transform::translate(pt(1), pt(2));
        let b: Transform<FrameSpace, PageSpace> = Transform::translate(pt(10), pt(20));
        let p = a.then(&b).apply(Point::origin());
        assert_eq!(p, Point::new(pt(11), pt(22)));
    }

    #[test]
    fn quarter_turns_and_mirrors_are_exact() {
        let r = Matrix::rotate_quarter(1);
        // Clockwise in a y-down space: +x goes to +y.
        assert_eq!(r.apply(pt(3), pt(0)), (Length::ZERO, pt(3)));
        let full = r.then(&r).then(&r).then(&r);
        assert_eq!(full, Matrix::IDENTITY);
        let m = Matrix::mirror_x();
        assert_eq!(m.then(&m), Matrix::IDENTITY);
        assert_eq!(m.apply(pt(5), pt(7)), (pt(-5), pt(7)));
    }

    #[test]
    fn inverses_undo_exact_transforms() {
        let t = Matrix::rotate_quarter(3)
            .then(&Matrix::mirror_x())
            .then(&Matrix::translate(pt(40), Length(-77)));
        let inv = t.inverse().unwrap();
        assert_eq!(t.then(&inv), Matrix::IDENTITY);
        let (x, y) = t.apply(Length(12345), Length(-999));
        assert_eq!(inv.apply(x, y), (Length(12345), Length(-999)));
        assert_eq!(Matrix::scale(Fixed::ZERO, Fixed::ONE).inverse(), None);
    }

    #[test]
    fn bounds_of_a_rotated_rect() {
        let t: Transform<FrameSpace, PageSpace> =
            Transform::new(Matrix::rotate_quarter(1).then(&Matrix::translate(pt(100), pt(0))));
        let r = Rect::new(Point::origin(), pt(30), pt(10));
        assert_eq!(
            t.bounds(&r),
            Rect::new(Point::new(pt(90), pt(0)), pt(10), pt(30))
        );
    }

    #[test]
    fn fixed_scaling_rounds_half_away_from_zero() {
        let half = Fixed::from_ratio(1, 2);
        assert_eq!(half.scale(Length(3)), Length(2));
        assert_eq!(half.scale(Length(-3)), Length(-2));
        assert_eq!(Fixed::from_ratio(1, 4) * Fixed::from_int(4), Fixed::ONE);
    }

    #[test]
    fn rounding_is_symmetric_for_every_sign_combination() {
        for (n, d, q) in [
            (3, 2, 2),
            (-3, 2, -2),
            (3, -2, -2),
            (-3, -2, 2),
            (1, 3, 0),
            (-1, -3, 0),
        ] {
            assert_eq!(div_round(n, d), q, "{n} / {d}");
        }
        assert_eq!(div_round(i64::MIN, -1), i64::MAX);
    }

    #[test]
    fn transforms_serialize_as_their_matrix() {
        let t: Transform<FrameSpace, PageSpace> = Transform::translate(Length(1), Length(2));
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"xx":65536,"yx":0,"xy":0,"yy":65536,"tx":1,"ty":2}"#
        );
    }
}
