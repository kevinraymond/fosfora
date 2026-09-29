//! Float assertions for the unit tests.
//!
//! The workspace denies `clippy::float_cmp`, so the tests compare floats
//! through [`assert_close!`] instead of `assert_eq!`. The values they check
//! are exact by construction (constants copied through, packed uniform lanes
//! read back, whole-number floats), so the tolerance is tight: [`EPS`],
//! relative once a magnitude passes 1.

/// Relative tolerance of [`close`], absolute below a magnitude of 1.
pub const EPS: f32 = 1e-6;

/// Anything that flattens to a list of `f32`: a scalar, an array or slice of
/// them, or nested arrays such as uniform rows.
pub trait Floats {
    /// Appends every float in order.
    fn push_floats(&self, out: &mut Vec<f32>);
}

impl Floats for f32 {
    fn push_floats(&self, out: &mut Vec<f32>) {
        out.push(*self);
    }
}

impl<T: Floats> Floats for [T] {
    fn push_floats(&self, out: &mut Vec<f32>) {
        for x in self {
            x.push_floats(out);
        }
    }
}

impl<T: Floats, const N: usize> Floats for [T; N] {
    fn push_floats(&self, out: &mut Vec<f32>) {
        self.as_slice().push_floats(out);
    }
}

impl<T: Floats> Floats for Vec<T> {
    fn push_floats(&self, out: &mut Vec<f32>) {
        self.as_slice().push_floats(out);
    }
}

impl<T: Floats + ?Sized> Floats for &T {
    fn push_floats(&self, out: &mut Vec<f32>) {
        (**self).push_floats(out);
    }
}

fn flat(x: &(impl Floats + ?Sized)) -> Vec<f32> {
    let mut out = Vec::new();
    x.push_floats(&mut out);
    out
}

/// Whether `a` and `b` hold the same number of floats, each pair within
/// [`EPS`] (relative above 1). Identical bits always match, so equal
/// infinities and `f32::MAX` sentinels compare as equal.
pub fn close(a: &(impl Floats + ?Sized), b: &(impl Floats + ?Sized)) -> bool {
    let (a, b) = (flat(a), flat(b));
    a.len() == b.len()
        && a.iter().zip(&b).all(|(&x, &y)| {
            x.to_bits() == y.to_bits() || (x - y).abs() <= EPS * x.abs().max(y.abs()).max(1.0)
        })
}

/// `assert_eq!` for floats and float arrays, within [`EPS`]: see [`close`].
/// Takes the same optional message arguments as `assert_eq!`.
macro_rules! assert_close {
    ($a:expr, $b:expr $(,)?) => {{
        let (a, b) = (&$a, &$b);
        assert!(
            $crate::test_util::close(a, b),
            "assertion `left ≈ right` failed\n  left: {a:?}\n right: {b:?}"
        );
    }};
    ($a:expr, $b:expr, $($arg:tt)+) => {{
        let (a, b) = (&$a, &$b);
        assert!(
            $crate::test_util::close(a, b),
            "assertion `left ≈ right` failed: {}\n  left: {a:?}\n right: {b:?}",
            format_args!($($arg)+)
        );
    }};
}
