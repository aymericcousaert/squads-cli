use serde::{Deserialize, Serialize};

/// User profile from Microsoft Graph
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub display_name: Option<String>,
    pub business_phones: Option<Vec<String>>,
    pub given_name: Option<String>,
    pub job_title: Option<String>,
    pub mail: Option<String>,
    pub mobile_phone: Option<String>,
    pub office_location: Option<String>,
    pub preferred_language: Option<String>,
    pub surname: Option<String>,
    pub company_name: Option<String>,
    pub user_principal_name: Option<String>,
}

/// Users list response from Graph API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Users {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    pub value: Vec<Profile>,
}

/// Short profile from Teams API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortProfile {
    pub user_principal_name: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    pub user_location: Option<String>,
    pub email: Option<String>,
    pub user_type: Option<String>,
    pub is_short_profile: Option<bool>,
    pub tenant_name: Option<String>,
    pub company_name: Option<String>,
    pub display_name: Option<String>,
    #[serde(rename = "type")]
    pub profile_type: Option<String>,
    pub mri: String,
    pub object_id: Option<String>,
}

/// Fetch short profile response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchShortProfile {
    #[serde(rename = "type")]
    pub response_type: Option<String>,
    pub value: Vec<ShortProfile>,
}

/// User properties from Teams
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProperties {
    pub license_type: Option<String>,
    pub personal_file_site: Option<String>,
    pub locale: Option<String>,
    pub primary_member_name: Option<String>,
    pub skype_name: Option<String>,
}

/// User presence information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub mri: String,
    pub etag: String,
    pub source: Option<String>,
    pub presence: PresenceInfo,
}

/// Presence details
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceInfo {
    pub source_network: Option<String>,
    pub availability: Option<String>,
    pub activity: Option<String>,
    pub device_type: Option<String>,
    pub last_active_time: Option<String>,
}

/// Presences response
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presences {
    pub presence: Vec<Presence>,
    pub is_snapshot: Option<bool>,
}
