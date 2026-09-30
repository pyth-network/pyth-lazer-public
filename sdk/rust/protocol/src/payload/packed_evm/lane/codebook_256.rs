//! A way to encode a number between 0 and 105.28 bps into a 8-bit index.

struct Segment {
    /// Table index of the segment's first entry.
    first_index: usize,
    /// Table index of the segment's last entry (inclusive).
    last_index: usize,
    /// Value stored at `first_index`, in 0.01 bps.
    first_value: u16,
    /// Increment between consecutive entries, in 0.01 bps.
    step: u16,
}

/// Piecewise definition of the codebook.
///
/// This is the generator for [`TABLE`]. It reproduces, segment for segment,
/// the table baked into the on-chain `Codebook256` Solidity library; the
/// `table_matches_canonical_solidity_blob` test pins the two together.
const SEGMENTS: [Segment; 5] = [
    Segment {
        first_index: 0,
        last_index: 9,
        first_value: 0,
        step: 1,
    },
    Segment {
        first_index: 10,
        last_index: 63,
        first_value: 12,
        step: 2,
    },
    Segment {
        first_index: 64,
        last_index: 127,
        first_value: 123,
        step: 5,
    },
    Segment {
        first_index: 128,
        last_index: 191,
        first_value: 448,
        step: 10,
    },
    Segment {
        first_index: 192,
        last_index: 254,
        first_value: 1228,
        step: 150,
    },
];

/// The codebook, in units of 0.01 bps.
const TABLE: [u16; Codebook256::TABLE_LEN] = build_table();

#[allow(clippy::indexing_slicing)] // This will panic at compile time if there are out of bounds accesses
const fn build_table() -> [u16; Codebook256::TABLE_LEN] {
    let mut table = [0; Codebook256::TABLE_LEN];
    let mut segment = 0;
    while segment < SEGMENTS.len() {
        let Segment {
            first_index,
            last_index,
            first_value,
            step,
        } = SEGMENTS[segment];
        let mut index = first_index;
        let mut value = first_value;
        while index <= last_index {
            table[index] = value;
            value += step;
            index += 1;
        }
        segment += 1;
    }
    table
}

/// The 256-entry spread codebook, in units of 0.01 bps.
///
/// An on-chain decoder reads the same table out of the `Codebook256` Solidity
/// library, so the entries are a cross-language contract: they cannot change
/// without breaking an already-deployed decoder.
pub struct Codebook256;

impl Codebook256 {
    /// Number of entries in the codebook.
    pub const TABLE_LEN: usize = 255;

    /// The byte a lane carries for [`Code::Unavailable`]: the one value of the
    /// 256 that is not an index into the 255-entry table.
    pub const UNAVAILABLE: u8 = u8::MAX;

    /// Largest value the codebook can encode, in 0.01 bps — 105.28 bps.
    pub const MAX_VALUE: u16 = TABLE[Self::TABLE_LEN - 1];

    /// Looks the index up in the table, in units of 0.01 bps.
    pub fn decode(index: u8) -> Option<u16> {
        TABLE.get(usize::from(index)).copied()
    }

    /// Finds the index whose entry is closest to `value_hundredths_bps`.
    ///
    /// The return type is a [`CodebookIndex`], so this cannot hand back the
    /// reserved index and be mistaken for a spread.
    pub fn encode(value_hundredths_bps: i64) -> Option<CodebookIndex> {
        let value = u16::try_from(value_hundredths_bps).ok()?;
        if value > Self::MAX_VALUE {
            return None;
        }
        // `value <= MAX_VALUE`, the last encodable entry, so this lands in range.
        let upper = TABLE.partition_point(|&entry| entry < value);
        let upper_value = *TABLE.get(upper)?;

        let index = if upper == 0 {
            0
        } else {
            let lower_value = *TABLE.get(upper.checked_sub(1)?)?;

            if value.checked_sub(lower_value)? < upper_value.checked_sub(value)? {
                upper.checked_sub(1)?
            } else {
                upper
            }
        };

        CodebookIndex::new(u8::try_from(index).ok()?)
    }
}

/// An index into [`Codebook256`]: always one of the table's entries, never the
/// reserved byte.
///
/// The entry is read once, when the index is built, so there is no later
/// lookup that could miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodebookIndex {
    index: u8,
    hundredths_bps: u16,
}

impl CodebookIndex {
    /// Some when `byte` indexes the table, None for [`Codebook256::UNAVAILABLE`].
    pub fn new(byte: u8) -> Option<Self> {
        Some(Self {
            index: byte,
            hundredths_bps: Codebook256::decode(byte)?,
        })
    }

    pub fn get(self) -> u8 {
        self.index
    }

    /// The spread this index stands for, in units of 0.01 bps.
    pub fn hundredths_bps(self) -> u16 {
        self.hundredths_bps
    }
}

/// What one of a lane's two spread bytes says.
///
/// The byte has 256 values but the codebook has 255 entries, so one value is
/// spare and means "no spread". Spelling that out here rather than leaving the
/// field a `u8` puts the reserved value in the type system: a reader has to say
/// what it does about an unavailable spread instead of reading `255` as
/// 105.28 bps, and a writer cannot emit the reserved byte by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Code {
    /// A real spread, at this index into the codebook.
    Index(CodebookIndex),
    /// This feed has no spread to report.
    Unavailable,
}

impl Code {
    pub fn from_byte(byte: u8) -> Self {
        match CodebookIndex::new(byte) {
            Some(index) => Self::Index(index),
            None => Self::Unavailable,
        }
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Self::Index(index) => index.get(),
            Self::Unavailable => Codebook256::UNAVAILABLE,
        }
    }

    /// The spread, in units of 0.01 bps, or None when there is none to report.
    pub fn hundredths_bps(self) -> Option<u16> {
        match self {
            Self::Index(index) => Some(index.hundredths_bps()),
            Self::Unavailable => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codebook as it is baked into the on-chain `Codebook256` Solidity
    /// library: 512 bytes, 256 big-endian `u16` entries.
    const CANONICAL_TABLE_HEX: &str = "0000000100020003000400050006000700080009000c000e00100012001400160018001a001c001e00200022002400260028002a002c002e00300032003400360038003a003c003e00400042004400460048004a004c004e00500052005400560058005a005c005e00600062006400660068006a006c006e0070007200740076007b00800085008a008f00940099009e00a300a800ad00b200b700bc00c100c600cb00d000d500da00df00e400e900ee00f300f800fd01020107010c01110116011b01200125012a012f01340139013e01430148014d01520157015c01610166016b01700175017a017f01840189018e01930198019d01a201a701ac01b101b601c001ca01d401de01e801f201fc02060210021a0224022e02380242024c02560260026a0274027e02880292029c02a602b002ba02c402ce02d802e202ec02f60300030a0314031e03280332033c03460350035a0364036e03780382038c039603a003aa03b403be03c803d203dc03e603f003fa0404040e04180422042c043604cc056205f8068e072407ba085008e6097c0a120aa80b3e0bd40c6a0d000d960e2c0ec20f580fee1084111a11b0124612dc13721408149e153415ca166016f6178c182218b8194e19e41a7a1b101ba61c3c1cd21d681dfe1e941f2a1fc0205620ec2182221822ae234423da24702506259c263226c8275e27f4288a29202710";

    /// The cross-language contract for this whole format.
    ///
    /// The Rust table is generated from the step table in `SEGMENTS`; the blob
    /// above is copied from the Solidity library that will decode `PackedEvm`
    /// on chain. Serialising one and comparing it to the other is the only thing
    /// keeping the two implementations in agreement, because there is no shared
    /// source and no conformance harness between them.
    ///
    /// If this test fails, do not adjust the blob to match the code. Every entry
    /// is part of a deployed on-chain decoder's ABI: changing a value silently
    /// re-prices every spread that decoder reads. A different table is a new
    /// format version, not an edit to this one.
    #[test]
    fn table_matches_canonical_solidity_blob() {
        let mut bytes = Vec::with_capacity(TABLE.len() * 2);
        for entry in TABLE {
            bytes.extend_from_slice(&entry.to_be_bytes());
        }
        // The blob carries a 256th entry for the reserved byte, which the table
        // does not hold.
        let canonical = hex::decode(CANONICAL_TABLE_HEX).unwrap();
        assert_eq!(canonical.len(), 2 * (Codebook256::TABLE_LEN + 1));
        assert_eq!(bytes.as_slice(), &canonical[..bytes.len()]);
    }

    /// `encode` binary-searches the table, so it must be strictly increasing, and
    /// the endpoints pin the range the format spec promises: 0 up to 105.28 bps.
    #[test]
    fn table_is_strictly_increasing_from_zero_to_the_max_value() {
        assert_eq!(TABLE.first(), Some(&0));
        assert_eq!(TABLE.last(), Some(&10_528));
        assert_eq!(Codebook256::MAX_VALUE, 10_528);
        assert!(TABLE.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn decode_matches_the_step_table_at_every_segment_boundary() {
        // (index, value in 0.01 bps), from the segment table in the format spec.
        let boundaries: [(u8, u16); 11] = [
            (0, 0),
            (9, 9),
            (10, 12),
            (63, 118),
            (64, 123),
            (127, 438),
            (128, 448),
            (191, 1078),
            (192, 1228),
            (253, 10_378),
            (254, 10_528),
        ];
        for (index, value) in boundaries {
            assert_eq!(Codebook256::decode(index).unwrap(), value, "index {index}");
        }
    }

    #[test]
    fn decode_steps_by_a_constant_within_each_segment() {
        // (first_index, last_index, step), from the segment table in the spec.
        let segments: [(u8, u8, u32); 5] = [
            (0, 9, 1),
            (10, 63, 2),
            (64, 127, 5),
            (128, 191, 10),
            (192, 254, 150),
        ];
        for (first_index, last_index, step) in segments {
            for index in first_index..last_index {
                assert_eq!(
                    u32::from(Codebook256::decode(index + 1).unwrap())
                        - u32::from(Codebook256::decode(index).unwrap()),
                    step,
                    "step from index {index} to {}",
                    index + 1
                );
            }
        }
    }

    /// A decoder reads index 255 as "no value", so an encoder that could ever emit
    /// it would make a real spread indistinguishable from a missing one.
    ///
    /// `CodebookIndex` rules that out by construction, but the byte it carries is
    /// what actually reaches the wire, so this checks the byte.
    ///
    /// The two loops are exhaustive between them: the second pins that every value
    /// above `MAX_VALUE` is `None`, so the first covers every input that can yield
    /// a `Some` at all.
    #[test]
    fn encode_never_returns_the_unavailable_sentinel() {
        for value in 0..=i64::from(Codebook256::MAX_VALUE) {
            let index =
                Codebook256::encode(value).unwrap_or_else(|| panic!("no index for {value}"));
            assert_ne!(index.get(), Codebook256::UNAVAILABLE, "value {value}");
            assert!(
                usize::from(index.get()) < Codebook256::TABLE_LEN,
                "value {value}"
            );
        }
        for value in (i64::from(Codebook256::MAX_VALUE) + 1)..=20_000 {
            assert_eq!(Codebook256::encode(value), None, "value {value}");
        }
        for value in [i64::from(u16::MAX), 1_000_000, i64::MAX] {
            assert_eq!(Codebook256::encode(value), None, "value {value}");
        }
    }

    /// A value below the table's first entry has no index; it is not clamped to
    /// zero the way a value between two entries is snapped to the nearer one.
    #[test]
    fn encode_rejects_a_negative_value() {
        for value in [-1, -100, i64::MIN] {
            assert_eq!(Codebook256::encode(value), None, "value {value}");
        }
    }

    /// The index `encode` returns, as the byte it will occupy in a lane.
    fn encoded(value_hundredths_bps: i64) -> Option<u8> {
        Codebook256::encode(value_hundredths_bps).map(CodebookIndex::get)
    }

    #[test]
    fn encode_round_trips_every_encodable_index() {
        for index in 0..Codebook256::TABLE_LEN {
            let index = u8::try_from(index).unwrap();
            let value = Codebook256::decode(index).unwrap();
            assert_eq!(encoded(i64::from(value)), Some(index), "index {index}");
        }
    }

    #[test]
    fn encode_saturates_at_the_largest_entry() {
        assert_eq!(encoded(10_528), Some(254));
        assert_eq!(encoded(10_529), None);
        assert_eq!(encoded(10_600), None);
    }

    #[test]
    fn encode_picks_the_nearest_entry_and_breaks_ties_upward() {
        // Between 118 (index 63) and 123 (index 64): 2 away from the lower entry.
        assert_eq!(encoded(120), Some(63));
        // Between 118 and 123 again, but 2 away from the upper entry.
        assert_eq!(encoded(121), Some(64));
        // Exactly between 12 (index 10) and 14 (index 11): the tie goes upward.
        assert_eq!(encoded(13), Some(11));
        // Exactly between 1228 (index 192) and 1378 (index 193).
        assert_eq!(encoded(1303), Some(193));
        // Below the first entry above zero, so it snaps to zero.
        assert_eq!(encoded(0), Some(0));
    }

    /// The reserved byte is the one value a `CodebookIndex` refuses, and the one a
    /// `Code` reads as unavailable.
    #[test]
    fn the_reserved_byte_is_the_only_unavailable_code() {
        for byte in 0..=u8::MAX {
            match (CodebookIndex::new(byte), Code::from_byte(byte)) {
                (Some(index), Code::Index(from_byte)) => {
                    assert_eq!(index, from_byte, "byte {byte}");
                    assert_eq!(index.get(), byte, "byte {byte}");
                    assert_eq!(
                        index.hundredths_bps(),
                        Codebook256::decode(byte).unwrap(),
                        "byte {byte}"
                    );
                }
                (None, Code::Unavailable) => assert_eq!(byte, Codebook256::UNAVAILABLE),
                (index, code) => panic!("byte {byte} gave {index:?} and {code:?}"),
            }
            assert_eq!(Code::from_byte(byte).to_byte(), byte, "byte {byte}");
        }
        assert_eq!(Code::Unavailable.hundredths_bps(), None);
    }
}
