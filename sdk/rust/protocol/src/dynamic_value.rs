use std::collections::BTreeMap;

use crate::time::{DurationUs, TimestampUs};
use anyhow::bail;
use derive_more::From;
use serde::{
    ser::{SerializeMap, SerializeSeq},
    Deserialize, Serialize,
};

#[derive(Debug, Clone, PartialEq, From)]
pub enum DynamicValue {
    String(String),
    F64(f64),
    U64(u64),
    I64(i64),
    Bool(bool),
    Timestamp(TimestampUs),
    Duration(DurationUs),
    Bytes(Vec<u8>),
    List(Vec<DynamicValue>),
    Map(BTreeMap<String, DynamicValue>),
}

impl Serialize for DynamicValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            DynamicValue::String(v) => serializer.serialize_str(v),
            DynamicValue::F64(v) => serializer.serialize_f64(*v),
            DynamicValue::U64(v) => serializer.serialize_u64(*v),
            DynamicValue::I64(v) => serializer.serialize_i64(*v),
            DynamicValue::Bool(v) => serializer.serialize_bool(*v),
            DynamicValue::Timestamp(v) => serializer.serialize_u64(v.as_micros()),
            DynamicValue::Duration(v) => {
                serializer.serialize_str(&humantime::format_duration((*v).into()).to_string())
            }
            DynamicValue::Bytes(v) => serializer.serialize_str(&hex::encode(v)),
            DynamicValue::List(v) => {
                let mut seq_serializer = serializer.serialize_seq(Some(v.len()))?;
                for element in v {
                    seq_serializer.serialize_element(element)?;
                }
                seq_serializer.end()
            }
            DynamicValue::Map(map) => {
                let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    map_serializer.serialize_entry(k, v)?;
                }
                map_serializer.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for DynamicValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(s) => Ok(DynamicValue::String(s)),
            serde_json::Value::Bool(b) => Ok(DynamicValue::Bool(b)),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(DynamicValue::I64(i))
                } else if let Some(u) = n.as_u64() {
                    Ok(DynamicValue::U64(u))
                } else if let Some(f) = n.as_f64() {
                    Ok(DynamicValue::F64(f))
                } else {
                    Err(serde::de::Error::custom(format!(
                        "unsupported number value: {n}"
                    )))
                }
            }
            serde_json::Value::Array(arr) => {
                let items = arr
                    .into_iter()
                    .map(|v| serde_json::from_value(v).map_err(serde::de::Error::custom))
                    .collect::<Result<Vec<DynamicValue>, _>>()?;
                Ok(DynamicValue::List(items))
            }
            serde_json::Value::Object(obj) => {
                let map = obj
                    .into_iter()
                    .map(|(k, v)| {
                        let dv: DynamicValue =
                            serde_json::from_value(v).map_err(serde::de::Error::custom)?;
                        Ok((k, dv))
                    })
                    .collect::<Result<BTreeMap<_, _>, D::Error>>()?;
                Ok(DynamicValue::Map(map))
            }
            serde_json::Value::Null => Err(serde::de::Error::custom(
                "null values are not supported in DynamicValue",
            )),
        }
    }
}

impl DynamicValue {
    pub fn is_str(&self, field_name: &str) -> anyhow::Result<()> {
        match self {
            DynamicValue::String(_) => Ok(()),
            _ => bail!("invalid value type for {field_name}: expected String, got {self:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_string() {
        let dv: DynamicValue = serde_json::from_str(r#""hello""#).unwrap();
        assert_eq!(dv, DynamicValue::String("hello".into()));
    }

    #[test]
    fn deserialize_bool() {
        let dv: DynamicValue = serde_json::from_str("true").unwrap();
        assert_eq!(dv, DynamicValue::Bool(true));
    }

    #[test]
    fn deserialize_integer() {
        let dv: DynamicValue = serde_json::from_str("42").unwrap();
        assert_eq!(dv, DynamicValue::I64(42));
    }

    #[test]
    fn deserialize_negative_integer() {
        let dv: DynamicValue = serde_json::from_str("-7").unwrap();
        assert_eq!(dv, DynamicValue::I64(-7));
    }

    #[test]
    fn deserialize_float() {
        let dv: DynamicValue = serde_json::from_str("1.618").unwrap();
        assert_eq!(dv, DynamicValue::F64(1.618));
    }

    #[test]
    fn deserialize_null_is_rejected() {
        let result = serde_json::from_str::<DynamicValue>("null");
        assert!(result.is_err());
    }

    #[test]
    fn roundtrip_complex_value() {
        let json = r#"{
            "name": "BTC/USD",
            "asset_type": "crypto",
            "decimals": 8,
            "active": true,
            "tags": ["price", "spot"],
            "nested": {
                "exchange": "binance",
                "weight": 1.5,
                "ids": [1, 2, 3]
            }
        }"#;

        let dv: DynamicValue = serde_json::from_str(json).unwrap();

        // Verify structure
        let map = match &dv {
            DynamicValue::Map(m) => m,
            other => panic!("expected Map, got {other:?}"),
        };
        assert_eq!(
            map.get("name"),
            Some(&DynamicValue::String("BTC/USD".into()))
        );
        assert_eq!(map.get("decimals"), Some(&DynamicValue::I64(8)));
        assert_eq!(map.get("active"), Some(&DynamicValue::Bool(true)));
        assert_eq!(
            map.get("tags"),
            Some(&DynamicValue::List(vec![
                DynamicValue::String("price".into()),
                DynamicValue::String("spot".into()),
            ]))
        );

        let nested = match map.get("nested") {
            Some(DynamicValue::Map(m)) => m,
            other => panic!("expected nested Map, got {other:?}"),
        };
        assert_eq!(nested.get("weight"), Some(&DynamicValue::F64(1.5)));
        assert_eq!(
            nested.get("ids"),
            Some(&DynamicValue::List(vec![
                DynamicValue::I64(1),
                DynamicValue::I64(2),
                DynamicValue::I64(3),
            ]))
        );

        // Roundtrip: serialize back and deserialize again
        let serialized = serde_json::to_string(&dv).unwrap();
        let dv2: DynamicValue = serde_json::from_str(&serialized).unwrap();
        assert_eq!(dv, dv2);
    }

    #[test]
    fn roundtrip_u64_larger_than_i64_max() {
        let value = DynamicValue::U64(u64::MAX);
        let serialized = serde_json::to_string(&value).unwrap();
        let deserialized: DynamicValue = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, DynamicValue::U64(u64::MAX));
    }
}
