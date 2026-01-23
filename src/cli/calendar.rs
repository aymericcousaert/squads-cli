use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;
use crate::types::{
    AttendeeRequest, CreateEventRequest, DateTimeZone, EmailAddressSimple, EventBody, Location,
};

use super::output::{print_output, print_single, print_success};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct CalendarCommand {
    #[command(subcommand)]
    pub command: CalendarSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum CalendarSubcommand {
    /// List today's calendar events
    Today,

    /// List this week's calendar events
    Week,

    /// List calendar events in a date range
    List {
        /// Start date (YYYY-MM-DD)
        #[arg(short, long)]
        start: String,

        /// End date (YYYY-MM-DD)
        #[arg(short, long)]
        end: String,
    },

    /// Show details of a specific event
    Show {
        /// Event ID
        event_id: String,
    },

    /// Create a new calendar event
    Create {
        /// Event subject/title
        #[arg(short = 'T', long)]
        title: String,

        /// Start datetime (YYYY-MM-DDTHH:MM)
        #[arg(short, long)]
        start: String,

        /// End datetime (YYYY-MM-DDTHH:MM)
        #[arg(short, long)]
        end: String,

        /// Attendees (comma-separated emails)
        #[arg(short, long)]
        attendees: Option<String>,

        /// Location
        #[arg(short, long)]
        location: Option<String>,

        /// Make it a Teams meeting
        #[arg(long)]
        teams: bool,

        /// Description/body
        #[arg(short, long)]
        body: Option<String>,
    },

    /// RSVP to an event
    Rsvp {
        /// Event ID
        event_id: String,

        /// Response: accept, decline, or tentative
        #[arg(short, long)]
        response: String,

        /// Optional comment
        #[arg(short, long)]
        comment: Option<String>,
    },

    /// Delete a calendar event
    Delete {
        /// Event ID
        event_id: String,
    },

    /// Get the join URL for a Teams meeting
    Join {
        /// Event ID (optional - joins next meeting if not provided)
        event_id: Option<String>,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct EventRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Subject")]
    subject: String,
    #[tabled(rename = "Location")]
    location: String,
    #[tabled(rename = "Status")]
    status: String,
}

pub async fn execute(cmd: CalendarCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        CalendarSubcommand::Today => today(config, format).await,
        CalendarSubcommand::Week => week(config, format).await,
        CalendarSubcommand::List { start, end } => list(config, &start, &end, format).await,
        CalendarSubcommand::Show { event_id } => show(config, &event_id, format).await,
        CalendarSubcommand::Create {
            title,
            start,
            end,
            attendees,
            location,
            teams,
            body,
        } => {
            create(
                config, &title, &start, &end, attendees, location, teams, body, format,
            )
            .await
        }
        CalendarSubcommand::Rsvp {
            event_id,
            response,
            comment,
        } => rsvp(config, &event_id, &response, comment).await,
        CalendarSubcommand::Delete { event_id } => delete(config, &event_id).await,
        CalendarSubcommand::Join { event_id } => join(config, event_id, format).await,
    }
}

async fn today(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let events = client.get_calendar_today().await?;
    display_events(events.value, format);
    Ok(())
}

async fn week(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let events = client.get_calendar_week().await?;
    display_events(events.value, format);
    Ok(())
}

async fn list(config: &Config, start: &str, end: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let start_dt = format!("{}T00:00:00Z", start);
    let end_dt = format!("{}T23:59:59Z", end);
    let events = client.get_calendar_events(&start_dt, &end_dt).await?;
    display_events(events.value, format);
    Ok(())
}

fn display_events(events: Vec<crate::types::CalendarEvent>, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            print_single(&events, format);
        }
        _ => {
            if events.is_empty() {
                println!("No events found");
                return;
            }

            let rows: Vec<EventRow> = events
                .into_iter()
                .map(|e| {
                    let time = e
                        .start
                        .map(|s| {
                            // Extract just the date and time
                            let dt = &s.date_time;
                            if dt.len() >= 16 {
                                format!("{} {}", &dt[5..10], &dt[11..16])
                            } else {
                                dt.clone()
                            }
                        })
                        .unwrap_or_default();

                    let location = e
                        .location
                        .and_then(|l| l.display_name)
                        .or_else(|| {
                            if e.is_online_meeting == Some(true) {
                                Some("Teams Meeting".to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    let status = e
                        .response_status
                        .and_then(|r| r.response)
                        .unwrap_or_else(|| "none".to_string());

                    EventRow {
                        id: truncate(&e.id.unwrap_or_default(), 12),
                        time,
                        subject: truncate(&e.subject.unwrap_or_default(), 35),
                        location: truncate(&location, 20),
                        status,
                    }
                })
                .collect();

            print_output(&rows, format);
        }
    }
}

async fn show(config: &Config, event_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let event = client.get_calendar_event(event_id).await?;

    match format {
        OutputFormat::Json => {
            print_single(&event, format);
        }
        _ => {
            println!("Subject: {}", event.subject.unwrap_or_default());

            if let Some(start) = event.start {
                println!("Start: {} ({})", start.date_time, start.time_zone);
            }
            if let Some(end) = event.end {
                println!("End: {} ({})", end.date_time, end.time_zone);
            }

            if let Some(loc) = event.location.and_then(|l| l.display_name) {
                println!("Location: {}", loc);
            }

            if let Some(organizer) = event.organizer.and_then(|o| o.email_address) {
                let name = organizer.name.unwrap_or_default();
                let email = organizer.address.unwrap_or_default();
                println!("Organizer: {} <{}>", name, email);
            }

            if let Some(attendees) = event.attendees {
                if !attendees.is_empty() {
                    println!("Attendees:");
                    for att in attendees {
                        if let Some(email) = att.email_address {
                            let name = email.name.unwrap_or_default();
                            let addr = email.address.unwrap_or_default();
                            let status = att
                                .status
                                .and_then(|s| s.response)
                                .unwrap_or_else(|| "none".to_string());
                            println!("  - {} <{}> ({})", name, addr, status);
                        }
                    }
                }
            }

            if event.is_online_meeting == Some(true) {
                if let Some(url) = event
                    .online_meeting
                    .and_then(|m| m.join_url)
                    .or(event.online_meeting_url)
                {
                    println!("Join URL: {}", url);
                }
            }

            if let Some(preview) = event.body_preview {
                if !preview.is_empty() {
                    println!("---");
                    println!("{}", preview);
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn create(
    config: &Config,
    title: &str,
    start: &str,
    end: &str,
    attendees: Option<String>,
    location: Option<String>,
    teams: bool,
    body: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    // Parse attendees
    let attendee_list: Option<Vec<AttendeeRequest>> = attendees.map(|a| {
        a.split(',')
            .map(|email| AttendeeRequest {
                email_address: EmailAddressSimple {
                    name: None,
                    address: Some(email.trim().to_string()),
                },
                attendee_type: "required".to_string(),
            })
            .collect()
    });

    let request = CreateEventRequest {
        subject: title.to_string(),
        start: DateTimeZone {
            date_time: format!("{}:00", start),
            time_zone: "UTC".to_string(),
        },
        end: DateTimeZone {
            date_time: format!("{}:00", end),
            time_zone: "UTC".to_string(),
        },
        body: body.map(|b| EventBody {
            content_type: "text".to_string(),
            content: b,
        }),
        location: location.map(|l| Location {
            display_name: Some(l),
            location_uri: None,
        }),
        attendees: attendee_list,
        is_online_meeting: if teams { Some(true) } else { None },
        online_meeting_provider: if teams {
            Some("teamsForBusiness".to_string())
        } else {
            None
        },
    };

    let event = client.create_calendar_event(request).await?;

    match format {
        OutputFormat::Json => {
            print_single(&event, format);
        }
        _ => {
            print_success(&format!(
                "Event created: {}",
                event.subject.unwrap_or_default()
            ));
            if let Some(id) = event.id {
                println!("ID: {}", id);
            }
            if teams {
                if let Some(url) = event
                    .online_meeting
                    .and_then(|m| m.join_url)
                    .or(event.online_meeting_url)
                {
                    println!("Teams URL: {}", url);
                }
            }
        }
    }

    Ok(())
}

async fn rsvp(
    config: &Config,
    event_id: &str,
    response: &str,
    comment: Option<String>,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client
        .rsvp_calendar_event(event_id, response, comment.as_deref())
        .await?;
    print_success(&format!("RSVP sent: {}", response));
    Ok(())
}

async fn delete(config: &Config, event_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.delete_calendar_event(event_id).await?;
    print_success("Event deleted");
    Ok(())
}

async fn join(config: &Config, event_id: Option<String>, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let event = if let Some(id) = event_id {
        client.get_calendar_event(&id).await?
    } else {
        // Find the next meeting with a join URL
        let events = client.get_calendar_today().await?;
        let now = chrono::Utc::now();

        events
            .value
            .into_iter()
            .find(|e| {
                e.is_online_meeting == Some(true)
                    && e.start
                        .as_ref()
                        .map(|s| {
                            chrono::DateTime::parse_from_rfc3339(&format!("{}Z", s.date_time))
                                .map(|dt| dt > now - chrono::Duration::minutes(30))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("No upcoming Teams meetings found today"))?
    };

    let join_url = event
        .online_meeting
        .and_then(|m| m.join_url)
        .or(event.online_meeting_url)
        .ok_or_else(|| anyhow::anyhow!("No join URL found for this event"))?;

    match format {
        OutputFormat::Json => {
            print_single(
                &serde_json::json!({
                    "subject": event.subject,
                    "joinUrl": join_url
                }),
                format,
            );
        }
        _ => {
            println!("Meeting: {}", event.subject.unwrap_or_default());
            println!("Join URL: {}", join_url);
            print_success("Copy the URL above to join the meeting");
        }
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_len {
        let truncated: String = chars[..max_len.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}
