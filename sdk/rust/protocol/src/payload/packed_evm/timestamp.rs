use crate::time::TimestampUs;
use thiserror::Error;

/// The other half of a `PackedEvm` word: a Unix microsecond timestamp in
/// 56 bits.
///
/// Microseconds are what `TimestampUs` carries everywhere else in the protocol,
/// and 56 bits of them run to roughly the year 4254.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimestampUs56(TimestampUs);

/// A timestamp too large for [`TimestampUs56`], carrying the offending value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("timestamp {} us does not fit the 56 bits a PackedEvm word holds", .0.as_micros())]
pub struct TimestampTooLarge(pub TimestampUs);

impl TimestampUs56 {
    /// Serialised width, in bytes.
    pub const SIZE: usize = 7;

    /// The widest timestamp the field holds.
    pub const MAX: Self = Self(TimestampUs::from_micros((1 << 56) - 1));

    pub fn new(timestamp_us: TimestampUs) -> Result<Self, TimestampTooLarge> {
        if timestamp_us > Self::MAX.0 {
            return Err(TimestampTooLarge(timestamp_us));
        }
        Ok(Self(timestamp_us))
    }

    pub fn get(&self) -> TimestampUs {
        self.0
    }

    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        // `new` refused anything wider, so the high byte is zero.
        let [_, timestamp_us @ ..] = self.0.as_micros().to_be_bytes();
        timestamp_us
    }

    /// Seven bytes always fit, so this cannot fail.
    pub fn from_bytes(timestamp_us: [u8; Self::SIZE]) -> Self {
        let [a, b, c, d, e, f, g] = timestamp_us;
        Self(TimestampUs::from_micros(u64::from_be_bytes([
            0, a, b, c, d, e, f, g,
        ])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field is 56 bits. A timestamp that needs more is refused at
    /// construction rather than truncated, so a payload can never quietly claim
    /// a time it was not built at.
    #[test]
    fn a_timestamp_too_wide_for_the_field_is_rejected() {
        let too_wide = TimestampUs::from_micros(1 << 56);
        assert_eq!(
            TimestampUs56::new(too_wide),
            Err(TimestampTooLarge(too_wide))
        );
    }

    /// The boundary itself is fine: `2^56 - 1` is the widest the field holds,
    /// and it fills every byte.
    #[test]
    fn the_widest_timestamp_the_field_holds_is_accepted() {
        let widest = TimestampUs::from_micros((1 << 56) - 1);
        assert_eq!(TimestampUs56::new(widest).unwrap(), TimestampUs56::MAX);
        assert_eq!(TimestampUs56::MAX.to_bytes(), [0xFF; TimestampUs56::SIZE]);
    }

    /// Seven bytes always fit, so reading a timestamp back can never fail.
    #[test]
    fn every_seven_byte_timestamp_round_trips() {
        for bytes in [
            [0; TimestampUs56::SIZE],
            [0xFF; TimestampUs56::SIZE],
            [0x06, 0x4b, 0x06, 0x15, 0xd1, 0x77, 0x40],
            [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ] {
            assert_eq!(TimestampUs56::from_bytes(bytes).to_bytes(), bytes);
        }
        assert_eq!(
            TimestampUs56::from_bytes([0x06, 0x4b, 0x06, 0x15, 0xd1, 0x77, 0x40]).get(),
            TimestampUs::from_micros(1_771_339_368_200_000)
        );
    }
}
