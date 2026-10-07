//! A packed representation of 4 price updates and their aggregator timestamp to be consumed by the EVM.

mod lane;
mod timestamp;

use {
    super::AggregatedPriceFeedData,
    crate::{
        price::Price,
        time::{DurationUs, TimestampUs},
        ChannelId, PriceFeedId, PriceFeedProperty,
    },
    anyhow::bail,
    byteorder::{ReadBytesExt, WriteBytesExt, BE},
    itertools::Itertools,
    lane::{Lane, Quote},
    std::io::{Cursor, Read, Write},
    timestamp::TimestampUs56,
};

/// `50 45 56 4d` — `PEVM` in ASCII
pub const PACKED_EVM_PAYLOAD_FORMAT_MAGIC: u32 = 1346721357;

/// How far behind a payload's own timestamp a feed's last update may be.
pub const PACKED_EVM_MAX_PRICE_AGE: DurationUs = DurationUs::from_secs_u32(5);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedEvmFeed {
    pub feed_id: PriceFeedId,
    pub lane: Lane,
}

impl PackedEvmFeed {
    /// The lane for `data` in a payload timestamped `payload_timestamp_us`.
    ///
    /// A feed with no data at all, no price, a price the format cannot hold, or
    /// an update older than [`PACKED_EVM_MAX_PRICE_AGE`] gets
    /// [`Lane::SENTINEL`].
    pub fn lane_for(
        data: Option<&AggregatedPriceFeedData>,
        payload_timestamp_us: TimestampUs,
    ) -> Lane {
        let Some(data) = data else {
            return Lane::SENTINEL;
        };
        let fresh = data
            .feed_update_timestamp
            .map(|updated| {
                payload_timestamp_us.saturating_duration_since(updated) <= PACKED_EVM_MAX_PRICE_AGE
            })
            .unwrap_or(false);
        if !fresh {
            return Lane::SENTINEL;
        }
        let Some(price) = data.price else {
            return Lane::SENTINEL;
        };
        Lane::encode(Quote {
            price: price.mantissa_i64(),
            bid: data.best_bid_price.map(Price::mantissa_i64),
            ask: data.best_ask_price.map(Price::mantissa_i64),
            confidence: data.confidence.map(Price::mantissa_i64),
            exponent: data.exponent,
        })
        .unwrap_or(Lane::SENTINEL)
    }
}

/// The `PackedEvm` wire payload: a [`PackedEvmPayloadData::HEADER_SIZE`]-byte
/// header followed by one 48-byte block per four feeds, always big-endian.
/// Each block packs four price updates (the "word") into a single EVM storage
/// slot, resulting in gas savings.
///
/// | offset | size | field |
/// |---|---|---|
/// | 0 | 4 | [`PACKED_EVM_PAYLOAD_FORMAT_MAGIC`] |
/// | 4 | 1 | `channel_id` |
/// | 5 | 1 | `num_feeds`, at least 1 |
/// | 6 | 48 per block | ⌈`num_feeds` / 4⌉ blocks, four feeds each |
///
/// and each block:
///
/// | offset within block | size | field |
/// |---|---|---|
/// | 0 | 16 | four `u32` feed ids |
/// | 16 | 32 | the word |
///
/// and the word:
///
/// | offset within word | size | field |
/// |---|---|---|
/// | 0 | 24 | four [`Lane`]s, in the same order as the feed ids |
/// | 24 | 7 | timestamp, a [`TimestampUs56`] |
/// | 31 | 1 | reserved, zero |
///
/// The feeds keep the order they were given in.
/// Every block carries the same timestamp, the payload's.
/// If num_feeds is not a multiple of 4,
/// the last block will be partially filled and will be padded at the end
/// with zero-ed feed ids and lanes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackedEvmPayloadData {
    timestamp_us: TimestampUs56,
    channel_id: ChannelId,
    feeds: Vec<PackedEvmFeed>,
}

struct Block {
    feeds: Vec<PackedEvmFeed>,
    timestamp_us: TimestampUs56,
}

impl Block {
    const NUM_FEEDS: usize = 4;

    #[cfg(test)]
    const SERIALIZED_SIZE: usize = 48;

    fn serialize(&self, mut writer: impl Write) -> anyhow::Result<()> {
        for PackedEvmFeed { feed_id, lane: _ } in &self.feeds {
            writer.write_u32::<BE>(feed_id.0)?;
        }
        for _ in self.feeds.len()..Self::NUM_FEEDS {
            writer.write_u32::<BE>(0)?;
        }

        for PackedEvmFeed { feed_id: _, lane } in &self.feeds {
            writer.write_all(&lane.to_bytes())?;
        }
        for _ in self.feeds.len()..Self::NUM_FEEDS {
            writer.write_all(&[0; Lane::SIZE])?;
        }
        writer.write_all(&self.timestamp_us.to_bytes())?;
        writer.write_u8(0)?;
        Ok(())
    }

    fn deserialize(mut reader: impl Read, expected_num_feeds: usize) -> anyhow::Result<Self> {
        let feed_ids = (0..expected_num_feeds)
            .map(|_| reader.read_u32::<BE>().map(PriceFeedId))
            .collect::<Result<Vec<_>, _>>()?;

        for _ in expected_num_feeds..Self::NUM_FEEDS {
            if reader.read_u32::<BE>()? != 0 {
                bail!("PackedEvm slot past num_feeds is not zero-filled");
            }
        }

        let lanes = (0..expected_num_feeds)
            .map(|_| {
                let mut lane_bytes = [0u8; Lane::SIZE];
                reader
                    .read_exact(&mut lane_bytes)
                    .map(|_| Lane::from_bytes(lane_bytes))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for _ in expected_num_feeds..Self::NUM_FEEDS {
            let mut lane_bytes = [0u8; Lane::SIZE];
            reader.read_exact(&mut lane_bytes)?;
            if lane_bytes != [0; Lane::SIZE] {
                bail!("PackedEvm slot past num_feeds is not zero-filled");
            }
        }

        let mut timestamp_us = [0u8; TimestampUs56::SIZE];
        reader.read_exact(&mut timestamp_us)?;
        let timestamp_us = TimestampUs56::from_bytes(timestamp_us);
        if reader.read_u8()? != 0 {
            bail!("PackedEvm reserved byte is not zero");
        }
        Ok(Self {
            feeds: feed_ids
                .into_iter()
                .zip(lanes)
                .map(|(feed_id, lane)| PackedEvmFeed { feed_id, lane })
                .collect(),
            timestamp_us,
        })
    }
}

impl PackedEvmPayloadData {
    /// Serialised header size in bytes.
    pub const HEADER_SIZE: usize = 6;

    /// The properties a lane is built from. A `PackedEvm` request cannot
    /// choose others.
    pub const PROPERTIES: [PriceFeedProperty; 6] = [
        PriceFeedProperty::Price,
        PriceFeedProperty::BestBidPrice,
        PriceFeedProperty::BestAskPrice,
        PriceFeedProperty::Confidence,
        PriceFeedProperty::Exponent,
        PriceFeedProperty::FeedUpdateTimestamp,
    ];

    /// Builds a payload from aggregated feed data.
    ///
    /// `feeds` is matched to `feed_ids` by id, not by position: it may be in
    /// any order and may omit feeds the server has no data for, which get
    /// [`Lane::SENTINEL`] in their slot. Feeds outside `feed_ids` are ignored.
    pub fn from_aggregated(
        timestamp_us: TimestampUs,
        channel_id: ChannelId,
        feed_ids: &[PriceFeedId],
        feeds: &[(PriceFeedId, AggregatedPriceFeedData)],
    ) -> Result<Self, anyhow::Error> {
        let feeds: Vec<PackedEvmFeed> = feed_ids
            .iter()
            .map(|feed_id| {
                (
                    feed_id,
                    feeds
                        .iter()
                        .find(|(id, _)| id == feed_id)
                        .map(|(_, data)| data),
                )
            })
            .map(|(feed_id, data)| PackedEvmFeed {
                feed_id: *feed_id,
                lane: PackedEvmFeed::lane_for(data, timestamp_us),
            })
            .collect();

        if feeds.is_empty() {
            bail!("PackedEvm payload carries no feeds");
        }

        Ok(Self {
            timestamp_us: TimestampUs56::new(timestamp_us)?,
            channel_id,
            feeds,
        })
    }

    pub fn timestamp_us(&self) -> TimestampUs56 {
        self.timestamp_us
    }

    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    pub fn feeds(&self) -> &[PackedEvmFeed] {
        &self.feeds
    }

    fn blocks(&self) -> Vec<Block> {
        self.feeds
            .chunks(Block::NUM_FEEDS)
            .map(|chunk| Block {
                feeds: chunk.to_vec(),
                timestamp_us: self.timestamp_us,
            })
            .collect()
    }

    #[cfg(test)]
    fn num_blocks(&self) -> usize {
        self.feeds.len().div_ceil(Block::NUM_FEEDS)
    }

    #[cfg(test)]
    fn serialized_size(&self) -> usize {
        Self::HEADER_SIZE.saturating_add(self.num_blocks().saturating_mul(Block::SERIALIZED_SIZE))
    }

    pub fn serialize(&self, mut writer: impl Write) -> anyhow::Result<()> {
        writer.write_u32::<BE>(PACKED_EVM_PAYLOAD_FORMAT_MAGIC)?;
        writer.write_u8(self.channel_id.0)?;
        writer.write_u8(self.feeds.len().try_into()?)?;

        for block in self.blocks() {
            block.serialize(&mut writer)?;
        }
        Ok(())
    }

    pub fn deserialize_slice(data: &[u8]) -> anyhow::Result<Self> {
        if data.len() < Self::HEADER_SIZE {
            bail!(
                "PackedEvm payload must be at least {} bytes, got {}",
                Self::HEADER_SIZE,
                data.len()
            );
        }
        let mut reader = Cursor::new(data);

        let magic = reader.read_u32::<BE>()?;
        if magic != PACKED_EVM_PAYLOAD_FORMAT_MAGIC {
            bail!("magic mismatch");
        }
        let channel_id = ChannelId(reader.read_u8()?);
        let num_feeds = usize::from(reader.read_u8()?);

        let mut blocks = Vec::new();
        for chunk in &(0..num_feeds).chunks(Block::NUM_FEEDS) {
            let expected_num_feeds = chunk.count();
            blocks.push(Block::deserialize(&mut reader, expected_num_feeds)?);
        }

        if !blocks.iter().map(|block| block.timestamp_us).all_equal() {
            bail!("PackedEvm blocks have different timestamps");
        };

        let timestamp_us = blocks
            .first()
            .ok_or_else(|| anyhow::anyhow!("PackedEvm payload carries no feeds"))?
            .timestamp_us;

        if reader
            .position()
            .try_into()
            .map(|position: usize| data.len() != position)
            .unwrap_or(false)
        {
            bail!("Not all bytes were read");
        }

        Ok(Self {
            timestamp_us,
            channel_id,
            feeds: blocks.into_iter().flat_map(|block| block.feeds).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::MarketSession, payload::packed_evm::timestamp::TimestampTooLarge};

    /// Every payload in these tests carries this timestamp unless it says otherwise.
    const TIMESTAMP_US: u64 = 1_771_339_368_200_000;

    /// The values a feed's properties carry. Distinct from [`Quote`], which is
    /// what a lane is built from: here even the price may be absent.
    ///
    /// `typical()` is deliberately asymmetric — the confidence, the bid's distance
    /// from the price and the ask's are all different — so a test that reads the
    /// wrong one of them fails rather than coincidentally passing.
    #[derive(Clone, Copy)]
    struct FeedValues {
        price: Option<i64>,
        bid: Option<i64>,
        ask: Option<i64>,
        confidence: Option<i64>,
        exponent: i16,
    }

    impl FeedValues {
        /// A quote around $67134.36, at Lazer's usual crypto exponent.
        fn typical() -> Self {
            Self {
                price: Some(6_713_436_287_632),
                bid: Some(6_713_017_907_790),
                ask: Some(6_713_596_631_740),
                // 43 hundredths of a bp of the price, which ties between two
                // codebook entries and so rounds up to 44.
                confidence: Some(288_677_760),
                exponent: -8,
            }
        }

        /// A round quote of `dollars`, one cent wide on each side.
        fn round(dollars: i64) -> Self {
            Self {
                price: Some(dollars * 100),
                bid: Some(dollars * 100 - 1),
                ask: Some(dollars * 100 + 1),
                confidence: Some(1),
                exponent: -2,
            }
        }
    }

    type Feed = (PriceFeedId, AggregatedPriceFeedData);

    /// One feed's aggregated data, last updated at `updated_us`.
    fn feed(feed_id: u32, values: FeedValues, updated_us: u64) -> Feed {
        let price = |mantissa: Option<i64>| mantissa.map(|m| Price::from_mantissa(m).unwrap());
        let data = AggregatedPriceFeedData {
            price: price(values.price),
            best_bid_price: price(values.bid),
            best_ask_price: price(values.ask),
            confidence: price(values.confidence),
            ..AggregatedPriceFeedData::empty(
                values.exponent,
                MarketSession::Regular,
                TimestampUs::from_micros(updated_us),
            )
        };
        (PriceFeedId(feed_id), data)
    }

    /// A feed whose prices are fresh as of the payload's own timestamp.
    fn fresh_feed(feed_id: u32, values: FeedValues) -> Feed {
        feed(feed_id, values, TIMESTAMP_US)
    }

    fn feed_ids(ids: impl IntoIterator<Item = u32>) -> Vec<PriceFeedId> {
        ids.into_iter().map(PriceFeedId).collect()
    }

    /// Builds the payload for `feeds` at [`TIMESTAMP_US`] on `channel_id`, as a
    /// subscription to exactly those feed ids would.
    fn pack_on(channel_id: ChannelId, feeds: &[Feed]) -> PackedEvmPayloadData {
        let ids = feed_ids(feeds.iter().map(|(id, _)| id.0));
        PackedEvmPayloadData::from_aggregated(
            TimestampUs::from_micros(TIMESTAMP_US),
            channel_id,
            &ids,
            feeds,
        )
        .unwrap()
    }

    fn pack(feeds: &[Feed]) -> PackedEvmPayloadData {
        pack_on(ChannelId::REAL_TIME, feeds)
    }

    fn serialize(payload: &PackedEvmPayloadData) -> Vec<u8> {
        let mut bytes = Vec::new();
        payload.serialize(&mut bytes).unwrap();
        bytes
    }

    /// Where block `block` starts in a serialised payload.
    fn block_offset(block: usize) -> usize {
        PackedEvmPayloadData::HEADER_SIZE + block * Block::SERIALIZED_SIZE
    }

    /// The lane bytes for slot `slot` of block `block`, straight out of the word.
    fn lane_bytes(bytes: &[u8], block: usize, slot: usize) -> &[u8] {
        let word = block_offset(block) + Block::NUM_FEEDS * 4;
        &bytes[word + slot * Lane::SIZE..word + (slot + 1) * Lane::SIZE]
    }

    /// The feed-id bytes for slot `slot` of block `block`.
    fn feed_id_bytes(bytes: &[u8], block: usize, slot: usize) -> &[u8] {
        let ids = block_offset(block);
        &bytes[ids + slot * 4..ids + (slot + 1) * 4]
    }

    /// The timestamp bytes of block `block`.
    fn timestamp_range(block: usize) -> std::ops::Range<usize> {
        let start = block_offset(block) + Block::NUM_FEEDS * (4 + Lane::SIZE);
        start..start + TimestampUs56::SIZE
    }

    /// `count` fresh feeds with ids `1..=count`, enough to need
    /// `count.div_ceil(4)` blocks.
    fn fresh_feeds(count: usize) -> Vec<Feed> {
        (1..=count)
            .map(|id| fresh_feed(u32::try_from(id).unwrap(), FeedValues::typical()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Round-trip and size
    // -----------------------------------------------------------------------

    /// Every count `num_feeds` can express, from one feed up to the 255 that
    /// fill its byte, so partial, full and multiple blocks all round-trip.
    #[test]
    fn round_trips_for_every_feed_count() {
        for count in 1..=usize::from(u8::MAX) {
            let feeds: Vec<_> = (0..count)
                .map(|index| {
                    fresh_feed(
                        u32::try_from(index).unwrap() + 1,
                        FeedValues::round(100 + i64::try_from(index).unwrap()),
                    )
                })
                .collect();
            let payload = pack(&feeds);
            let bytes = serialize(&payload);
            assert_eq!(
                PackedEvmPayloadData::deserialize_slice(&bytes).unwrap(),
                payload,
                "{count} feeds"
            );
        }
    }

    #[test]
    fn serialises_to_a_header_plus_one_block_per_four_feeds_for_every_feed_count() {
        for count in 1..=usize::from(u8::MAX) {
            let payload = pack(&fresh_feeds(count));
            let bytes = serialize(&payload);
            assert_eq!(
                bytes.len(),
                PackedEvmPayloadData::HEADER_SIZE + count.div_ceil(4) * 48,
                "{count} feeds"
            );
            assert_eq!(bytes.len(), payload.serialized_size(), "{count} feeds");
        }
    }

    /// `num_feeds` is a byte, so a 256th feed cannot be written at all.
    #[test]
    fn more_feeds_than_num_feeds_can_count_do_not_serialise() {
        let payload = pack(&fresh_feeds(usize::from(u8::MAX) + 1));
        assert!(payload.serialize(&mut Vec::new()).is_err());
    }

    /// The feed order on the wire is the subscription's order, and a feed id
    /// listed twice is carried twice.
    #[test]
    fn feeds_keep_their_given_order_and_may_repeat() {
        let payload = pack(&[
            fresh_feed(900, FeedValues::typical()),
            fresh_feed(3, FeedValues::typical()),
            fresh_feed(900, FeedValues::typical()),
            fresh_feed(77, FeedValues::typical()),
            fresh_feed(12, FeedValues::typical()),
        ]);
        let ids = |payload: &PackedEvmPayloadData| {
            payload
                .feeds()
                .iter()
                .map(|feed| feed.feed_id.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&payload), [900, 3, 900, 77, 12]);

        let bytes = serialize(&payload);
        for (index, expected) in [900u32, 3, 900, 77].into_iter().enumerate() {
            assert_eq!(feed_id_bytes(&bytes, 0, index), expected.to_be_bytes());
        }
        assert_eq!(feed_id_bytes(&bytes, 1, 0), 12u32.to_be_bytes());
        assert_eq!(
            ids(&PackedEvmPayloadData::deserialize_slice(&bytes).unwrap()),
            [900, 3, 900, 77, 12]
        );
    }

    // -----------------------------------------------------------------------
    // Golden vectors
    //
    // These are the durable cross-language contract for this format: a future
    // Solidity decoder is correct when it reads them the way the field
    // breakdown next to each one says it should.
    // -----------------------------------------------------------------------

    /// One feed, so three of the four slots are unused.
    ///
    /// | offset | bytes | field |
    /// |---|---|---|
    /// | 0 | `5045564d` | magic, `PEVM` |
    /// | 4 | `01` | channel id 1 (real time) |
    /// | 5 | `01` | one feed |
    /// | 6 | `00000001` | feed id 1 |
    /// | 10 | `00000000 00000000 00000000` | three unused slots, zero |
    /// | 22 | `1c00639b1a2f` | lane 0 |
    /// | 28 | `000000000000` x3 | three unused lanes, zero |
    /// | 46 | `064b0615d17740` | timestamp 1771339368200000 µs |
    /// | 53 | `00` | reserved |
    ///
    /// Lane 0 unpacks as exponent `-3`, mantissa `67134363`, confidence index `26`
    /// (`0x1a`), spread index `47` (`0x2f`):
    ///
    /// - price `6713436287632e-8` re-normalised to `67134363e-3`,
    /// - the confidence, `288677760`, is 43 hundredths of a bp of the price. That
    ///   falls exactly between codebook entries 42 and 44, and a tie goes to the
    ///   higher index, so it encodes as index 26, whose entry is 44.
    /// - the book runs `6713017907790` to `6713596631740`, so it is 86 hundredths
    ///   of a bp wide. Index 47 holds 86 exactly.
    ///
    /// Note that the spread is the width of the whole book, bid to ask. The price
    /// sets the scale it is measured against and nothing else, so where the price
    /// sits inside the book does not enter into it.
    const GOLDEN_ONE_FEED: &str = "5045564d0101000000010000000000000000000000001c00639b1a2f\
000000000000000000000000000000000000064b0615d1774000";

    /// Four feeds, one of them stale.
    ///
    /// | offset | bytes | field |
    /// |---|---|---|
    /// | 0 | `5045564d` | magic, `PEVM` |
    /// | 4 | `02` | channel id 2 (fixed rate 50) |
    /// | 5 | `04` | four feeds |
    /// | 6 | `0000000a 00000014 0000001e 00000028` | feed ids 10, 20, 30, 40 |
    /// | 22 | `10002710364f` | lane for feed 10 |
    /// | 28 | `10004e201db7` | lane for feed 20 |
    /// | 34 | `ffffffffffff` | lane for feed 30, stale, so the sentinel |
    /// | 40 | `10009c401100` | lane for feed 40 |
    /// | 46 | `064b0615d17740` | timestamp 1771339368200000 µs |
    /// | 53 | `00` | reserved |
    ///
    /// The three live lanes all carry exponent `-2`, and every one of them has a
    /// confidence of one cent:
    ///
    /// - feed 10, `$100.00` quoted `99.99`/`100.01`: mantissa `10000` (`0x2710`),
    ///   confidence index `54` (`0x36`) for 100 hundredths of a bp, spread index
    ///   `79` (`0x4f`) — the book is 200 wide and 198 is the nearest entry,
    /// - feed 20, `$200.00` quoted `199.90`/`200.10`: mantissa `20000` (`0x4e20`),
    ///   confidence index `29` (`0x1d`) for 50, spread index `183` (`0xb7`) — the
    ///   book is 1000 wide and 998 is the nearest entry,
    /// - feed 40, `$400.00` with bid and ask both on the price: mantissa `40000`
    ///   (`0x9c40`), confidence index `17` (`0x11`) — 25 ties between entries 24
    ///   and 26, so it rounds up to 26 — and spread index `0`, a book of no width
    ///   at all.
    const GOLDEN_FOUR_FEEDS: &str = "5045564d02040000000a000000140000001e0000002810002710364f\
10004e201db7ffffffffffff10009c401100064b0615d1774000";

    /// Five feeds, so a full block and then a second one holding only the
    /// fifth. The second block repeats the timestamp and zero-fills its three
    /// spare slots, exactly as a one-feed payload would.
    ///
    /// | offset | bytes | field |
    /// |---|---|---|
    /// | 0 | `5045564d` | magic, `PEVM` |
    /// | 4 | `01` | channel id 1 (real time) |
    /// | 5 | `05` | five feeds |
    /// | 6 | `00000001 00000002 00000003 00000004` | block 0: feed ids 1 to 4 |
    /// | 22 | `1c00639b1a2f` | lane for feed 1, as in [`GOLDEN_ONE_FEED`] |
    /// | 28 | `10002710364f` | lane for feed 2, as feed 10 in [`GOLDEN_FOUR_FEEDS`] |
    /// | 34 | `ffffffffffff` | lane for feed 3, stale, so the sentinel |
    /// | 40 | `1c00639b1a2f` | lane for feed 4 |
    /// | 46 | `064b0615d17740` | timestamp 1771339368200000 µs |
    /// | 53 | `00` | reserved |
    /// | 54 | `00000005 00000000 00000000 00000000` | block 1: feed id 5, three unused slots |
    /// | 70 | `10002710364f` | lane for feed 5 |
    /// | 76 | `000000000000` x3 | three unused lanes, zero |
    /// | 94 | `064b0615d17740` | the same timestamp again |
    /// | 101 | `00` | reserved |
    const GOLDEN_FIVE_FEEDS: &str = "5045564d0105000000010000000200000003000000041c00639b1a2f\
10002710364fffffffffffff1c00639b1a2f064b0615d1774000000000050000000000000000000000001000\
2710364f000000000000000000000000000000000000064b0615d1774000";

    #[test]
    fn golden_vector_one_feed() {
        let payload = pack(&[fresh_feed(1, FeedValues::typical())]);
        assert_eq!(hex::encode(serialize(&payload)), GOLDEN_ONE_FEED);
        assert_eq!(
            PackedEvmPayloadData::deserialize_slice(&hex::decode(GOLDEN_ONE_FEED).unwrap())
                .unwrap(),
            payload
        );
    }

    #[test]
    fn golden_vector_four_feeds() {
        let stale = TIMESTAMP_US - PACKED_EVM_MAX_PRICE_AGE.as_micros() - 1;
        let feeds = [
            fresh_feed(10, FeedValues::round(100)),
            feed(
                20,
                FeedValues {
                    price: Some(20_000),
                    bid: Some(19_990),
                    ask: Some(20_010),
                    confidence: Some(1),
                    exponent: -2,
                },
                TIMESTAMP_US,
            ),
            feed(30, FeedValues::round(300), stale),
            feed(
                40,
                FeedValues {
                    price: Some(40_000),
                    bid: Some(40_000),
                    ask: Some(40_000),
                    confidence: Some(1),
                    exponent: -2,
                },
                TIMESTAMP_US,
            ),
        ];
        let payload = pack_on(ChannelId::FIXED_RATE_50, &feeds);
        assert_eq!(hex::encode(serialize(&payload)), GOLDEN_FOUR_FEEDS);
        assert_eq!(
            PackedEvmPayloadData::deserialize_slice(&hex::decode(GOLDEN_FOUR_FEEDS).unwrap())
                .unwrap(),
            payload
        );
    }

    #[test]
    fn golden_vector_five_feeds() {
        let stale = TIMESTAMP_US - PACKED_EVM_MAX_PRICE_AGE.as_micros() - 1;
        let payload = pack(&[
            fresh_feed(1, FeedValues::typical()),
            fresh_feed(2, FeedValues::round(100)),
            feed(3, FeedValues::typical(), stale),
            fresh_feed(4, FeedValues::typical()),
            fresh_feed(5, FeedValues::round(100)),
        ]);
        assert_eq!(hex::encode(serialize(&payload)), GOLDEN_FIVE_FEEDS);
        assert_eq!(
            PackedEvmPayloadData::deserialize_slice(&hex::decode(GOLDEN_FIVE_FEEDS).unwrap())
                .unwrap(),
            payload
        );
    }

    // -----------------------------------------------------------------------
    // Zero-fill versus sentinel-fill
    // -----------------------------------------------------------------------

    /// The two fills mean opposite things. A slot past `num_feeds` holds no feed at
    /// all and is zero. A feed that is present but has no usable price gets
    /// [`Lane::SENTINEL`], every byte `0xFF`.
    #[test]
    fn unused_slots_are_zero_filled_while_a_failed_feed_is_sentinel_filled() {
        let stale = TIMESTAMP_US - PACKED_EVM_MAX_PRICE_AGE.as_micros() - 1;
        let bytes = serialize(&pack(&[
            fresh_feed(1, FeedValues::typical()),
            feed(2, FeedValues::typical(), stale),
        ]));

        assert_eq!(bytes[5], 2, "num_feeds");
        assert_ne!(lane_bytes(&bytes, 0, 0), [0xFF; Lane::SIZE]);
        assert_eq!(
            lane_bytes(&bytes, 0, 1),
            Lane::SENTINEL.to_bytes(),
            "a present but unusable feed"
        );
        for slot in 2..Block::NUM_FEEDS {
            assert_eq!(
                feed_id_bytes(&bytes, 0, slot),
                [0; 4],
                "slot {slot} feed id"
            );
            assert_eq!(
                lane_bytes(&bytes, 0, slot),
                [0; Lane::SIZE],
                "slot {slot} lane"
            );
        }
    }

    /// Only the last block can be partly filled, and its spare slots are zero
    /// like a one-block payload's.
    #[test]
    fn only_the_last_block_has_unused_slots_and_they_are_zero_filled() {
        let bytes = serialize(&pack(&fresh_feeds(5)));

        assert_eq!(bytes[5], 5, "num_feeds");
        for slot in 0..Block::NUM_FEEDS {
            assert_ne!(
                feed_id_bytes(&bytes, 0, slot),
                [0; 4],
                "block 0 slot {slot}"
            );
            assert_ne!(
                lane_bytes(&bytes, 0, slot),
                [0; Lane::SIZE],
                "block 0 slot {slot}"
            );
        }
        assert_eq!(feed_id_bytes(&bytes, 1, 0), 5u32.to_be_bytes());
        assert_ne!(lane_bytes(&bytes, 1, 0), [0; Lane::SIZE]);
        for slot in 1..Block::NUM_FEEDS {
            assert_eq!(
                feed_id_bytes(&bytes, 1, slot),
                [0; 4],
                "block 1 slot {slot}"
            );
            assert_eq!(
                lane_bytes(&bytes, 1, slot),
                [0; Lane::SIZE],
                "block 1 slot {slot}"
            );
        }
    }

    /// The timestamp is written once per block, and every copy is the same.
    #[test]
    fn every_block_carries_the_payload_timestamp() {
        let bytes = serialize(&pack(&fresh_feeds(9)));
        let expected = TimestampUs56::new(TimestampUs::from_micros(TIMESTAMP_US))
            .unwrap()
            .to_bytes();
        for block in 0..3 {
            assert_eq!(bytes[timestamp_range(block)], expected, "block {block}");
        }
    }

    // -----------------------------------------------------------------------
    // Staleness
    // -----------------------------------------------------------------------

    /// A feed exactly `PACKED_EVM_MAX_PRICE_AGE` behind the payload is still
    /// fresh. One microsecond older is stale.
    #[test]
    fn staleness_boundary_is_inclusive() {
        let max_age = PACKED_EVM_MAX_PRICE_AGE.as_micros();
        for (updated_us, stale, name) in [
            (TIMESTAMP_US - max_age, false, "exactly at the boundary"),
            (TIMESTAMP_US - max_age - 1, true, "one microsecond past it"),
        ] {
            let payload = pack(&[feed(1, FeedValues::typical(), updated_us)]);
            assert_eq!(payload.feeds()[0].lane.is_unavailable(), stale, "{name}");
        }
    }

    /// A feed whose update timestamp is ahead of the payload is not stale; the age
    /// saturates at zero rather than wrapping into the distant past.
    #[test]
    fn a_feed_from_the_future_is_not_stale() {
        let payload = pack(&[feed(1, FeedValues::typical(), TIMESTAMP_US + 1_000_000)]);
        assert!(!payload.feeds()[0].lane.is_unavailable());
    }

    // -----------------------------------------------------------------------
    // Per-lane failure modes
    // -----------------------------------------------------------------------

    /// Each of these costs one feed its lane and nothing else. The payload still
    /// serialises and its other feeds still carry good data.
    ///
    /// The list is short because only the price can sink a lane. A confidence or a
    /// book that cannot be encoded costs its own byte and leaves the rest of the
    /// lane standing — see `losing_one_code_leaves_the_other_alone` in the
    /// `lane` tests.
    #[test]
    fn every_lane_failure_mode_sentinels_only_its_own_lane() {
        let cases: [(&str, FeedValues, u64); 3] = [
            (
                "price absent",
                FeedValues {
                    price: None,
                    ..FeedValues::typical()
                },
                TIMESTAMP_US,
            ),
            (
                "price does not pack",
                FeedValues {
                    // A positive exponent is not a scale this format can hold.
                    exponent: 1,
                    ..FeedValues::typical()
                },
                TIMESTAMP_US,
            ),
            (
                "stale",
                FeedValues::typical(),
                TIMESTAMP_US - PACKED_EVM_MAX_PRICE_AGE.as_micros() - 1,
            ),
        ];

        for (name, values, updated_us) in cases {
            let payload = pack(&[
                fresh_feed(1, FeedValues::typical()),
                feed(2, values, updated_us),
                fresh_feed(3, FeedValues::typical()),
            ]);
            let lanes = payload.feeds();
            assert_eq!(lanes[1].lane, Lane::SENTINEL, "{name}");
            assert_ne!(lanes[0].lane, Lane::SENTINEL, "{name}: sibling before");
            assert_ne!(lanes[2].lane, Lane::SENTINEL, "{name}: sibling after");
            assert_eq!(
                serialize(&payload).len(),
                payload.serialized_size(),
                "{name}"
            );
        }
    }

    /// A feed with no update timestamp at all cannot be shown to be fresh, so it
    /// loses its lane rather than being taken on trust.
    #[test]
    fn a_feed_without_an_update_timestamp_gets_the_sentinel() {
        let (feed_id, mut data) = fresh_feed(1, FeedValues::typical());
        data.feed_update_timestamp = None;
        let payload = pack(&[(feed_id, data)]);
        assert_eq!(payload.feeds()[0].lane, Lane::SENTINEL);
    }

    /// A subscribed feed the router has no data for still occupies its slot: the
    /// payload always carries every feed of the subscription, sentinel or not.
    #[test]
    fn a_subscribed_feed_with_no_data_keeps_its_slot_with_the_sentinel() {
        let feeds = [fresh_feed(2, FeedValues::typical())];
        let payload = PackedEvmPayloadData::from_aggregated(
            TimestampUs::from_micros(TIMESTAMP_US),
            ChannelId::REAL_TIME,
            &feed_ids([2, 1]),
            &feeds,
        )
        .unwrap();
        let lanes = payload.feeds();
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].feed_id, PriceFeedId(2));
        assert_ne!(lanes[0].lane, Lane::SENTINEL);
        assert_eq!(lanes[1].feed_id, PriceFeedId(1));
        assert_eq!(lanes[1].lane, Lane::SENTINEL);
    }

    /// Data the format does not read is simply ignored.
    #[test]
    fn unrelated_aggregated_fields_do_not_change_the_lane() {
        let (feed_id, mut data) = fresh_feed(1, FeedValues::typical());
        let plain = pack(&[(feed_id, data.clone())]);
        data.publisher_count = 18;
        data.ema_price = data.price;
        let decorated = pack(&[(feed_id, data)]);
        assert_eq!(plain, decorated);
    }

    // -----------------------------------------------------------------------
    // Timestamp
    // -----------------------------------------------------------------------

    /// [`TimestampUs56`] refuses a timestamp wider than its field;
    /// the constructor passes that refusal on rather than truncating.
    #[test]
    fn a_timestamp_too_wide_for_the_field_is_rejected_by_the_constructor() {
        let too_wide = TimestampUs::from_micros(1 << 56);
        let err = PackedEvmPayloadData::from_aggregated(
            too_wide,
            ChannelId::REAL_TIME,
            &feed_ids([1]),
            &[fresh_feed(1, FeedValues::typical())],
        )
        .unwrap_err();
        assert_eq!(
            err.downcast_ref::<TimestampTooLarge>(),
            Some(&TimestampTooLarge(too_wide))
        );
    }

    /// A timestamp whose every byte is set proves the field is not being written a
    /// byte short, and survives the round trip.
    #[test]
    fn a_timestamp_using_all_56_bits_round_trips() {
        let widest = TimestampUs::from_micros((1 << 56) - 1);
        let payload = PackedEvmPayloadData::from_aggregated(
            widest,
            ChannelId::REAL_TIME,
            &feed_ids([1]),
            &[fresh_feed(1, FeedValues::typical())],
        )
        .unwrap();
        let bytes = serialize(&payload);
        assert_eq!(bytes[46..53], [0xFF; TimestampUs56::SIZE]);
        assert_eq!(
            PackedEvmPayloadData::deserialize_slice(&bytes)
                .unwrap()
                .timestamp_us(),
            TimestampUs56::MAX
        );
    }

    // -----------------------------------------------------------------------
    // Deserialisation is strict
    // -----------------------------------------------------------------------

    /// `num_feeds` fixes the length exactly: a byte short, a byte over, or a
    /// whole block missing are all rejected, for one block and for two.
    #[test]
    fn deserialize_rejects_a_slice_whose_length_does_not_match_num_feeds() {
        for count in [1, 5] {
            let bytes = serialize(&pack(&fresh_feeds(count)));
            let one_byte_short = &bytes[..bytes.len() - 1];
            let one_byte_over = [bytes.clone(), vec![0]].concat();
            let one_block_short = &bytes[..bytes.len() - Block::SERIALIZED_SIZE];
            for (name, candidate) in [
                ("a byte short", one_byte_short),
                ("a byte over", &one_byte_over[..]),
                ("a block short", one_block_short),
            ] {
                assert!(
                    PackedEvmPayloadData::deserialize_slice(candidate).is_err(),
                    "{count} feeds, {name}"
                );
            }
        }
    }

    /// Zero is the one count the format has no encoding for; a header on its own
    /// carries no timestamp.
    #[test]
    fn deserialize_rejects_zero_feeds() {
        let mut bytes = serialize(&pack(&fresh_feeds(1)));
        bytes[5] = 0;
        for candidate in [&bytes[..], &bytes[..PackedEvmPayloadData::HEADER_SIZE]] {
            assert!(PackedEvmPayloadData::deserialize_slice(candidate)
                .unwrap_err()
                .to_string()
                .contains("no feeds"));
        }
    }

    /// Every block repeats the payload's timestamp, so two blocks that disagree
    /// cannot have come from one payload.
    #[test]
    fn deserialize_rejects_blocks_with_different_timestamps() {
        let mut bytes = serialize(&pack(&fresh_feeds(5)));
        let second_block_timestamp = timestamp_range(1);
        bytes[second_block_timestamp.end - 1] ^= 1;
        assert!(PackedEvmPayloadData::deserialize_slice(&bytes)
            .unwrap_err()
            .to_string()
            .contains("different timestamps"));
    }

    #[test]
    fn deserialize_rejects_another_format_s_magic() {
        let mut bytes = serialize(&pack(&[fresh_feed(1, FeedValues::typical())]));
        bytes[..4].copy_from_slice(&crate::payload::PAYLOAD_FORMAT_MAGIC.to_be_bytes());
        assert!(PackedEvmPayloadData::deserialize_slice(&bytes)
            .unwrap_err()
            .to_string()
            .contains("magic"));
    }

    /// A count the slice has too few blocks for, whether by one feed or by the
    /// most a byte can hold, is rejected rather than read short.
    #[test]
    fn deserialize_rejects_a_feed_count_the_slice_has_too_few_blocks_for() {
        let bytes = serialize(&pack(&fresh_feeds(4)));
        for num_feeds in [5u8, 255] {
            let mut bytes = bytes.clone();
            bytes[5] = num_feeds;
            assert!(
                PackedEvmPayloadData::deserialize_slice(&bytes).is_err(),
                "{num_feeds} feeds claimed"
            );
        }
    }

    /// Padding is part of the encoding, not spare room: a decoder that tolerated
    /// junk there would let the same payload be written more than one way. That
    /// holds in a trailing block as much as in the only block.
    #[test]
    fn deserialize_rejects_non_canonical_padding() {
        for (count, block) in [(1, 0), (5, 1)] {
            let bytes = serialize(&pack(&fresh_feeds(count)));
            // An unused feed-id slot, an unused lane, and the reserved byte.
            let unused_feed_id = block_offset(block) + 4;
            let unused_lane = block_offset(block) + Block::NUM_FEEDS * 4 + Lane::SIZE;
            let reserved = timestamp_range(block).end;
            for (name, offset) in [
                ("unused feed id", unused_feed_id),
                ("unused lane", unused_lane),
                ("reserved byte", reserved),
            ] {
                let mut bytes = bytes.clone();
                bytes[offset] = 0xFF;
                assert!(
                    PackedEvmPayloadData::deserialize_slice(&bytes).is_err(),
                    "{count} feeds, block {block}, {name} at offset {offset}"
                );
            }
        }
    }
}
