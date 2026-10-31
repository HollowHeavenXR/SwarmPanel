//! Tolerant JSON -> model conversion. The server (and its Lua predecessor)
//! is not perfectly consistent about types: counts arrive as numbers or
//! strings, booleans as true/false or 0/1, ids as numbers or strings. Every
//! model field goes through `Lenient`, so one odd value never fails a whole
//! decode; it just falls back to the field's default.

use serde_json::Value;
use std::collections::HashMap;

/// Rust i64 that Swift sees as `Int` (see uniffi.toml).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SwiftInt(pub i64);

uniffi::custom_type!(SwiftInt, i64);

impl crate::UniffiCustomTypeConverter for SwiftInt {
    type Builtin = i64;
    fn into_custom(val: i64) -> uniffi::Result<Self> {
        Ok(SwiftInt(val))
    }
    fn from_custom(obj: Self) -> i64 {
        obj.0
    }
}

impl serde::Serialize for SwiftInt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

impl From<i64> for SwiftInt {
    fn from(v: i64) -> Self {
        SwiftInt(v)
    }
}

pub trait Lenient: Sized {
    fn from_json(v: &Value) -> Option<Self>;
}

impl Lenient for String {
    fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

impl Lenient for SwiftInt {
    fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)).map(SwiftInt),
            Value::String(s) => {
                let t = s.trim();
                t.parse::<i64>().ok().or_else(|| t.parse::<f64>().ok().map(|f| f.trunc() as i64)).map(SwiftInt)
            }
            Value::Bool(b) => Some(SwiftInt(*b as i64)),
            _ => None,
        }
    }
}

impl Lenient for f64 {
    fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            Value::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }
}

impl Lenient for bool {
    fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => n.as_f64().map(|f| f != 0.0),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "t" | "1" | "yes" | "on" => Some(true),
                "false" | "f" | "0" | "no" | "off" | "" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }
}

impl<T: Lenient> Lenient for Option<T> {
    fn from_json(v: &Value) -> Option<Self> {
        if v.is_null() { Some(None) } else { Some(T::from_json(v)) }
    }
}

impl<T: Lenient> Lenient for Vec<T> {
    fn from_json(v: &Value) -> Option<Self> {
        v.as_array().map(|a| a.iter().filter_map(T::from_json).collect())
    }
}

impl<T: Lenient> Lenient for HashMap<String, T> {
    fn from_json(v: &Value) -> Option<Self> {
        v.as_object().map(|o| o.iter().filter_map(|(k, x)| T::from_json(x).map(|t| (k.clone(), t))).collect())
    }
}

/// Decode a model from JSON text, defaulting anything unreadable.
pub fn decode<T: Lenient + Default>(text: &str) -> Result<T, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(T::from_json(&v).unwrap_or_default())
}

/// Declares a model: a uniffi Record (Swift struct with camelCase fields)
/// that decodes leniently from the server's snake_case JSON and serializes
/// back to it. `field as "json_key"` maps a renamed field.
#[macro_export]
macro_rules! model {
    // `finish(s) { ... }` runs after decoding, to fill derived fields once
    // (so the UI never has to call back into Rust while rendering).
    ($(#[$m:meta])* $name:ident { $($f:ident $(as $key:literal)? : $t:ty),* $(,)? } finish($s:ident) $body:block) => {
        $(#[$m])*
        #[derive(uniffi::Record, Clone, Debug, Default, PartialEq, serde::Serialize)]
        pub struct $name { $( $(#[serde(rename = $key)])? pub $f: $t ),* }

        impl $crate::lenient::Lenient for $name {
            fn from_json(v: &serde_json::Value) -> Option<Self> {
                let o = v.as_object()?;
                #[allow(unused_mut)]
                let mut $s = Self { $( $f: o.get($crate::model!(@key $f $($key)?)).and_then(<$t as $crate::lenient::Lenient>::from_json).unwrap_or_default() ),* };
                $body
                Some($s)
            }
        }
    };
    ($(#[$m:meta])* $name:ident { $($f:ident $(as $key:literal)? : $t:ty),* $(,)? }) => {
        $crate::model!($(#[$m])* $name { $($f $(as $key)? : $t),* } finish(_s) {});
    };
    (@key $f:ident $key:literal) => { $key };
    (@key $f:ident) => { stringify!($f) };
}
