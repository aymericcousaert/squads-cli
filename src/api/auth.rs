use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;
use serde_json::Value;

use super::TEAMS_CLIENT_ID;
use crate::types::{AccessToken, DeviceCodeInfo};

fn get_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Generate a device code for OAuth login
pub async fn gen_device_code(tenant_id: &str) -> Result<DeviceCodeInfo> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/devicecode",
        tenant_id
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        "User-Agent",
        "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"
            .parse()
            .unwrap(),
    );
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );

    let body = format!(
        "client_id={}&resource=https://api.spaces.skype.com",
        TEAMS_CLIENT_ID
    );

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let res = client.post(&url).headers(headers).body(body).send().await?;

    if res.status().is_success() {
        let body = res.text().await?;
        let info: DeviceCodeInfo =
            serde_json::from_str(&body).context("Failed to parse device code response")?;
        Ok(info)
    } else {
        let status = res.status();
        let body = res.text().await?;
        Err(anyhow!(
            "Failed to generate device code: {} - {}",
            status,
            body
        ))
    }
}

/// Poll for refresh token after user authorizes device code
pub async fn gen_refresh_token_from_device_code(
    device_code: &str,
    tenant_id: &str,
) -> Result<AccessToken> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/token",
        tenant_id
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_static("https://teams.microsoft.com"),
    );
    headers.insert(
        "User-Agent",
        "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"
            .parse()
            .unwrap(),
    );

    let body = format!(
        "client_id={}&code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
        TEAMS_CLIENT_ID, device_code
    );

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let res = client.post(&url).headers(headers).body(body).send().await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await?;
        return Err(anyhow!(
            "Device code not yet authorized: {} - {}",
            status,
            body
        ));
    }

    let token_data: HashMap<String, Value> = res.json().await?;

    let value = token_data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No refresh_token in response"))?;

    let expires_in = token_data
        .get("expires_in")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3600);

    Ok(AccessToken {
        value: value.to_string(),
        expires: get_epoch_s() + expires_in,
    })
}

/// Renew a refresh token
pub async fn renew_refresh_token(
    refresh_token: &AccessToken,
    tenant_id: &str,
) -> Result<AccessToken> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        tenant_id
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_static("https://teams.microsoft.com"),
    );

    let body = format!(
        "client_id={}&scope=openid profile offline_access&grant_type=refresh_token&client_info=1&x-client-SKU=msal.js.browser&x-client-VER=3.7.1&refresh_token={}",
        TEAMS_CLIENT_ID, refresh_token.value
    );

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let res = client.post(&url).headers(headers).body(body).send().await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await?;
        return Err(anyhow!(
            "Failed to renew refresh token: {} - {}",
            status,
            body
        ));
    }

    let token_data: HashMap<String, Value> = res.json().await?;

    let value = token_data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No refresh_token in response"))?;

    let expires_in = token_data
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    Ok(AccessToken {
        value: value.to_string(),
        expires: get_epoch_s() + expires_in,
    })
}

/// Generate an access token for a specific scope
pub async fn gen_token(
    refresh_token: &AccessToken,
    scope: &str,
    tenant_id: &str,
) -> Result<AccessToken> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        tenant_id
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_static("https://teams.microsoft.com"),
    );

    let body = format!(
        "client_id={}&scope={} openid profile offline_access&grant_type=refresh_token&client_info=1&x-client-SKU=msal.js.browser&x-client-VER=3.7.1&refresh_token={}&claims={{\"access_token\":{{\"xms_cc\":{{\"values\":[\"CP1\"]}}}}}}",
        TEAMS_CLIENT_ID, scope, refresh_token.value
    );

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let res = client.post(&url).headers(headers).body(body).send().await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await?;
        return Err(anyhow!(
            "Failed to generate token for {}: {} - {}",
            scope,
            status,
            body
        ));
    }

    let token_data: HashMap<String, Value> = res.json().await?;

    let value = token_data
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No access_token in response"))?;

    let expires_in = token_data
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    Ok(AccessToken {
        value: value.to_string(),
        expires: get_epoch_s() + expires_in,
    })
}

/// Generate a Skype token for real-time features
pub async fn gen_skype_token(access_token: &AccessToken) -> Result<AccessToken> {
    let url = "https://teams.microsoft.com/api/authsvc/v1.0/authz";

    let req_access_token = format!("Bearer {}", access_token.value);

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(&req_access_token)?,
    );
    headers.insert("Content-Length", "0".parse()?);

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let res = client.post(url).headers(headers).send().await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await?;
        return Err(anyhow!(
            "Failed to generate Skype token: {} - {}",
            status,
            body
        ));
    }

    let token_data: HashMap<String, Value> = res.json().await?;

    let tokens = token_data
        .get("tokens")
        .ok_or_else(|| anyhow!("No tokens in response"))?;

    let value = tokens
        .get("skypeToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No skypeToken in response"))?;

    let expires_in = tokens
        .get("expiresIn")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    Ok(AccessToken {
        value: value.to_string(),
        expires: get_epoch_s() + expires_in,
    })
}
