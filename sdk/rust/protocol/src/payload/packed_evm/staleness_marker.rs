use crate::time::DurationUs;

/// How far behind a `PackedEvm` payload's timestamp one feed's last update is,
/// bucketed into the two bits a word has to spare for it.
///
/// | bits | marker | age |
/// |---|---|---|
/// | `00` | [`Zero`](Self::Zero) | the feed was updated at the payload's timestamp |
/// | `01` | [`OneSecondOrLess`](Self::OneSecondOrLess) | up to and including one second |
/// | `10` | [`FiveSecondsOrLess`](Self::FiveSecondsOrLess) | more than one, up to and including five seconds |
/// | `11` | [`MoreThanFiveSeconds`](Self::MoreThanFiveSeconds) | more than five seconds, or no update to measure |
///
/// Four markers, one per feed of a word, share the word's last byte: see
/// [`pack`](Self::pack). Every two-bit pattern is a marker, so unpacking
/// cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StalenessMarker {
    Zero,
    OneSecondOrLess,
    FiveSecondsOrLess,
    MoreThanFiveSeconds,
}

impl StalenessMarker {
    /// How many markers one byte holds.
    pub const PER_BYTE: usize = 4;

    const BITS: u32 = 2;
    const MASK: u8 = 0b11;

    pub fn max_age(&self) -> Option<DurationUs> {
        match self {
            Self::Zero => Some(DurationUs::ZERO),
            Self::OneSecondOrLess => Some(DurationUs::from_secs_u32(1)),
            Self::FiveSecondsOrLess => Some(DurationUs::from_secs_u32(5)),
            Self::MoreThanFiveSeconds => None,
        }
    }

    pub fn from_age(age: DurationUs) -> Self {
        if age == DurationUs::ZERO {
            Self::Zero
        } else if age <= DurationUs::from_secs_u32(1) {
            Self::OneSecondOrLess
        } else if age <= DurationUs::from_secs_u32(5) {
            Self::FiveSecondsOrLess
        } else {
            Self::MoreThanFiveSeconds
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::OneSecondOrLess => 1,
            Self::FiveSecondsOrLess => 2,
            Self::MoreThanFiveSeconds => 3,
        }
    }

    fn from_bits(bits: u8) -> Self {
        match bits & Self::MASK {
            0 => Self::Zero,
            1 => Self::OneSecondOrLess,
            2 => Self::FiveSecondsOrLess,
            _ => Self::MoreThanFiveSeconds,
        }
    }

    pub fn pack(markers: [Self; Self::PER_BYTE]) -> u8 {
        markers
            .iter()
            .fold(0, |byte, marker| (byte << Self::BITS) | marker.to_bits())
    }

    pub fn unpack(byte: u8) -> [Self; Self::PER_BYTE] {
        [
            Self::from_bits(byte >> (3 * Self::BITS)),
            Self::from_bits(byte >> (2 * Self::BITS)),
            Self::from_bits(byte >> Self::BITS),
            Self::from_bits(byte),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;

    const ALL: [StalenessMarker; 4] = [
        StalenessMarker::Zero,
        StalenessMarker::OneSecondOrLess,
        StalenessMarker::FiveSecondsOrLess,
        StalenessMarker::MoreThanFiveSeconds,
    ];

    fn secs(secs: u32) -> DurationUs {
        DurationUs::from_secs_u32(secs)
    }

    fn micros(micros: u64) -> DurationUs {
        DurationUs::from_micros(micros)
    }

    /// Zero is its own bucket: one microsecond is already "one second or less".
    /// Every other ceiling belongs to its bucket, and a microsecond more tips
    /// into the next one.
    #[test]
    fn from_age_buckets_by_inclusive_ceiling() {
        for (age, expected) in [
            (DurationUs::ZERO, StalenessMarker::Zero),
            (micros(1), StalenessMarker::OneSecondOrLess),
            (micros(999_999), StalenessMarker::OneSecondOrLess),
            (secs(1), StalenessMarker::OneSecondOrLess),
            (micros(1_000_001), StalenessMarker::FiveSecondsOrLess),
            (secs(3), StalenessMarker::FiveSecondsOrLess),
            (secs(5), StalenessMarker::FiveSecondsOrLess),
            (micros(5_000_001), StalenessMarker::MoreThanFiveSeconds),
            (secs(60), StalenessMarker::MoreThanFiveSeconds),
            (micros(u64::MAX), StalenessMarker::MoreThanFiveSeconds),
        ] {
            assert_eq!(StalenessMarker::from_age(age), expected, "{age:?}");
        }
    }

    /// `max_age` and `from_age` agree: a bucket's ceiling maps back to the
    /// bucket, and the ceilings go up in marker order.
    #[test]
    fn max_age_is_the_oldest_age_that_still_maps_to_the_marker() {
        for marker in ALL {
            let Some(max_age) = marker.max_age() else {
                assert_eq!(marker, StalenessMarker::MoreThanFiveSeconds);
                continue;
            };
            assert_eq!(StalenessMarker::from_age(max_age), marker, "{marker:?}");
            assert_ne!(
                StalenessMarker::from_age(micros(max_age.as_micros() + 1)),
                marker,
                "{marker:?}: a microsecond past the ceiling"
            );
        }
        let ceilings: Vec<_> = ALL.iter().map(StalenessMarker::max_age).collect();
        assert_eq!(
            ceilings,
            [Some(DurationUs::ZERO), Some(secs(1)), Some(secs(5)), None]
        );
    }

    /// The first marker lands in the top two bits, the last in the bottom two.
    #[test]
    fn pack_puts_the_first_marker_in_the_most_significant_bits() {
        assert_eq!(StalenessMarker::pack([StalenessMarker::Zero; 4]), 0);
        assert_eq!(
            StalenessMarker::pack([
                StalenessMarker::MoreThanFiveSeconds,
                StalenessMarker::Zero,
                StalenessMarker::Zero,
                StalenessMarker::Zero,
            ]),
            0b11_00_00_00
        );
        assert_eq!(
            StalenessMarker::pack([
                StalenessMarker::Zero,
                StalenessMarker::Zero,
                StalenessMarker::Zero,
                StalenessMarker::OneSecondOrLess,
            ]),
            0b00_00_00_01
        );
        assert_eq!(
            StalenessMarker::pack([
                StalenessMarker::Zero,
                StalenessMarker::OneSecondOrLess,
                StalenessMarker::FiveSecondsOrLess,
                StalenessMarker::MoreThanFiveSeconds,
            ]),
            0b00_01_10_11
        );
        assert_eq!(
            StalenessMarker::pack([StalenessMarker::MoreThanFiveSeconds; 4]),
            0xFF
        );
    }

    /// All 256 combinations of four markers pack to distinct bytes and unpack
    /// to themselves, so every byte is a valid marker byte and no two mean
    /// the same thing.
    #[test]
    fn pack_and_unpack_are_inverses_over_every_byte() {
        let mut seen = [false; 256];
        for combination in
            std::iter::repeat_n(ALL.iter(), StalenessMarker::PER_BYTE).multi_cartesian_product()
        {
            let markers: [StalenessMarker; 4] = combination
                .iter()
                .map(|marker| **marker)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            let byte = StalenessMarker::pack(markers);
            assert!(
                !seen[usize::from(byte)],
                "{markers:?} collides at {byte:#x}"
            );
            seen[usize::from(byte)] = true;
            assert_eq!(StalenessMarker::unpack(byte), markers, "byte {byte:#x}");
        }
        assert!(seen.iter().all(|seen| *seen));
        for byte in 0..=u8::MAX {
            assert_eq!(
                StalenessMarker::pack(StalenessMarker::unpack(byte)),
                byte,
                "byte {byte:#x}"
            );
        }
    }
}
