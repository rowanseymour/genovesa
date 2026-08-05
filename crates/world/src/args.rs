//! Reading two-part command-line options.
//!
//! Both binaries in this workspace take options shaped `98,-317`, `2560x1440`
//! or a bare `8192` standing for both halves, and they have to fail alike:
//! the same trim, the same "if either half is nonsense the whole option is",
//! the same message naming what it should have looked like. Saying that once
//! is what keeps them consistent, which is the part a caller notices — an
//! option that rejects `98` on one binary and quietly takes it on the other
//! is worse than either behaviour on its own.
//!
//! It lives in this crate rather than in the game's because `mapgen` ships
//! here, and the game's command line already depends on this crate rather
//! than the other way about.

/// Reads a two-part option as both halves parsed the same way, or one error
/// naming what it should have looked like. The separator is required.
pub fn pair<T>(
    value: &str,
    separator: char,
    axis: impl Fn(&str) -> Option<T>,
    expected: &str,
) -> Result<(T, T), String> {
    let (first, second) = value
        .split_once(separator)
        .ok_or_else(|| format!("`{value}` is not {expected}"))?;
    halves(value, first, second, axis, expected)
}

/// The same, except that a value with no separator in it stands for both
/// halves — `8192` for a square window, `1024` for a square map.
pub fn pair_or_single<T>(
    value: &str,
    separator: char,
    axis: impl Fn(&str) -> Option<T>,
    expected: &str,
) -> Result<(T, T), String> {
    let (first, second) = value.split_once(separator).unwrap_or((value, value));
    halves(value, first, second, axis, expected)
}

fn halves<T>(
    value: &str,
    first: &str,
    second: &str,
    axis: impl Fn(&str) -> Option<T>,
    expected: &str,
) -> Result<(T, T), String> {
    let bad = || format!("`{value}` is not {expected}");
    Ok((
        axis(first.trim()).ok_or_else(bad)?,
        axis(second.trim()).ok_or_else(bad)?,
    ))
}

/// A number of metres that is somewhere.
///
/// `inf` and `nan` both parse happily as floats and neither is a place: a
/// render walks out from the point it is given, so a non-finite one samples
/// the world at non-finite coordinates and writes a picture of nothing,
/// having said nothing about it.
pub fn metres(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|m| m.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(value: &str) -> Result<(f32, f32), String> {
        pair(value, ',', metres, "a point, e.g. 2000,-3000")
    }

    #[test]
    fn both_halves_are_parsed_and_trimmed() {
        assert_eq!(point("2000,-3000"), Ok((2000.0, -3000.0)));
        assert_eq!(point(" 2000 , -3000 "), Ok((2000.0, -3000.0)));
    }

    #[test]
    fn either_half_being_nonsense_fails_the_whole_option() {
        for bad in ["2000", "north,south", "1,2,3", "inf,0", "0,nan", "-inf,0"] {
            let error = point(bad).expect_err("should be refused");
            assert!(
                error.contains(bad) && error.contains("a point"),
                "`{bad}` gave an error that names neither it nor what was wanted: {error}"
            );
        }
    }

    #[test]
    fn a_single_value_can_stand_for_both_halves() {
        let span = |v: &str| pair_or_single(v, 'x', metres, "a span");
        assert_eq!(span("8192"), Ok((8192.0, 8192.0)));
        assert_eq!(span("8192x4096"), Ok((8192.0, 4096.0)));
        assert!(span("8192xzero").is_err());
    }
}
