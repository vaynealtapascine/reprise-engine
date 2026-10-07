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

    /// The length of the vector `(dx, dy)`: the square root of
    /// `dx² + dy²`, rounded half up and saturated. Exact integer arithmetic.
    pub fn hypot(dx: Length, dy: Length) -> Length {
        let n = (dx.0.unsigned_abs() as u128).pow(2) + (dy.0.unsigned_abs() as u128).pow(2);
        let r = isqrt(n);
        // (r + 1/2)² = r² + r + 1/4, so n rounds up exactly when n > r² + r.
        let r = if n - r * r > r { r + 1 } else { r };
        Length(r.min(i32::MAX as u128) as i32)
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

/// Exact constructors for the frame transforms of decision 20: arbitrary
/// rotations, mirrors on either axis, and transforms about an origin.
///
/// # Arbitrary angles
///
/// An angle is authored as a **direction**: the integer vector `(dx, dy)` that
/// the rotated +x axis points along. [`Matrix::rotate_toward`] normalises it
/// with integer arithmetic only, rounding each coefficient (the cosine and the
/// sine) *correctly* to 16.16, so the matrix depends on nothing but the two
/// integers and is bit-identical on every platform (38). An angle in
/// thousandths of a degree is turned into a direction by
/// [`sin_cos_millidegrees`], an integer Taylor series that is its own
/// specification. Rotations are clockwise, because page space grows
/// downwards, like [`Matrix::rotate_quarter`].
impl Matrix {
    /// Mirrors along the y axis: y becomes -y.
    pub const fn mirror_y() -> Matrix {
        Matrix::scale(Fixed::ONE, Fixed::MINUS_ONE)
    }

    /// The rotation that turns +x towards `(dx, dy)`. `None` for `(0, 0)`,
    /// which has no direction. Axis directions give exact quarter turns.
    pub fn rotate_toward(dx: i32, dy: i32) -> Option<Matrix> {
        let n = (dx.unsigned_abs() as u128).pow(2) + (dy.unsigned_abs() as u128).pow(2);
        if n == 0 {
            return None;
        }
        let (c, s) = (unit_coefficient(dx, n), unit_coefficient(dy, n));
        Some(Matrix {
            xx: c,
            yx: s,
            xy: Fixed(-s.0),
            yy: c,
            tx: Length::ZERO,
            ty: Length::ZERO,
        })
    }

    /// The rotation by `millidegrees` thousandths of a degree, clockwise.
    /// Multiples of 90° are exact quarter turns.
    pub fn rotate_millidegrees(millidegrees: i64) -> Matrix {
        let (c, s) = sin_cos_millidegrees(millidegrees);
        // (c, s) is a unit vector in Q30, so it is never (0, 0).
        Matrix::rotate_toward(c, s).unwrap_or(Matrix::IDENTITY)
    }

    /// The same map, performed about `(x, y)` instead of the origin: move
    /// `(x, y)` to the origin, apply `self`, move back.
    pub fn about(&self, x: Length, y: Length) -> Matrix {
        Matrix::translate(-x, -y)
            .then(self)
            .then(&Matrix::translate(x, y))
    }

    /// The determinant of the linear part, in units of 2⁻³². Zero exactly
    /// when [`Matrix::inverse`] is `None`.
    pub fn determinant(&self) -> i64 {
        self.xx.0 as i64 * self.yy.0 as i64 - self.xy.0 as i64 * self.yx.0 as i64
    }

    /// Whether the inverse exists *and* undoes the map: composing the two
    /// gives the identity's linear part to within 1/256. A matrix so close to
    /// singular that its inverse saturates is not invertible in practice, and
    /// layout treats it as degenerate.
    pub fn has_usable_inverse(&self) -> bool {
        let Some(inv) = self.inverse() else {
            return false;
        };
        let round_trip = self.then(&inv);
        let near = |a: Fixed, b: Fixed| (a.0 as i64 - b.0 as i64).abs() <= 256;
        near(round_trip.xx, Fixed::ONE)
            && near(round_trip.yy, Fixed::ONE)
            && near(round_trip.xy, Fixed::ZERO)
            && near(round_trip.yx, Fixed::ZERO)
    }
}

/// `v / √n` in 16.16, rounded half away from zero, computed exactly: the
/// result is the largest `c ≥ 0` with `(c - ½)² ≤ v² 2³² / n`, that is
/// `(2c - 1)² n ≤ v² 2³⁴`. Callers guarantee `v² ≤ n`, so `c ≤ 2¹⁶`.
fn unit_coefficient(v: i32, n: u128) -> Fixed {
    let a = v.unsigned_abs() as u128;
    let bound = (a * a) << 34;
    let fits = |c: u128| c == 0 || (2 * c - 1).pow(2) * n <= bound;
    let mut c = isqrt(((a * a) << 32) / n).min(1 << 16);
    while c < (1 << 16) && fits(c + 1) {
        c += 1;
    }
    while !fits(c) {
        c -= 1;
    }
    let c = c as i32;
    Fixed(if v < 0 { -c } else { c })
}

/// The largest `r` with `r² ≤ n`.
pub fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    // Newton's method from a power of two at or above the root decreases
    // monotonically to it.
    let mut x = 1u128 << (128 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// π × 2⁶⁰, rounded: the one transcendental constant layout uses.
const PI_Q60: i128 = 3_622_009_729_038_561_421;

/// The cosine and sine of an angle in thousandths of a degree, in Q30 (so
/// 1.0 is 2³⁰). Clockwise in a y-down space. Multiples of 90° are exact.
///
/// Integer arithmetic only: the angle is reduced to a quadrant exactly, then
/// a Taylor series is summed in Q60 until its terms vanish. That procedure is
/// the specification, so the result is the same on every platform (38).
pub fn sin_cos_millidegrees(millidegrees: i64) -> (i32, i32) {
    let md = millidegrees.rem_euclid(360_000);
    let (quadrant, rest) = (md / 90_000, md % 90_000);
    // rest < 90°, so x < π/2 < 2⁶¹ in Q60.
    let x = div_round_i128(rest as i128 * PI_Q60, 180_000);
    let x2 = div_round_i128(x * x, 1 << 60);
    let series = |first: i128, start: i128| {
        let (mut sum, mut term, mut k) = (first, first, start);
        for _ in 0..64 {
            term = -div_round_i128(div_round_i128(term * x2, 1 << 60), k * (k + 1));
            if term == 0 {
                break;
            }
            sum += term;
            k += 2;
        }
        sum
    };
    let q30 = |v: i128| div_round_i128(v, 1 << 30).clamp(-(1 << 30), 1 << 30) as i32;
    let (c, s) = (q30(series(1 << 60, 1)), q30(series(x, 2)));
    match quadrant {
        0 => (c, s),
        1 => (-s, c),
        2 => (-c, -s),
        _ => (s, -c),
    }
}

/// `n / d` for `d > 0`, rounding half away from zero.
fn div_round_i128(n: i128, d: i128) -> i128 {
    let magnitude = (n.abs() + d / 2) / d;
    if n < 0 { -magnitude } else { magnitude }
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

/// `text-orientation` (20): how glyphs sit in vertical lines. An authored
/// style keyword; it has no effect in horizontal frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextOrientation {
    /// UAX #50: upright for `U` and `Tu`, rotated for `R`, and for `Tr` upright
    /// only when the face has a vertical alternate.
    #[default]
    Mixed,
    /// Every character upright, and strong left to right for bidi.
    Upright,
    /// Every character rotated, as in horizontal text turned a quarter.
    Sideways,
}

impl TextOrientation {
    /// The stored keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            TextOrientation::Mixed => "mixed",
            TextOrientation::Upright => "upright",
            TextOrientation::Sideways => "sideways",
        }
    }

    /// Reads a stored keyword; `None` for anything else, which callers keep
    /// verbatim and report.
    pub fn parse(keyword: &str) -> Option<TextOrientation> {
        match keyword {
            "mixed" => Some(TextOrientation::Mixed),
            "upright" => Some(TextOrientation::Upright),
            "sideways" => Some(TextOrientation::Sideways),
            _ => None,
        }
    }
}

/// `text-combine-upright` (20): tate-chū-yoko, short horizontal runs set
/// upright in one em of a vertical line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextCombineUpright {
    #[default]
    None,
    /// Each maximal run of non-whitespace graphemes, up to a limit.
    All,
    /// Each maximal run of at most this many ASCII digits. Always 2 to 4.
    Digits(u8),
}

impl TextCombineUpright {
    /// The stored keyword: `none`, `all` or `digits N`.
    pub fn keyword(self) -> String {
        match self {
            TextCombineUpright::None => "none".into(),
            TextCombineUpright::All => "all".into(),
            TextCombineUpright::Digits(n) => format!("digits {n}"),
        }
    }

    /// Reads a stored keyword. `digits` alone means `digits 2`; counts outside
    /// 2 to 4 are not readable, as in CSS.
    pub fn parse(keyword: &str) -> Option<TextCombineUpright> {
        match keyword {
            "none" => Some(TextCombineUpright::None),
            "all" => Some(TextCombineUpright::All),
            "digits" => Some(TextCombineUpright::Digits(2)),
            _ => match keyword.strip_prefix("digits ")? {
                "2" => Some(TextCombineUpright::Digits(2)),
                "3" => Some(TextCombineUpright::Digits(3)),
                "4" => Some(TextCombineUpright::Digits(4)),
                _ => None,
            },
        }
    }
}

/// How a shaped run's glyphs sit relative to its line (20, 22). Derived by
/// shaping and recorded in layout, so editing and display agree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlyphOrientation {
    /// Glyph axes are the line's axes: horizontal text, or sideways in a
    /// vertical line. Shaped horizontally.
    #[default]
    Sideways,
    /// Each glyph's top points to the line's start: shaped vertically, with
    /// inline advances down the column, on the line's central baseline.
    Upright,
    /// Tate-chū-yoko: a horizontal composition standing upright in one em of
    /// the line. Drawn like `Upright`.
    Combined,
}

impl GlyphOrientation {
    pub fn is_sideways(&self) -> bool {
        *self == GlyphOrientation::Sideways
    }

    /// True for runs whose glyphs stand upright in a vertical line.
    pub fn is_upright(self) -> bool {
        !self.is_sideways()
    }
}

impl Matrix {
    /// Swaps the axes: x becomes y and y becomes x. A reflection, exact.
    pub const fn transpose() -> Matrix {
        Matrix {
            xx: Fixed::ZERO,
            yx: Fixed::ONE,
            xy: Fixed::ONE,
            yy: Fixed::ZERO,
            tx: Length::ZERO,
            ty: Length::ZERO,
        }
    }
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
    fn orientation_keywords_round_trip_and_refuse_the_rest() {
        for o in [
            TextOrientation::Mixed,
            TextOrientation::Upright,
            TextOrientation::Sideways,
        ] {
            assert_eq!(TextOrientation::parse(o.keyword()), Some(o));
        }
        for c in [
            TextCombineUpright::None,
            TextCombineUpright::All,
            TextCombineUpright::Digits(2),
            TextCombineUpright::Digits(3),
            TextCombineUpright::Digits(4),
        ] {
            assert_eq!(TextCombineUpright::parse(&c.keyword()), Some(c));
        }
        assert_eq!(
            TextCombineUpright::parse("digits"),
            Some(TextCombineUpright::Digits(2))
        );
        for bad in [
            "",
            "Mixed",
            "digits 1",
            "digits 5",
            "digits  2",
            "digits -2",
            "sideways-right",
        ] {
            assert_eq!(TextOrientation::parse(bad), None);
            assert_eq!(TextCombineUpright::parse(bad), None);
        }
    }

    #[test]
    fn transposition_is_an_exact_involution() {
        let t = Matrix::transpose();
        assert_eq!(t.then(&t), Matrix::IDENTITY);
        assert_eq!(t.apply(pt(3), pt(-7)), (pt(-7), pt(3)));
        assert!(t.determinant() < 0);
        assert_eq!(t.inverse(), Some(t));
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

#[cfg(test)]
mod rotation_tests {
    use super::*;

    fn pt(n: i32) -> Length {
        Length::from_pt(n)
    }

    #[test]
    fn axis_directions_are_exact_quarter_turns() {
        assert_eq!(Matrix::rotate_toward(7, 0), Some(Matrix::IDENTITY));
        assert_eq!(Matrix::rotate_toward(0, 3), Some(Matrix::rotate_quarter(1)));
        assert_eq!(
            Matrix::rotate_toward(i32::MIN, 0),
            Some(Matrix::rotate_quarter(2))
        );
        assert_eq!(
            Matrix::rotate_toward(0, -1),
            Some(Matrix::rotate_quarter(3))
        );
        assert_eq!(Matrix::rotate_toward(0, 0), None, "no direction");
        for turns in -8..8 {
            assert_eq!(
                Matrix::rotate_millidegrees(turns as i64 * 90_000),
                Matrix::rotate_quarter(turns)
            );
        }
    }

    #[test]
    fn directions_round_each_coefficient_correctly() {
        // 3-4-5: cos 0.6 and sin 0.8, rounded to 16.16.
        let m = Matrix::rotate_toward(3, 4).unwrap();
        assert_eq!(
            (m.xx.0, m.yx.0, m.xy.0, m.yy.0),
            (39322, 52429, -52429, 39322)
        );
        // 45°: 0.70710678… × 65536 = 46340.95.
        let m = Matrix::rotate_toward(1, 1).unwrap();
        assert_eq!((m.xx.0, m.yx.0), (46341, 46341));
        // The same direction at any scale gives the same matrix, up to the
        // integer limits.
        for k in [2, 1000, 1 << 20] {
            assert_eq!(
                Matrix::rotate_toward(3 * k, 4 * k),
                Matrix::rotate_toward(3, 4)
            );
        }
        assert_eq!(
            Matrix::rotate_toward(i32::MIN, i32::MIN),
            Matrix::rotate_toward(-1, -1)
        );
        // Compared with a float reference away from rounding ties. Floats
        // only check the integer result here; they never feed layout.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let dx = (seed as u32) as i32;
            let dy = ((seed >> 32) as u32) as i32;
            let Some(m) = Matrix::rotate_toward(dx, dy) else {
                continue;
            };
            let len = ((dx as f64).powi(2) + (dy as f64).powi(2)).sqrt();
            for (v, got) in [(dx, m.xx.0), (dy, m.yx.0)] {
                let exact = v as f64 / len * 65536.0;
                if (exact.abs().fract() - 0.5).abs() > 1e-6 {
                    assert_eq!(
                        got as f64,
                        exact.abs().round() * exact.signum(),
                        "{dx},{dy}"
                    );
                }
            }
        }
    }

    #[test]
    fn millidegrees_follow_the_unit_circle() {
        let close = |md: i64, c: f64, s: f64| {
            let (qc, qs) = sin_cos_millidegrees(md);
            let unit = (1u64 << 30) as f64;
            assert!((qc as f64 / unit - c).abs() < 1e-8, "cos {md}");
            assert!((qs as f64 / unit - s).abs() < 1e-8, "sin {md}");
        };
        close(0, 1.0, 0.0);
        close(30_000, 3f64.sqrt() / 2.0, 0.5);
        close(45_000, 0.5f64.sqrt(), 0.5f64.sqrt());
        close(-30_000, 3f64.sqrt() / 2.0, -0.5);
        close(
            359_999,
            (359.999f64).to_radians().cos(),
            (359.999f64).to_radians().sin(),
        );
        close(
            89_999,
            (89.999f64).to_radians().cos(),
            (89.999f64).to_radians().sin(),
        );
        // Reduction is exact at the integer limits.
        assert_eq!(
            sin_cos_millidegrees(i64::MAX),
            sin_cos_millidegrees(i64::MAX.rem_euclid(360_000))
        );
        assert_eq!(
            sin_cos_millidegrees(i64::MIN),
            sin_cos_millidegrees(i64::MIN.rem_euclid(360_000))
        );
        let m = Matrix::rotate_millidegrees(30_000);
        assert_eq!((m.xx.0, m.yx.0), (56756, 32768));
    }

    #[test]
    fn rotations_about_a_point_keep_it_fixed() {
        let r = Matrix::rotate_toward(-7, 13)
            .unwrap()
            .about(pt(50), pt(-20));
        assert_eq!(r.apply(pt(50), pt(-20)), (pt(50), pt(-20)));
        let flip = Matrix::mirror_y().about(Length::ZERO, pt(10));
        assert_eq!(flip.apply(pt(3), pt(0)), (pt(3), pt(20)));
        assert_eq!(flip.then(&flip), Matrix::IDENTITY);
    }

    #[test]
    fn arbitrary_rotations_round_trip_within_a_unit() {
        let m = Matrix::rotate_toward(1_000_003, -999_999)
            .unwrap()
            .then(&Matrix::translate(pt(123), pt(-45)));
        let inv = m.inverse().unwrap();
        for (x, y) in [(0, 0), (12345, -999), (pt(500).0, pt(700).0)] {
            let (px, py) = m.apply(Length(x), Length(y));
            let (bx, by) = inv.apply(px, py);
            assert!((bx.0 - x).abs() <= 2 && (by.0 - y).abs() <= 2, "{x},{y}");
        }
        assert!(m.has_usable_inverse());
    }

    #[test]
    fn singular_and_nearly_singular_matrices_have_no_usable_inverse() {
        assert!(!Matrix::scale(Fixed::ZERO, Fixed::ONE).has_usable_inverse());
        // A 1/65536 scale inverts to 65536, past the 16.16 range.
        assert!(!Matrix::scale(Fixed(1), Fixed::ONE).has_usable_inverse());
        assert!(Matrix::scale(Fixed::from_ratio(1, 4), Fixed::ONE).has_usable_inverse());
        assert!(Matrix::IDENTITY.has_usable_inverse());
        assert_eq!(
            Matrix::scale(Fixed::from_int(2), Fixed::ONE).determinant(),
            2 << 32
        );
    }

    #[test]
    fn square_roots_are_exact() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, u64::MAX as u128, u128::MAX] {
            let r = isqrt(n);
            assert!(r * r <= n, "{n}");
            assert!((r + 1).checked_mul(r + 1).is_none_or(|sq| sq > n), "{n}");
        }
        assert_eq!(Length::hypot(Length(3), Length(-4)), Length(5));
        // √2 = 1.414…, √(2·2) = 2; 1.5² = 2.25 so √2.24 rounds down.
        assert_eq!(Length::hypot(Length(1), Length(1)), Length(1));
        assert_eq!(Length::hypot(Length::MIN, Length::MIN), Length::MAX);
        assert_eq!(Length::hypot(Length::ZERO, Length::ZERO), Length::ZERO);
    }
}
