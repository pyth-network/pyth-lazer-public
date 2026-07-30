use {
    serde::{Deserialize, Serialize},
    std::fmt::Display,
};

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolState {
    ComingSoon,
    Stable,
    Inactive,
    Beta,
    Expired,
    /// Deserialization fallback for states introduced after this build;
    /// never originates server-side.
    #[serde(other)]
    Unknown,
}

impl Display for SymbolState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolState::ComingSoon => write!(f, "coming_soon"),
            SymbolState::Stable => write!(f, "stable"),
            SymbolState::Inactive => write!(f, "inactive"),
            SymbolState::Beta => write!(f, "beta"),
            SymbolState::Expired => write!(f, "expired"),
            SymbolState::Unknown => write!(f, "unknown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_states_roundtrip() {
        for (state, wire) in [
            (SymbolState::ComingSoon, "\"coming_soon\""),
            (SymbolState::Stable, "\"stable\""),
            (SymbolState::Inactive, "\"inactive\""),
            (SymbolState::Beta, "\"beta\""),
            (SymbolState::Expired, "\"expired\""),
            (SymbolState::Unknown, "\"unknown\""),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<SymbolState>(wire).unwrap(),
                state,
                "{wire} should deserialize"
            );
        }
    }

    #[test]
    fn unrecognized_state_deserializes_to_unknown() {
        assert_eq!(
            serde_json::from_str::<SymbolState>("\"some_future_state\"").unwrap(),
            SymbolState::Unknown,
        );
    }
}
