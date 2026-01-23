mod message;
mod team;
mod user;

pub use message::*;
pub use team::*;
pub use user::*;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::str::FromStr;

/// Access token with expiration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub value: String,
    pub expires: u64,
}

/// Device code information for OAuth flow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeInfo {
    #[serde(rename = "user_code")]
    pub user_code: String,
    #[serde(rename = "device_code")]
    pub device_code: String,
    #[serde(rename = "verification_url")]
    pub verification_url: String,
    #[serde(rename = "expires_in")]
    pub expires_in: String,
    pub interval: String,
    pub message: String,
}

/// Token storage containing all tokens
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStore {
    #[serde(default)]
    pub tokens: std::collections::HashMap<String, AccessToken>,
}

impl TokenStore {
    pub fn get(&self, scope: &str) -> Option<&AccessToken> {
        self.tokens.get(scope)
    }

    pub fn insert(&mut self, scope: String, token: AccessToken) {
        self.tokens.insert(scope, token);
    }

    pub fn refresh_token(&self) -> Option<&AccessToken> {
        self.tokens.get("refresh_token")
    }

    pub fn skype_token(&self) -> Option<&AccessToken> {
        self.tokens.get("skype_token")
    }
}

// Helper deserializers (ported from Squads)

/// Strip URL prefix from contact IDs
pub fn strip_url<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.map(|url| {
        let pass1 = url
            .strip_prefix("https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/contacts/")
            .unwrap_or(&url);
        pass1
            .strip_prefix("https://notifications.skype.net/v1/users/ME/contacts/")
            .unwrap_or(pass1)
            .to_string()
    }))
}

/// Convert string to i64
pub fn string_to_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(s) => i64::from_str(&s).map_err(serde::de::Error::custom),
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("Number is not a valid i64")),
        _ => Err(serde::de::Error::custom("Unexpected type")),
    }
}

/// Convert string to bool
pub fn string_to_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Bool(b) => Ok(b),
        Value::String(s) => match s.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(serde::de::Error::custom(format!(
                "Invalid boolean string: {}",
                s
            ))),
        },
        _ => Err(serde::de::Error::custom("Unexpected type")),
    }
}

/// Convert string to optional bool
pub fn string_to_option_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    if let Some(value) = value {
        match value {
            Value::Bool(b) => Ok(Some(b)),
            Value::String(s) => match s.as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(serde::de::Error::custom(format!(
                    "Invalid boolean string: {}",
                    s
                ))),
            },
            _ => Err(serde::de::Error::custom("Unexpected type")),
        }
    } else {
        Ok(None)
    }
}
