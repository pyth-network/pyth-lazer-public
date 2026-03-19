use {
    serde::{Deserialize, Serialize},
    std::fmt::Display,
};

/// Asset class for an exchange entity. Mirrors the proto `AssetClass` enum.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeAssetClass {
    #[default]
    Unspecified,
    Equity,
    Future,
}

impl Display for ExchangeAssetClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unspecified => write!(f, "unspecified"),
            Self::Equity => write!(f, "equity"),
            Self::Future => write!(f, "future"),
        }
    }
}

/// Asset subclass for an exchange entity. Mirrors the proto `AssetSubclass` enum.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeAssetSubclass {
    #[default]
    Unspecified,
    CommonStock,
    Etf,
    Energy,
    Metals,
    Equity,
    FixedIncome,
    Fx,
    Agricultural,
}

impl Display for ExchangeAssetSubclass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unspecified => write!(f, "unspecified"),
            Self::CommonStock => write!(f, "common_stock"),
            Self::Etf => write!(f, "etf"),
            Self::Energy => write!(f, "energy"),
            Self::Metals => write!(f, "metals"),
            Self::Equity => write!(f, "equity"),
            Self::FixedIncome => write!(f, "fixed_income"),
            Self::Fx => write!(f, "fx"),
            Self::Agricultural => write!(f, "agricultural"),
        }
    }
}

/// Asset sector for an exchange entity. Mirrors the proto `AssetSector` enum.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeAssetSector {
    #[default]
    Unspecified,
    Technology,
    Financials,
    BroadMarket,
    Oil,
    Metals,
    Index,
    Rates,
    Fx,
    Agricultural,
}

impl Display for ExchangeAssetSector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unspecified => write!(f, "unspecified"),
            Self::Technology => write!(f, "technology"),
            Self::Financials => write!(f, "financials"),
            Self::BroadMarket => write!(f, "broad_market"),
            Self::Oil => write!(f, "oil"),
            Self::Metals => write!(f, "metals"),
            Self::Index => write!(f, "index"),
            Self::Rates => write!(f, "rates"),
            Self::Fx => write!(f, "fx"),
            Self::Agricultural => write!(f, "agricultural"),
        }
    }
}
