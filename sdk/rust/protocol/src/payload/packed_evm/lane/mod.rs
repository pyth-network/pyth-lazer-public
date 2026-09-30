//! A packed representation of a price, a confidence and a spread in 6 bytes.

mod codebook_256;
mod packed_price;

use codebook_256::{Code, Codebook256};
use packed_price::{pack_price, U64x32, U27, U5};

/// One feed's slot in a `PackedEvm` word: 48 bits, packed most-significant
/// bit first.
///
/// | bits | field |
/// |---|---|
/// | 5 | exponent, storing `-exponent` unsigned |
/// | 27 | price mantissa |
/// | 8 | `confidence`, [`Codebook256`]-encoded |
/// | 8 | `spread`, [`Codebook256`]-encoded |
///
/// A lane holds one absolute value — the price, as a mantissa and the exponent
/// it is quoted at — and two relative ones, the confidence and the spread.
/// The confidence and the spread are ratios against the price.
///
/// Every 48-bit pattern is a lane, so [`Lane::from_bytes`] cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lane {
    price: U64x32,
    confidence: Code,
    spread: Code,
}

const HUNDREDTHS_BPS_PER_UNIT: i128 = 1_000_000;

impl Lane {
    pub const SIZE: usize = 6;

    /// The lane that says "this feed has no price in this payload".
    pub const SENTINEL: Self = Self {
        price: U64x32 {
            neg_exponent: U5(U5::MAX_VALUE),
            mantissa: U27(U27::MAX_VALUE),
        },
        confidence: Code::Unavailable,
        spread: Code::Unavailable,
    };

    /// Builds a lane from one feed's quote.
    pub fn encode(quote: Quote) -> Option<Self> {
        let Quote {
            price,
            bid,
            ask,
            confidence,
            exponent,
        } = quote;

        let confidence = confidence
            .and_then(|confidence| {
                spread_hundredths_bps(i128::from(confidence), price)
                    .and_then(|spread| Codebook256::encode(spread).map(Code::Index))
            })
            .unwrap_or(Code::Unavailable);

        let spread = bid
            .zip(ask)
            .and_then(|(bid, ask)| i128::from(ask).checked_sub(i128::from(bid)))
            .and_then(|diff| spread_hundredths_bps(diff, price))
            .and_then(|spread| Codebook256::encode(spread).map(Code::Index))
            .unwrap_or(Code::Unavailable);

        Some(Self {
            price: pack_price(price, exponent)?,
            confidence,
            spread,
        })
    }

    pub fn is_unavailable(self) -> bool {
        self.confidence == Code::Unavailable && self.spread == Code::Unavailable
    }

    pub fn price(&self) -> Option<U64x32> {
        if self.is_unavailable() {
            None
        } else {
            Some(self.price)
        }
    }

    pub fn confidence(self) -> Code {
        self.confidence
    }

    pub fn spread(self) -> Code {
        self.spread
    }

    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let packed = (u64::from(self.price.neg_exponent.get()) << 43)
            | (u64::from(self.price.mantissa.get()) << 16)
            | (u64::from(self.confidence.to_byte()) << 8)
            | u64::from(self.spread.to_byte());
        // The packed value is 48 bits wide, so the first two bytes are zero.
        let [_, _, lane @ ..] = packed.to_be_bytes();
        lane
    }

    pub fn from_bytes(lane: [u8; Self::SIZE]) -> Self {
        let [first, second, third, fourth, confidence, spread] = lane;
        Self {
            price: U64x32 {
                // The exponent is the top 5 bits; the mantissa's 27 run from the
                // bottom 3 bits of the same byte through the next three bytes.
                neg_exponent: U5(first >> 3),
                mantissa: U27(u32::from(first & 0b111) << 24
                    | u32::from(second) << 16
                    | u32::from(third) << 8
                    | u32::from(fourth)),
            },
            confidence: Code::from_byte(confidence),
            spread: Code::from_byte(spread),
        }
    }
}

/// One feed's quote, every mantissa at the same `exponent`.
///
/// The fields are separate rather than positional arguments because `bid`,
/// `ask` and `confidence` are all `Option<i64>`: naming them at the call site
/// is what keeps a bid from being passed as an ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Quote {
    pub price: i64,
    pub bid: Option<i64>,
    pub ask: Option<i64>,
    pub confidence: Option<i64>,
    pub exponent: i16,
}

fn spread_hundredths_bps(difference: i128, price: i64) -> Option<i64> {
    if price <= 0 || difference < 0 {
        return None;
    }
    let price = i128::from(price);
    let scaled = difference
        .checked_mul(HUNDREDTHS_BPS_PER_UNIT)?
        .checked_add(price / 2)?;
    i64::try_from(scaled.checked_div(price)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quote around $67134.36, at Lazer's usual crypto exponent.
    ///
    /// Deliberately asymmetric — the confidence, the bid's distance from the
    /// price and the ask's are all different — so a test that reads the wrong
    /// one of them fails rather than coincidentally passing.
    fn typical() -> Quote {
        Quote {
            price: 6_713_436_287_632,
            bid: Some(6_713_017_907_790),
            ask: Some(6_713_596_631_740),
            // 43 hundredths of a bp of the price, which ties between two
            // codebook entries and so rounds up to 44.
            confidence: Some(288_677_760),
            exponent: -8,
        }
    }

    fn encode(quote: Quote) -> Lane {
        Lane::encode(quote).unwrap()
    }

    // -----------------------------------------------------------------------
    // Packing
    // -----------------------------------------------------------------------

    #[test]
    fn the_sentinel_lane_is_six_0xff_bytes() {
        assert_eq!(Lane::SENTINEL.to_bytes(), [0xFF; Lane::SIZE]);
        assert_eq!(Lane::from_bytes([0xFF; Lane::SIZE]), Lane::SENTINEL);
        assert!(Lane::SENTINEL.is_unavailable());
    }

    #[test]
    fn lane_bytes_round_trip_for_every_bit_pattern_that_matters() {
        for pattern in [
            [0x00; Lane::SIZE],
            [0xFF; Lane::SIZE],
            [0x1C, 0x00, 0x63, 0x9B, 0x23, 0x10],
            [0xF8, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0x07, 0xFF, 0xFF, 0xFF, 0x00, 0x00],
        ] {
            assert_eq!(Lane::from_bytes(pattern).to_bytes(), pattern);
        }
    }

    /// The lane's exponent belongs to the price alone.
    ///
    /// Here the ask is one tick over the widest 27-bit mantissa, so the ask could
    /// not be stored at this scale — but nothing stores the ask. The price fits,
    /// so the price keeps its own exponent and its full precision, and the spread
    /// that the ask feeds into is a ratio that has no scale to lose.
    #[test]
    fn an_unstorable_ask_does_not_coarsen_the_price() {
        let lane = encode(Quote {
            price: 134_217_700,
            bid: Some(134_217_600),
            // One over `2^27 - 1`, the widest mantissa a lane can hold.
            ask: Some(134_217_800),
            confidence: Some(1),
            exponent: -8,
        });
        // Exponent 8 in the top 5 bits, then mantissa 134217700 (`0x7ffffe4`)
        // unchanged, then a confidence of 0 and a spread of 1 hundredth of a bp.
        assert_eq!(
            hex::encode(lane.to_bytes()),
            "47ffffe40001",
            "the price kept the exponent it was quoted at"
        );
    }

    /// A lane's two codes read back as codes, so "not reported" is a variant
    /// rather than a byte a caller has to recognise.
    #[test]
    fn lane_codes_read_back_as_confidence_and_spread() {
        let live = Lane::from_bytes(encode(typical()).to_bytes());

        assert_eq!(live.confidence().hundredths_bps(), Some(44));
        assert_eq!(live.spread().hundredths_bps(), Some(86));
        assert!(matches!(live.confidence(), Code::Index(_)));

        assert_eq!(Lane::SENTINEL.confidence(), Code::Unavailable);
        assert_eq!(Lane::SENTINEL.spread(), Code::Unavailable);
        assert_eq!(Lane::SENTINEL.confidence().hundredths_bps(), None);
    }

    // -----------------------------------------------------------------------
    // Only the price can sink a lane
    // -----------------------------------------------------------------------

    /// A confidence or a book that cannot be encoded costs its own byte and
    /// leaves the rest of the lane standing (see
    /// `losing_one_code_leaves_the_other_alone`); a price the format cannot
    /// hold costs the whole lane.
    #[test]
    fn only_a_price_the_format_cannot_hold_fails_the_lane() {
        // A positive exponent is not a scale this format can hold.
        assert_eq!(
            Lane::encode(Quote {
                exponent: 1,
                ..typical()
            }),
            None
        );
        // The lane carries an unsigned price.
        assert_eq!(
            Lane::encode(Quote {
                price: 0,
                ..typical()
            }),
            None
        );
    }

    /// A quote that loses both codes still has a price, and keeps it.
    ///
    /// This is the one case where `Lane::is_unavailable` and `Lane::SENTINEL` come
    /// apart: the lane reports both codes unavailable, but its price is real and a
    /// consumer that threw the lane away on `is_unavailable` alone would be
    /// discarding good data.
    #[test]
    fn a_lane_with_neither_code_still_carries_its_price() {
        let lane = encode(Quote {
            bid: None,
            ask: None,
            confidence: None,
            ..typical()
        });

        assert_eq!(lane.confidence(), Code::Unavailable);
        assert_eq!(lane.spread(), Code::Unavailable);
        assert!(lane.is_unavailable());
        assert_ne!(lane, Lane::SENTINEL);
        // The price is the one the quote carried, packed as it always is.
        assert_eq!(hex::encode(lane.to_bytes()), "1c00639bffff");
    }

    // -----------------------------------------------------------------------
    // The two codes are independent readings
    // -----------------------------------------------------------------------

    /// The confidence comes from the confidence and the spread from the book.
    /// Neither stands in for the other, so losing one leaves the other alone.
    #[test]
    fn losing_one_code_leaves_the_other_alone() {
        let typical = typical();
        let cases = [
            (
                "bid missing",
                Quote {
                    bid: None,
                    ..typical
                },
                None,
                Some(44),
            ),
            (
                "ask missing",
                Quote {
                    ask: None,
                    ..typical
                },
                None,
                Some(44),
            ),
            (
                "both sides missing",
                Quote {
                    bid: None,
                    ask: None,
                    ..typical
                },
                None,
                Some(44),
            ),
            (
                // The book is inverted, so its width is negative and the codebook
                // has no entry for it.
                "book crossed",
                Quote {
                    bid: typical.ask,
                    ask: typical.bid,
                    ..typical
                },
                None,
                Some(44),
            ),
            (
                "confidence missing",
                Quote {
                    confidence: None,
                    ..typical
                },
                Some(86),
                None,
            ),
            (
                // 15% of the price, far past the codebook's 105.28 bps ceiling.
                "confidence past the codebook ceiling",
                Quote {
                    confidence: Some(1_007_015_443_144),
                    ..typical
                },
                Some(86),
                None,
            ),
            (
                // A dollar wide on a $100 price is 100 bps of spread, past the
                // ceiling, but the confidence is untouched.
                "book wider than the codebook can hold",
                Quote {
                    price: 10_000,
                    bid: Some(9_000),
                    ask: Some(10_001),
                    confidence: Some(1),
                    exponent: -2,
                },
                None,
                Some(100),
            ),
        ];

        for (name, quote, spread, confidence) in cases {
            let lane = encode(quote);
            assert_eq!(lane.spread().hundredths_bps(), spread, "{name}: spread");
            assert_eq!(
                lane.confidence().hundredths_bps(),
                confidence,
                "{name}: confidence"
            );
        }
    }

    /// The spread is the width of the whole book. The price sets the scale it is
    /// measured against and nothing else, so moving the price inside the book does
    /// not change it.
    #[test]
    fn the_spread_is_the_book_width_wherever_the_price_sits() {
        let typical = typical();
        let width = |price| {
            encode(Quote {
                price,
                // Hold the scale fixed, so only the book's position moves.
                confidence: None,
                ..typical
            })
            .spread()
            .hundredths_bps()
        };
        // Resting on the bid, in the middle, and resting on the ask all read the
        // same 86 hundredths of a bp.
        assert_eq!(width(typical.bid.unwrap()), Some(86));
        assert_eq!(width(typical.price), Some(86));
        assert_eq!(width(typical.ask.unwrap()), Some(86));
    }
}
