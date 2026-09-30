//! A packed representation of a price where mantissa and exponent are packed into 4 bytes.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct U5(pub(super) u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct U27(pub(super) u32);

impl U5 {
    pub(super) const MAX_VALUE: u8 = (1 << 5) - 1;

    fn new(value: u16) -> Option<Self> {
        let value = u8::try_from(value).ok()?;
        (value <= Self::MAX_VALUE).then_some(Self(value))
    }

    pub fn get(&self) -> u8 {
        self.0
    }
}

impl U27 {
    pub(super) const MAX_VALUE: u32 = (1 << 27) - 1;

    fn new(value: i64) -> Option<Self> {
        let value = u32::try_from(value).ok()?;
        (value <= Self::MAX_VALUE).then_some(Self(value))
    }

    pub fn get(&self) -> u32 {
        self.0
    }
}

enum RoundingMethod {
    Up,
    Down,
    NearestEven,
    Unnecessary,
}

impl RoundingMethod {
    fn update_from_remainder(self, remainder: i64) -> Option<Self> {
        match (self, remainder) {
            (_, 1..=4) => Some(Self::Down),
            (_, 6..=9) => Some(Self::Up),
            (RoundingMethod::Unnecessary, 0) => Some(RoundingMethod::Unnecessary),
            (RoundingMethod::Unnecessary, 5) => Some(RoundingMethod::NearestEven),
            (_, 5) => Some(Self::Up),
            (_, 0) => Some(Self::Down),
            (_, _) => None,
        }
    }
}

impl RoundingMethod {
    /// These saturating should not be a problem because the target <= i64::MAX / 10 in the algorithm below
    fn round(&self, target: i64) -> i64 {
        match self {
            RoundingMethod::Up => target.saturating_add(1),
            RoundingMethod::Down => target,
            RoundingMethod::NearestEven => {
                if target % 2 == 0 {
                    target
                } else {
                    target.saturating_add(1)
                }
            }
            RoundingMethod::Unnecessary => target,
        }
    }
}

/// A Lazer price re-normalised to fit a `PackedEvm` lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct U64x32 {
    pub(super) neg_exponent: U5,
    pub(super) mantissa: U27,
}

impl U64x32 {
    pub fn neg_exponent(&self) -> U5 {
        self.neg_exponent
    }

    pub fn mantissa(&self) -> U27 {
        self.mantissa
    }
}

pub fn pack_price(mantissa: i64, exponent: i16) -> Option<U64x32> {
    if mantissa <= 0 || exponent > 0 {
        return None;
    }

    // mantissa * divisor + remainder is an invariant of the loop
    let (mut mantissa, mut exponent) = (mantissa, exponent);
    let mut rounding_method = RoundingMethod::Unnecessary;

    loop {
        if let (Some(neg_exponent), Some(mantissa)) = (
            U5::new(exponent.unsigned_abs()),
            U27::new(rounding_method.round(mantissa)),
        ) {
            return Some(U64x32 {
                neg_exponent,
                mantissa,
            });
        }
        let remainder = mantissa % 10;
        (mantissa, exponent) = (mantissa / 10, exponent.checked_add(1)?); // can't overflow because exponent is always zero or negative

        rounding_method = rounding_method.update_from_remainder(remainder)?;

        if mantissa == 0 || exponent > 0 {
            // If mantissa is 0, the relative error is going to be up to a 100% of the original value, bail
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the packed price is within one third relative error of the
    /// input, measuring in the input's own units.
    #[track_caller]
    fn assert_packs_within_one_third(mantissa: i64, exponent: i16) {
        let packed = pack_price(mantissa, exponent)
            .unwrap_or_else(|| panic!("{mantissa}e{exponent} did not pack"));
        assert!(packed.mantissa.0 <= U27::MAX_VALUE);
        let packed_exponent = -i16::from(packed.neg_exponent.0);
        assert!(
            packed_exponent >= exponent,
            "the exponent must only walk up"
        );
        assert!(packed_exponent >= -31);

        let shift = u32::try_from(packed_exponent - exponent).unwrap();
        let expanded = i128::from(packed.mantissa.0) * 10_i128.pow(shift);
        let error = (expanded - i128::from(mantissa)).abs();
        // error / |mantissa| <= 1/3, kept in integers to avoid float rounding.
        let max_error = i128::from(mantissa).abs();
        eprintln!(
            "{mantissa}e{exponent} -> {}e{packed_exponent}: error {error}, relative error {:.4}",
            packed.mantissa.0,
            error as f64 / max_error as f64
        );
        assert!(
            error * 3 <= max_error,
            "{mantissa}e{exponent} became {}e{packed_exponent}, off by {error} (more than one third)",
            packed.mantissa.0
        );
    }

    #[test]
    fn pack_price_round_trips_realistic_prices() {
        // BTC around 100_000 at the exponent Lazer actually publishes it with.
        assert_packs_within_one_third(10_432_178_900_000, -8);
        // ETH around 3_500.
        assert_packs_within_one_third(352_167_450_000, -8);
        // A small-value token, at the most precise configured exponent.
        assert_packs_within_one_third(1_234_567, -12);
        // A small-value token that still needs several reductions.
        assert_packs_within_one_third(987_654_321_098, -12);
        // Equity-scale feeds.
        assert_packs_within_one_third(12_345_678, -5);
        assert_packs_within_one_third(1_234_567, -3);
        // Already small enough to pass straight through.
        assert_packs_within_one_third(1, -12);
    }

    #[test]
    fn pack_price_handles_extreme_values() {
        assert_packs_within_one_third(i64::MAX, -49);
        assert_packs_within_one_third(i64::MAX / 2, -49);
        assert_packs_within_one_third(5 * 10_i64.pow(18) + 1, -49);
        assert_packs_within_one_third(15 * 10_i64.pow(17) - 1, -49);
        assert_packs_within_one_third(15 * 10_i64.pow(17), -49);
    }

    #[test]
    fn pack_price_keeps_a_price_that_already_fits() {
        assert_eq!(
            pack_price(1_234_567, -12),
            Some(U64x32 {
                neg_exponent: U5::new(12).unwrap(),
                mantissa: U27::new(1_234_567).unwrap()
            })
        );
    }

    #[test]
    fn pack_price_walks_the_exponent_up_to_the_representable_range() {
        // Exponent -35 is outside the 5-bit field, so the mantissa has to come up
        // four decimal places even though it already fits in 27 bits.
        assert_eq!(
            pack_price(123_456_789_012, -35),
            Some(U64x32 {
                neg_exponent: U5::new(31).unwrap(),
                mantissa: U27::new(12_345_679).unwrap()
            })
        );
    }

    #[test]
    fn pack_price_rejects_a_price_that_cannot_fit_at_exponent_zero() {
        // The literals pin the 27-bit field width itself: 2^27 - 1 is the largest
        // mantissa a lane can hold, and 2^27 is one too many.
        assert_eq!(U27::MAX_VALUE, 134_217_727);
        assert_eq!(
            pack_price(134_217_727, 0),
            Some(U64x32 {
                neg_exponent: U5::new(0).unwrap(),
                mantissa: U27::new(134_217_727).unwrap()
            })
        );
        assert_eq!(pack_price(134_217_728, 0), None);
        assert_eq!(pack_price(i64::MAX, 0), None);
        assert_eq!(pack_price(i64::MAX, -1), None);
    }

    #[test]
    fn pack_price_rejects_prices_it_cannot_represent() {
        // The lane carries an unsigned price.
        assert_eq!(pack_price(0, -8), None);
        assert_eq!(pack_price(-1, -8), None);
        assert_eq!(pack_price(i64::MIN, -8), None);
        // The exponent field cannot hold a positive exponent, and walking toward
        // zero never reaches one.
        assert_eq!(pack_price(1, 1), None);
        // Rounding consumes the whole price on the way up to the representable
        // exponent range, leaving nothing to report.
        assert_eq!(pack_price(4, -40), None);
    }

    /// Half-up and half-to-even only disagree when the discarded digit is exactly
    /// 5, so both directions have to appear here. Under half-up the first case
    /// would round to 134_217_725 and this test would fail.
    #[test]
    fn pack_price_rounds_halves_to_even() {
        // 134_217_724.5 -> 134_217_724: the tie rounds down, to the even digit.
        assert_eq!(
            pack_price(1_342_177_245, -1),
            Some(U64x32 {
                neg_exponent: U5::new(0).unwrap(),
                mantissa: U27::new(134_217_724).unwrap()
            })
        );
        // 134_217_723.5 -> 134_217_724: the tie rounds up, to the even digit.
        assert_eq!(
            pack_price(1_342_177_235, -1),
            Some(U64x32 {
                neg_exponent: U5::new(0).unwrap(),
                mantissa: U27::new(134_217_724).unwrap()
            })
        );
    }

    /// A discarded digit above 5 rounds up even when an earlier discarded digit
    /// was below it: only the digit nearest the kept ones decides.
    #[test]
    fn pack_price_rounds_on_the_nearest_discarded_digit() {
        assert_eq!(
            pack_price(1_342_177_349, -2),
            Some(U64x32 {
                neg_exponent: U5::new(0).unwrap(),
                mantissa: U27::new(13_421_773).unwrap()
            })
        );
    }
}
