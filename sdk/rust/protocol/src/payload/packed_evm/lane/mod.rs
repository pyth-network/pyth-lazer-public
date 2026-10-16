//! A packed representation of a price and its relative confidence in 6 bytes.

mod confidence;
mod packed_price;

use packed_price::pack_price;

pub use {
    confidence::Confidence,
    packed_price::{PackedPrice, U64x27},
};

/// One feed's slot in a `PackedEvm` word: 48 bits, packed most-significant
/// bit first.
///
/// | bits | field |
/// |---|---|
/// | 5 | exponent, storing `-exponent` unsigned |
/// | 27 | price mantissa |
/// | 16 | [`Confidence`]: hundredths of a bp of the price, `0xFFFF` if unavailable |
///
/// A lane holds one absolute value — the price, as a mantissa and the exponent
/// it is quoted at — and one relative one, the confidence, which is a ratio
/// against the price.
///
/// Every 48-bit pattern is a lane, so [`Lane::from_bytes`] cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lane {
    price: PackedPrice,
    confidence: Confidence,
}

/// Hundredths of a basis point in one unit of the price.
const HUNDREDTHS_BPS_PER_UNIT: i128 = 1_000_000;

impl Lane {
    pub const SIZE: usize = 6;

    /// The lane that says "this feed has no price in this payload".
    pub const UNAVAILABLE: Self = Self {
        price: PackedPrice::Unavailable,
        confidence: Confidence::Unavailable,
    };

    /// Builds a lane from one feed's quote.
    pub fn encode(quote: Quote) -> Self {
        let Quote {
            price,
            confidence,
            exponent,
        } = quote;

        let confidence = confidence
            .and_then(|confidence| relative_hundredths_bps(i128::from(confidence), price))
            .map(Confidence::new)
            .unwrap_or(Confidence::Unavailable);

        Self {
            price: pack_price(price, exponent),
            confidence,
        }
    }

    pub fn is_unavailable(self) -> bool {
        self == Self::UNAVAILABLE
    }

    pub fn price(&self) -> PackedPrice {
        self.price
    }

    pub fn confidence(self) -> Confidence {
        self.confidence
    }

    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let packed = u64::from(u32::from_be_bytes(self.price.to_bytes())) << 16
            | u64::from(u16::from_be_bytes(self.confidence.to_bytes()));
        // The packed value is 48 bits wide, so the first two bytes are zero.
        let [_, _, lane @ ..] = packed.to_be_bytes();
        lane
    }

    pub fn from_bytes(lane: [u8; Self::SIZE]) -> Self {
        let [first, second, third, fourth, fifth, sixth] = lane;
        Self {
            price: PackedPrice::from_bytes([first, second, third, fourth]),
            confidence: Confidence::from_bytes([fifth, sixth]),
        }
    }
}

/// One feed's quote, both mantissas at the same `exponent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Quote {
    pub price: i64,
    pub confidence: Option<i64>,
    pub exponent: i16,
}

fn relative_hundredths_bps(part: i128, price: i64) -> Option<u16> {
    if price <= 0 || part < 0 {
        return None;
    }
    let price = i128::from(price);
    let scaled = part
        .checked_mul(HUNDREDTHS_BPS_PER_UNIT)?
        .checked_add(price / 2)?;
    u16::try_from(scaled.checked_div(price)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quote around $67134.36, at Lazer's usual crypto exponent.
    fn typical() -> Quote {
        Quote {
            price: 6_713_436_287_632,
            // 43 hundredths of a bp of the price, give or take a rounding error
            // far below half of one.
            confidence: Some(288_677_760),
            exponent: -8,
        }
    }

    /// `typical()`'s lane: exponent `-3` and mantissa `67134363` in the first
    /// four bytes, then confidence `43` (`0x002b`).
    const TYPICAL_LANE_HEX: &str = "1c00639b002b";

    fn encode(quote: Quote) -> Lane {
        Lane::encode(quote)
    }

    fn confidence_of(quote: Quote) -> Option<u16> {
        encode(quote).confidence().hundredths_bps()
    }

    // -----------------------------------------------------------------------
    // Packing
    // -----------------------------------------------------------------------

    #[test]
    fn the_sentinel_lane_is_six_0xff_bytes() {
        assert_eq!(Lane::UNAVAILABLE.to_bytes(), [0xFF; Lane::SIZE]);
        assert_eq!(Lane::from_bytes([0xFF; Lane::SIZE]), Lane::UNAVAILABLE);
        assert!(Lane::UNAVAILABLE.is_unavailable());
    }

    #[test]
    fn lane_bytes_round_trip_for_every_bit_pattern_that_matters() {
        for pattern in [
            [0x00; Lane::SIZE],
            [0xFF; Lane::SIZE],
            // `typical()`.
            [0x1C, 0x00, 0x63, 0x9B, 0x00, 0x2B],
            // The widest exponent, a mantissa of zero and a confidence of zero.
            [0xF8, 0x00, 0x00, 0x00, 0x00, 0x00],
            // Exponent zero, the widest mantissa and the widest confidence.
            [0x07, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE],
            // An available price with no confidence.
            [0x1C, 0x00, 0x63, 0x9B, 0xFF, 0xFF],
            // No price, but a confidence: not the sentinel lane.
            [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x2B],
        ] {
            assert_eq!(Lane::from_bytes(pattern).to_bytes(), pattern);
        }
    }

    /// The price takes the first four bytes and the confidence the last two,
    /// each big-endian.
    #[test]
    fn the_price_and_the_confidence_each_have_their_own_bytes() {
        let lane = encode(typical());
        assert_eq!(hex::encode(lane.to_bytes()), TYPICAL_LANE_HEX);
        assert_eq!(lane.price(), pack_price(6_713_436_287_632, -8));
        assert_eq!(lane.price().to_bytes(), [0x1C, 0x00, 0x63, 0x9B]);
        assert_eq!(lane.confidence(), Confidence::Available(43));
        assert_eq!(lane.confidence().to_bytes(), [0x00, 0x2B]);
        assert_eq!(Lane::from_bytes(lane.to_bytes()), lane);
    }

    /// "Not reported" is a variant, not a byte a caller has to recognise.
    #[test]
    fn the_confidence_reads_back_as_a_variant() {
        let live = Lane::from_bytes(encode(typical()).to_bytes());
        assert_eq!(live.confidence(), Confidence::Available(43));
        assert_eq!(live.confidence().hundredths_bps(), Some(43));

        assert_eq!(Lane::UNAVAILABLE.confidence(), Confidence::Unavailable);
        assert_eq!(Lane::UNAVAILABLE.confidence().hundredths_bps(), None);
    }

    #[test]
    fn a_price_the_format_cannot_hold_only_fails_the_price() {
        // A positive exponent is not a scale this format can hold.
        let postive_exponent_lane = encode(Quote {
            exponent: 1,
            ..typical()
        });
        assert_eq!(postive_exponent_lane.price(), PackedPrice::Unavailable);
        assert_eq!(
            postive_exponent_lane.confidence(),
            Confidence::Available(43),
            "the confidence is a ratio, so the exponent does not enter into it"
        );
        assert!(!postive_exponent_lane.is_unavailable());
        assert_eq!(
            hex::encode(postive_exponent_lane.to_bytes()),
            "ffffffff002b"
        );

        // Roundtrip the lane
        assert_eq!(
            Lane::from_bytes(postive_exponent_lane.to_bytes()),
            postive_exponent_lane
        );
    }

    /// A quote without a confidence still has a price, and keeps it.
    #[test]
    fn a_lane_without_a_confidence_still_carries_its_price() {
        let lane = encode(Quote {
            confidence: None,
            ..typical()
        });

        assert_eq!(lane.confidence(), Confidence::Unavailable);
        assert!(!lane.is_unavailable());
        assert_ne!(lane, Lane::UNAVAILABLE);
        // The price is the one the quote carried, packed as it always is.
        assert_eq!(hex::encode(lane.to_bytes()), "1c00639bffff");
    }

    /// A price that is not positive is no scale to measure a confidence
    /// against, so it takes the confidence down with it and the lane is the
    /// sentinel.
    #[test]
    fn a_price_that_is_not_positive_sinks_the_whole_lane() {
        for price in [0, -1, i64::MIN] {
            let lane = encode(Quote { price, ..typical() });
            assert_eq!(lane, Lane::UNAVAILABLE, "price {price}");
        }
    }

    // -----------------------------------------------------------------------
    // The confidence is a ratio against the price
    // -----------------------------------------------------------------------

    /// The confidence is measured against the price, not stored in the price's
    /// units, so the exponent they share cancels out.
    #[test]
    fn the_confidence_does_not_depend_on_the_exponent() {
        // One cent on $100, quoted at three different scales: a hundredth of a
        // percent, so 100 hundredths of a bp every time.
        let at_exponent = |exponent: i16, price: i64, confidence: i64| {
            encode(Quote {
                price,
                confidence: Some(confidence),
                exponent,
            })
        };
        let cents = at_exponent(-2, 10_000, 1);
        let micros = at_exponent(-6, 100_000_000, 10_000);
        let lazer = at_exponent(-8, 10_000_000_000, 1_000_000);

        for lane in [cents, micros, lazer] {
            assert_eq!(lane.confidence(), Confidence::Available(100));
        }
        // The prices are the same value at different scales, and the lane
        // keeps the scale each was quoted at while it fits.
        assert_eq!(cents.price(), pack_price(10_000, -2));
        assert_eq!(micros.price(), pack_price(100_000_000, -6));
        assert_ne!(cents.price(), micros.price());
    }

    /// The confidence has no mantissa field of its own to overflow: a
    /// confidence wider than the 27 bits a price gets is still just a ratio.
    #[test]
    fn a_confidence_wider_than_the_mantissa_field_is_still_a_ratio() {
        let lane = encode(Quote {
            // $100.
            price: 10_000_000_000,
            // $2, which as a mantissa is past the 2^27 - 1 ceiling a price
            // mantissa has, and as a ratio is 2%.
            confidence: Some(200_000_000),
            exponent: -8,
        });
        assert_eq!(lane.confidence(), Confidence::Available(20_000));
        assert_eq!(lane.price(), pack_price(10_000_000_000, -8));
    }

    /// Doubling the price halves the confidence, and doubling both leaves it
    /// where it was.
    #[test]
    fn the_confidence_scales_with_the_price() {
        let typical = typical();
        let twice_the_price = Quote {
            price: typical.price * 2,
            ..typical
        };
        let twice_both = Quote {
            price: typical.price * 2,
            confidence: typical.confidence.map(|confidence| confidence * 2),
            ..typical
        };
        assert_eq!(confidence_of(typical), Some(43));
        // The exact ratio is a hair under 43, so its half is a hair under 21.5
        // and rounds down.
        assert_eq!(confidence_of(twice_the_price), Some(21));
        assert_eq!(confidence_of(twice_both), Some(43));
    }

    #[test]
    fn the_confidence_rounds_half_up_to_a_whole_hundredth_of_a_bp() {
        // At a price of 4_000_000 each unit of confidence is a quarter of a
        // hundredth of a bp.
        let quarters = |confidence: i64| {
            confidence_of(Quote {
                price: 4_000_000,
                confidence: Some(confidence),
                exponent: -8,
            })
        };
        assert_eq!(quarters(0), Some(0), "0");
        assert_eq!(quarters(1), Some(0), "0.25 rounds down");
        assert_eq!(quarters(2), Some(1), "0.5 rounds up");
        assert_eq!(quarters(3), Some(1), "0.75 rounds up");
        assert_eq!(quarters(5), Some(1), "1.25 rounds down");
        assert_eq!(quarters(6), Some(2), "1.5 rounds up");
    }

    /// The field tops out one short of the sentinel. A confidence that rounds
    /// to the sentinel or beyond is not reported, not clamped.
    #[test]
    fn a_confidence_past_the_field_ceiling_is_unavailable() {
        // At a price of 1_000_000 the confidence mantissa is the value in
        // hundredths of a bp, so the limits can be hit exactly.
        let exact = |confidence: i64| {
            confidence_of(Quote {
                price: 1_000_000,
                confidence: Some(confidence),
                exponent: -8,
            })
        };
        assert_eq!(exact(65_534), Some(65_534), "the widest value, 6.5534%");
        assert_eq!(exact(65_535), None, "the sentinel is not a value");
        assert_eq!(exact(65_536), None, "one past the field");
        assert_eq!(exact(1_000_000), None, "100% of the price");
        assert_eq!(exact(i64::MAX), None, "far past the field");

        // At a price of 2_000_000 the widest value's rounding is tested: 65_534.5
        // rounds up onto the sentinel, so it is unavailable too.
        assert_eq!(
            confidence_of(Quote {
                price: 2_000_000,
                confidence: Some(131_069),
                exponent: -8,
            }),
            None,
            "rounds up onto the sentinel"
        );
        assert_eq!(
            confidence_of(Quote {
                price: 2_000_000,
                confidence: Some(131_068),
                exponent: -8,
            }),
            Some(65_534)
        );
    }

    /// The price is unsigned, so the confidence is too: a negative one is not
    /// reported rather than being read as its absolute value.
    #[test]
    fn a_negative_confidence_is_unavailable() {
        for confidence in [-1, i64::MIN] {
            let lane = encode(Quote {
                confidence: Some(confidence),
                ..typical()
            });
            assert_eq!(lane.confidence(), Confidence::Unavailable, "{confidence}");
            assert_eq!(lane.price(), pack_price(6_713_436_287_632, -8));
        }
    }

    /// Losing the confidence leaves the price alone, and the other way round.
    #[test]
    fn losing_one_field_leaves_the_other_alone() {
        let typical = typical();
        let cases = [
            (
                "confidence missing",
                Quote {
                    confidence: None,
                    ..typical
                },
                pack_price(typical.price, typical.exponent),
                Confidence::Unavailable,
            ),
            (
                // 15% of the price, far past the 6.5534% ceiling.
                "confidence past the ceiling",
                Quote {
                    confidence: Some(1_007_015_443_144),
                    ..typical
                },
                pack_price(typical.price, typical.exponent),
                Confidence::Unavailable,
            ),
            (
                "price at a scale the lane cannot hold",
                Quote {
                    exponent: 1,
                    ..typical
                },
                PackedPrice::Unavailable,
                Confidence::Available(43),
            ),
        ];

        for (name, quote, price, confidence) in cases {
            let lane = encode(quote);
            assert_eq!(lane.price(), price, "{name}: price");
            assert_eq!(lane.confidence(), confidence, "{name}: confidence");
        }
    }
}
