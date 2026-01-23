use serde::{Deserialize, Serialize};

/// Calendar event from Microsoft Graph
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: Option<String>,
    pub subject: Option<String>,
    pub body_preview: Option<String>,
    pub start: Option<DateTimeZone>,
    pub end: Option<DateTimeZone>,
    pub location: Option<Location>,
    pub organizer: Option<Organizer>,
    pub attendees: Option<Vec<Attendee>>,
    pub is_online_meeting: Option<bool>,
    pub online_meeting_url: Option<String>,
    pub online_meeting: Option<OnlineMeeting>,
    pub web_link: Option<String>,
    pub response_status: Option<ResponseStatus>,
    pub is_cancelled: Option<bool>,
    pub is_all_day: Option<bool>,
}

/// Date time with timezone
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateTimeZone {
    pub date_time: String,
    pub time_zone: String,
}

/// Event location
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub display_name: Option<String>,
    pub location_uri: Option<String>,
}

/// Event organizer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organizer {
    pub email_address: Option<EmailAddressSimple>,
}

/// Simple email address
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAddressSimple {
    pub name: Option<String>,
    pub address: Option<String>,
}

/// Event attendee
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email_address: Option<EmailAddressSimple>,
    pub status: Option<ResponseStatus>,
    #[serde(rename = "type")]
    pub attendee_type: Option<String>,
}

/// Response status for RSVP
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseStatus {
    pub response: Option<String>,
    pub time: Option<String>,
}

/// Online meeting details
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineMeeting {
    pub join_url: Option<String>,
}

/// Calendar events list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvents {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<CalendarEvent>,
}

/// Request to create an event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventRequest {
    pub subject: String,
    pub start: DateTimeZone,
    pub end: DateTimeZone,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<EventBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<AttendeeRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_online_meeting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_meeting_provider: Option<String>,
}

/// Event body for creation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBody {
    pub content_type: String,
    pub content: String,
}

/// Attendee for event creation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttendeeRequest {
    pub email_address: EmailAddressSimple,
    #[serde(rename = "type")]
    pub attendee_type: String,
}
