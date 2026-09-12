// Suppress warnings for deserialization helpers and types kept for future use
#![allow(dead_code)]

mod calendar;
mod mail;
mod message;
mod sheets;
mod team;
mod user;

pub use calendar::*;

pub use mail::*;
pub use message::*;
pub use sheets::*;
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

/// Reduce a contacts URL to the MRI everything downstream compares against.
///
/// The region sits in the path on `teams.microsoft.com` and in the host on
/// `notifications.skype.net`, so keep the last segment instead of matching a
/// prefix. A bare MRI passes through unchanged.
pub fn strip_url<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.map(|url| last_segment(&url).to_string()))
}

/// Last path segment of a URL, or the whole string when it has no slash.
pub(crate) fn last_segment(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Holder {
        #[serde(deserialize_with = "strip_url")]
        from: Option<String>,
    }

    fn from_field(json: &str) -> Option<String> {
        serde_json::from_str::<Holder>(json)
            .expect("payload should parse")
            .from
    }

    #[test]
    fn strip_url_keeps_the_mri_of_a_contacts_url() {
        for region in ["emea", "amer", "apac"] {
            let json = format!(
                r#"{{"from":"https://teams.microsoft.com/api/chatsvc/{region}/v1/users/ME/contacts/8:orgid:u1"}}"#
            );
            assert_eq!(from_field(&json).as_deref(), Some("8:orgid:u1"));
        }
    }

    #[test]
    fn strip_url_handles_a_regional_notifications_host() {
        let json =
            r#"{"from":"https://emea.notifications.skype.net/v1/users/ME/contacts/8:orgid:u1"}"#;
        assert_eq!(from_field(json).as_deref(), Some("8:orgid:u1"));
        let json = r#"{"from":"https://notifications.skype.net/v1/users/ME/contacts/8:orgid:u1"}"#;
        assert_eq!(from_field(json).as_deref(), Some("8:orgid:u1"));
    }

    #[test]
    fn strip_url_passes_a_bare_mri_through() {
        assert_eq!(
            from_field(r#"{"from":"8:orgid:u1"}"#).as_deref(),
            Some("8:orgid:u1")
        );
        assert_eq!(from_field(r#"{"from":null}"#), None);
    }
}
