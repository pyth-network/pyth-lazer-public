use crate::publisher_update::feed_update::Update;
use crate::publisher_update::{FeedUpdate, FundingRateUpdate, PriceUpdate};
use crate::state::FeedState;
use ::protobuf::{EnumOrUnknown, MessageField};
use pyth_lazer_protocol::jrpc::{FeedUpdateParams, UpdateParams};
use pyth_lazer_protocol::{
    ExchangeAssetClass, ExchangeAssetSector, ExchangeAssetSubclass, FeedKind, SymbolState,
};

pub mod transaction_envelope {
    pub use crate::protobuf::transaction_envelope::*;
}

pub mod transaction {
    pub use crate::protobuf::pyth_lazer_transaction::*;
}

pub mod publisher_update {
    pub use crate::protobuf::publisher_update::*;
}

pub mod governance_instruction {
    pub use crate::protobuf::governance_instruction::*;
}

pub mod state {
    pub use crate::protobuf::state::*;
}

pub mod dynamic_value {
    pub use crate::protobuf::dynamic_value::*;
}

#[allow(rustdoc::broken_intra_doc_links)]
mod protobuf {
    include!(concat!(env!("OUT_DIR"), "/protobuf/mod.rs"));
}

mod convert_dynamic_value;

impl From<FeedUpdateParams> for FeedUpdate {
    fn from(value: FeedUpdateParams) -> Self {
        FeedUpdate {
            feed_id: Some(value.feed_id.0),
            source_timestamp: value.source_timestamp.into(),
            update: Some(value.update.into()),
            special_fields: Default::default(),
        }
    }
}

impl From<UpdateParams> for Update {
    fn from(value: UpdateParams) -> Self {
        match value {
            UpdateParams::PriceUpdate {
                price,
                best_bid_price,
                best_ask_price,
                trading_status,
                market_session,
            } => Update::PriceUpdate(PriceUpdate {
                price: price.map(|p| p.mantissa_i64()),
                best_bid_price: best_bid_price.map(|p| p.mantissa_i64()),
                best_ask_price: best_ask_price.map(|p| p.mantissa_i64()),
                trading_status: trading_status
                    .map(|ts| EnumOrUnknown::from(state::TradingStatus::from(ts))),
                market_session: market_session
                    .map(|ms| EnumOrUnknown::from(state::MarketSession::from(ms))),
                special_fields: Default::default(),
            }),
            UpdateParams::FundingRateUpdate {
                price,
                rate,
                funding_rate_interval,
            } => Update::FundingRateUpdate(FundingRateUpdate {
                price: price.map(|p| p.mantissa_i64()),
                rate: Some(rate.mantissa()),
                funding_rate_interval: MessageField::from_option(
                    funding_rate_interval.map(|i| i.into()),
                ),
                special_fields: Default::default(),
            }),
        }
    }
}

impl From<FeedState> for SymbolState {
    fn from(value: FeedState) -> Self {
        match value {
            FeedState::COMING_SOON => SymbolState::ComingSoon,
            FeedState::STABLE => SymbolState::Stable,
            FeedState::INACTIVE => SymbolState::Inactive,
            FeedState::BETA => SymbolState::Beta,
        }
    }
}

impl From<SymbolState> for FeedState {
    fn from(value: SymbolState) -> Self {
        match value {
            SymbolState::ComingSoon => FeedState::COMING_SOON,
            SymbolState::Stable => FeedState::STABLE,
            SymbolState::Inactive => FeedState::INACTIVE,
            SymbolState::Beta => FeedState::BETA,
        }
    }
}

impl From<FeedKind> for state::FeedKind {
    fn from(value: FeedKind) -> Self {
        match value {
            FeedKind::Price => state::FeedKind::PRICE,
            FeedKind::FundingRate => state::FeedKind::FUNDING_RATE,
        }
    }
}

impl From<state::FeedKind> for FeedKind {
    fn from(value: state::FeedKind) -> Self {
        match value {
            state::FeedKind::PRICE => FeedKind::Price,
            state::FeedKind::FUNDING_RATE => FeedKind::FundingRate,
        }
    }
}

impl TryFrom<state::Channel> for pyth_lazer_protocol::api::Channel {
    type Error = anyhow::Error;

    fn try_from(value: state::Channel) -> Result<Self, Self::Error> {
        Ok(match value.kind {
            Some(kind) => match kind {
                state::channel::Kind::Rate(rate) => {
                    pyth_lazer_protocol::api::Channel::FixedRate(rate.try_into()?)
                }
                state::channel::Kind::RealTime(_) => pyth_lazer_protocol::api::Channel::RealTime,
            },
            None => pyth_lazer_protocol::api::Channel::FixedRate(
                pyth_lazer_protocol::time::FixedRate::MIN,
            ),
        })
    }
}

impl From<pyth_lazer_protocol::api::Channel> for state::Channel {
    fn from(value: pyth_lazer_protocol::api::Channel) -> Self {
        let mut result = state::Channel::new();
        match value {
            pyth_lazer_protocol::api::Channel::FixedRate(rate) => {
                result.set_rate(rate.into());
            }
            pyth_lazer_protocol::api::Channel::RealTime => {
                result.set_real_time(::protobuf::well_known_types::empty::Empty::new());
            }
        };
        result
    }
}

impl From<pyth_lazer_protocol::api::MarketSession> for state::MarketSession {
    fn from(value: pyth_lazer_protocol::api::MarketSession) -> Self {
        match value {
            pyth_lazer_protocol::api::MarketSession::Regular => state::MarketSession::REGULAR,
            pyth_lazer_protocol::api::MarketSession::PreMarket => state::MarketSession::PRE_MARKET,
            pyth_lazer_protocol::api::MarketSession::PostMarket => {
                state::MarketSession::POST_MARKET
            }
            pyth_lazer_protocol::api::MarketSession::OverNight => state::MarketSession::OVER_NIGHT,
            pyth_lazer_protocol::api::MarketSession::Closed => state::MarketSession::CLOSED,
        }
    }
}

impl From<state::MarketSession> for pyth_lazer_protocol::api::MarketSession {
    fn from(value: state::MarketSession) -> Self {
        match value {
            state::MarketSession::REGULAR => pyth_lazer_protocol::api::MarketSession::Regular,
            state::MarketSession::PRE_MARKET => pyth_lazer_protocol::api::MarketSession::PreMarket,
            state::MarketSession::POST_MARKET => {
                pyth_lazer_protocol::api::MarketSession::PostMarket
            }
            state::MarketSession::OVER_NIGHT => pyth_lazer_protocol::api::MarketSession::OverNight,
            state::MarketSession::CLOSED => pyth_lazer_protocol::api::MarketSession::Closed,
        }
    }
}

impl From<pyth_lazer_protocol::api::TradingStatus> for state::TradingStatus {
    fn from(value: pyth_lazer_protocol::api::TradingStatus) -> Self {
        match value {
            pyth_lazer_protocol::api::TradingStatus::Open => {
                state::TradingStatus::TRADING_STATUS_OPEN
            }
            pyth_lazer_protocol::api::TradingStatus::Closed => {
                state::TradingStatus::TRADING_STATUS_CLOSED
            }
            pyth_lazer_protocol::api::TradingStatus::Halted => {
                state::TradingStatus::TRADING_STATUS_HALTED
            }
            pyth_lazer_protocol::api::TradingStatus::CorpAction => {
                state::TradingStatus::TRADING_STATUS_CORP_ACTION
            }
        }
    }
}

impl From<state::TradingStatus> for pyth_lazer_protocol::api::TradingStatus {
    fn from(value: state::TradingStatus) -> Self {
        match value {
            state::TradingStatus::TRADING_STATUS_OPEN => {
                pyth_lazer_protocol::api::TradingStatus::Open
            }
            state::TradingStatus::TRADING_STATUS_CLOSED => {
                pyth_lazer_protocol::api::TradingStatus::Closed
            }
            state::TradingStatus::TRADING_STATUS_HALTED => {
                pyth_lazer_protocol::api::TradingStatus::Halted
            }
            state::TradingStatus::TRADING_STATUS_CORP_ACTION => {
                pyth_lazer_protocol::api::TradingStatus::CorpAction
            }
        }
    }
}

impl From<state::ExchangeAssetClass> for ExchangeAssetClass {
    fn from(value: state::ExchangeAssetClass) -> Self {
        match value {
            state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_UNSPECIFIED => {
                ExchangeAssetClass::Unspecified
            }
            state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_EQUITY => ExchangeAssetClass::Equity,
            state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_FUTURE => ExchangeAssetClass::Future,
        }
    }
}

impl From<ExchangeAssetClass> for state::ExchangeAssetClass {
    fn from(value: ExchangeAssetClass) -> Self {
        match value {
            ExchangeAssetClass::Unspecified => {
                state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_UNSPECIFIED
            }
            ExchangeAssetClass::Equity => state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_EQUITY,
            ExchangeAssetClass::Future => state::ExchangeAssetClass::EXCHANGE_ASSET_CLASS_FUTURE,
        }
    }
}

impl From<state::ExchangeAssetSubclass> for ExchangeAssetSubclass {
    fn from(value: state::ExchangeAssetSubclass) -> Self {
        match value {
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_UNSPECIFIED => {
                ExchangeAssetSubclass::Unspecified
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_COMMON_STOCK => {
                ExchangeAssetSubclass::CommonStock
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_ETF => ExchangeAssetSubclass::Etf,
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_ENERGY => {
                ExchangeAssetSubclass::Energy
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_METALS => {
                ExchangeAssetSubclass::Metals
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_EQUITY => {
                ExchangeAssetSubclass::Equity
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_FIXED_INCOME => {
                ExchangeAssetSubclass::FixedIncome
            }
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_FX => ExchangeAssetSubclass::Fx,
            state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_AGRICULTURAL => {
                ExchangeAssetSubclass::Agricultural
            }
        }
    }
}

impl From<ExchangeAssetSubclass> for state::ExchangeAssetSubclass {
    fn from(value: ExchangeAssetSubclass) -> Self {
        match value {
            ExchangeAssetSubclass::Unspecified => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_UNSPECIFIED
            }
            ExchangeAssetSubclass::CommonStock => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_COMMON_STOCK
            }
            ExchangeAssetSubclass::Etf => state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_ETF,
            ExchangeAssetSubclass::Energy => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_ENERGY
            }
            ExchangeAssetSubclass::Metals => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_METALS
            }
            ExchangeAssetSubclass::Equity => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_EQUITY
            }
            ExchangeAssetSubclass::FixedIncome => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_FIXED_INCOME
            }
            ExchangeAssetSubclass::Fx => state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_FX,
            ExchangeAssetSubclass::Agricultural => {
                state::ExchangeAssetSubclass::EXCHANGE_ASSET_SUBCLASS_AGRICULTURAL
            }
        }
    }
}

impl From<state::ExchangeAssetSector> for ExchangeAssetSector {
    fn from(value: state::ExchangeAssetSector) -> Self {
        match value {
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_UNSPECIFIED => {
                ExchangeAssetSector::Unspecified
            }
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_TECHNOLOGY => {
                ExchangeAssetSector::Technology
            }
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_FINANCIALS => {
                ExchangeAssetSector::Financials
            }
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_BROAD_MARKET => {
                ExchangeAssetSector::BroadMarket
            }
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_OIL => ExchangeAssetSector::Oil,
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_METALS => ExchangeAssetSector::Metals,
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_INDEX => ExchangeAssetSector::Index,
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_RATES => ExchangeAssetSector::Rates,
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_FX => ExchangeAssetSector::Fx,
            state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_AGRICULTURAL => {
                ExchangeAssetSector::Agricultural
            }
        }
    }
}

impl From<ExchangeAssetSector> for state::ExchangeAssetSector {
    fn from(value: ExchangeAssetSector) -> Self {
        match value {
            ExchangeAssetSector::Unspecified => {
                state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_UNSPECIFIED
            }
            ExchangeAssetSector::Technology => {
                state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_TECHNOLOGY
            }
            ExchangeAssetSector::Financials => {
                state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_FINANCIALS
            }
            ExchangeAssetSector::BroadMarket => {
                state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_BROAD_MARKET
            }
            ExchangeAssetSector::Oil => state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_OIL,
            ExchangeAssetSector::Metals => state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_METALS,
            ExchangeAssetSector::Index => state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_INDEX,
            ExchangeAssetSector::Rates => state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_RATES,
            ExchangeAssetSector::Fx => state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_FX,
            ExchangeAssetSector::Agricultural => {
                state::ExchangeAssetSector::EXCHANGE_ASSET_SECTOR_AGRICULTURAL
            }
        }
    }
}
