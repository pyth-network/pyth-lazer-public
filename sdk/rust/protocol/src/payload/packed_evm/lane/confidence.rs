//! A feed's confidence relative to its price, in 2 bytes.

/// A feed's confidence as a ratio against its price, in hundredths of a basis
/// point (`1e-6` of the price), stored big-endian in the last two bytes of a
/// lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// The feed reported no confidence, or one the field cannot hold.
    Unavailable,
    /// The confidence, in hundredths of a basis point of the price. At most
    /// [`Confidence::MAX_HUNDREDTHS_BPS`].
    Available(u16),
}

impl Confidence {
    /// The widest confidence the field can hold: one short of the sentinel. It corresponds to 6.5534% of the price.
    pub const MAX_HUNDREDTHS_BPS: u16 = u16::MAX - 1;

    pub fn new(hundredths_bps: u16) -> Self {
        if hundredths_bps == u16::MAX {
            Self::Unavailable
        } else {
            Self::Available(hundredths_bps)
        }
    }

    pub fn from_bytes(bytes: [u8; 2]) -> Self {
        Self::new(u16::from_be_bytes(bytes))
    }

    pub fn to_bytes(self) -> [u8; 2] {
        match self {
            Self::Unavailable => [u8::MAX, u8::MAX],
            Self::Available(hundredths_bps) => hundredths_bps.to_be_bytes(),
        }
    }

    pub fn hundredths_bps(self) -> Option<u16> {
        match self {
            Self::Unavailable => None,
            Self::Available(hundredths_bps) => Some(hundredths_bps),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentinel_is_two_0xff_bytes_and_is_not_a_value() {
        assert_eq!(Confidence::Unavailable.to_bytes(), [0xFF, 0xFF]);
        assert_eq!(
            Confidence::from_bytes([0xFF, 0xFF]),
            Confidence::Unavailable
        );
        assert_eq!(Confidence::new(u16::MAX), Confidence::Unavailable);
        assert_eq!(Confidence::Unavailable.hundredths_bps(), None);
    }

    #[test]
    fn the_widest_value_is_one_short_of_the_sentinel() {
        assert_eq!(Confidence::MAX_HUNDREDTHS_BPS, 65_534);
        let widest = Confidence::new(Confidence::MAX_HUNDREDTHS_BPS);
        assert_eq!(widest, Confidence::Available(65_534));
        assert_eq!(widest.to_bytes(), [0xFF, 0xFE]);
        assert_eq!(widest.hundredths_bps(), Some(65_534));
    }

    #[test]
    fn values_are_stored_big_endian_and_round_trip() {
        for (value, bytes) in [
            (0, [0x00, 0x00]),
            (43, [0x00, 0x2B]),
            (0x1234, [0x12, 0x34]),
        ] {
            let confidence = Confidence::new(value);
            assert_eq!(confidence, Confidence::Available(value));
            assert_eq!(confidence.to_bytes(), bytes, "{value}");
            assert_eq!(Confidence::from_bytes(bytes), confidence, "{value}");
            assert_eq!(confidence.hundredths_bps(), Some(value), "{value}");
        }
        for value in 0..=u16::MAX {
            let confidence = Confidence::new(value);
            assert_eq!(Confidence::from_bytes(confidence.to_bytes()), confidence);
        }
    }
}
