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
    Today {
        /// Calendar ID to use (defaults to primary calendar)
        #[arg(short, long)]
        calendar_id: Option<String>,

        /// User ID/Email to use (for shared calendars)
        #[arg(short, long)]
        user_id: Option<String>,
    },

    /// List this week's calendar events
    Week {
        /// Calendar ID to use (defaults to primary calendar)
        #[arg(short, long)]
        calendar_id: Option<String>,

        /// User ID/Email to use (for shared calendars)
        #[arg(short, long)]
        user_id: Option<String>,
    },

    /// List calendar events in a date range
    List {
        /// Start date (YYYY-MM-DD)
        #[arg(short, long)]
        start: String,

        /// End date (YYYY-MM-DD)
        #[arg(short, long)]
        end: String,

        /// Calendar ID to use (defaults to primary calendar)
        #[arg(short, long)]
        calendar_id: Option<String>,
    },

    /// Show details of a specific event
    Show {
        /// Event ID
        event_id: String,

        /// Calendar ID (optional)
        #[arg(short, long)]
        calendar_id: Option<String>,
    },

    /// Get free/busy schedule for users
    FreeBusy {
        /// User emails, comma-separated
        #[arg(short, long)]
        users: String,

        /// Date (YYYY-MM-DD), defaults to today
        #[arg(short, long)]
        date: Option<String>,
    },

    /// List available calendars
    Calendars,

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

#[derive(Debug, Serialize, Tabled)]
struct CalendarRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Default")]
    is_default: String,
    #[tabled(rename = "Owner")]
    owner: String,
}

pub async fn execute(cmd: CalendarCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        CalendarSubcommand::Today {
            calendar_id,
            user_id,
        } => today(config, calendar_id, user_id, format).await,
        CalendarSubcommand::Week {
            calendar_id,
            user_id,
        } => week(config, calendar_id, user_id, format).await,
        CalendarSubcommand::List {
            start,
            end,
            calendar_id,
        } => list(config, &start, &end, calendar_id, format).await,
        CalendarSubcommand::Show {
            event_id,
            calendar_id,
        } => show(config, &event_id, calendar_id, format).await,
        CalendarSubcommand::Calendars => calendars(config, format).await,
        CalendarSubcommand::FreeBusy { users, date } => {
            free_busy(config, &users, date, format).await
        }
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

async fn today(
    config: &Config,
    calendar_id: Option<String>,
    user_id: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let now = chrono::Utc::now();
    let start = now.format("%Y-%m-%dT00:00:00Z").to_string();
    let end = now.format("%Y-%m-%dT23:59:59Z").to_string();

    let events = if let Some(u_id) = user_id {
        client.get_user_calendar_view(&u_id, &start, &end).await?
    } else if let Some(id) = calendar_id {
        client.get_calendar_events_for_id(&id, &start, &end).await?
    } else {
        client.get_calendar_today().await?
    };
    display_events(events.value, format);
    Ok(())
}

async fn week(
    config: &Config,
    calendar_id: Option<String>,
    user_id: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let now = chrono::Utc::now();
    let start = now.format("%Y-%m-%dT00:00:00Z").to_string();
    let end = (now + chrono::Duration::days(7))
        .format("%Y-%m-%dT23:59:59Z")
        .to_string();

    let events = if let Some(u_id) = user_id {
        client.get_user_calendar_view(&u_id, &start, &end).await?
    } else if let Some(id) = calendar_id {
        client.get_calendar_events_for_id(&id, &start, &end).await?
    } else {
        client.get_calendar_week().await?
    };
    display_events(events.value, format);
    Ok(())
}

async fn list(
    config: &Config,
    start: &str,
    end: &str,
    calendar_id: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let start_dt = format!("{}T00:00:00Z", start);
    let end_dt = format!("{}T23:59:59Z", end);

    let events = if let Some(id) = calendar_id {
        client
            .get_calendar_events_for_id(&id, &start_dt, &end_dt)
            .await?
    } else {
        client.get_calendar_events(&start_dt, &end_dt).await?
    };
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

async fn show(
    config: &Config,
    event_id: &str,
    _calendar_id: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    // For now, global event ID lookup usually works in Graph for accessible events
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

async fn free_busy(
    config: &Config,
    users: &str,
    date: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let target_date = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let start = format!("{}T00:00:00Z", target_date);
    let end = format!("{}T23:59:59Z", target_date);

    let user_list: Vec<&str> = users.split(',').map(|u| u.trim()).collect();
    let schedule = client.get_schedule(user_list, &start, &end).await?;

    match format {
        OutputFormat::Json => {
            print_single(&schedule, format);
        }
        _ => {
            if let Some(value) = schedule.get("value").and_then(|v| v.as_array()) {
                for item in value {
                    let user = item
                        .get("scheduleId")
                        .and_then(|id| id.as_str())
                        .unwrap_or("Unknown");
                    println!("\n📅 Schedule for: {}", user);

                    if let Some(items) = item.get("scheduleItems").and_then(|i| i.as_array()) {
                        if items.is_empty() {
                            println!("  No events found");
                        } else {
                            for evt in items {
                                let start_time = evt
                                    .get("start")
                                    .and_then(|s| s.get("dateTime"))
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                let end_time = evt
                                    .get("end")
                                    .and_then(|e| e.get("dateTime"))
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                let status =
                                    evt.get("status").and_then(|s| s.as_str()).unwrap_or("busy");
                                let subject = evt
                                    .get("subject")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("(No subject)");

                                let st = if start_time.len() >= 16 {
                                    &start_time[11..16]
                                } else {
                                    start_time
                                };
                                let et = if end_time.len() >= 16 {
                                    &end_time[11..16]
                                } else {
                                    end_time
                                };

                                println!("  - {} - {}: {} [{}]", st, et, subject, status);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn calendars(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let calendars = client.get_all_calendars().await?;

    let rows: Vec<CalendarRow> = calendars
        .into_iter()
        .map(|c| {
            let owner = c
                .owner
                .and_then(|o| o.email_address)
                .map(|e| e.name.unwrap_or(e.address.unwrap_or_default()))
                .unwrap_or_else(|| "Unknown".to_string());

            CalendarRow {
                id: c.id,
                name: c.name.unwrap_or_else(|| "Unnamed".to_string()),
                is_default: if c.is_default_calendar == Some(true) {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
                owner,
            }
        })
        .collect();

    print_output(&rows, format);
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
