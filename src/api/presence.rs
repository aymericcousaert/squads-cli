//! Picking your own availability, and reading anyone's.
//!
//! Teams works out an availability from the clients reporting for you, and a
//! client can also state one outright — what the menu in Teams does. Only the
//! outright statement is open to us: the endpoint activity report the web
//! client uses answers 404 for this token, and the endpoint list it belongs to
//! answers 410 Gone.
//!
//! Graph refuses presence for this token as well, so the presence service is
//! the only way to read one, including your own.

use anyhow::{anyhow, Result};
use serde_json::json;

use super::{TeamsClient, SCOPE_PRESENCE};
use crate::types::Presences;

/// A state you can ask for.
///
/// Fewer than the service reports back: the idle and in-a-call variants are
/// its own doing, not something a client asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Busy,
    DoNotDisturb,
    BeRightBack,
    Away,
    /// Hands the state back to the service, which is "Reset status" in Teams.
    Reset,
}

impl Availability {
    /// The value as the service spells it.
    ///
    /// A reset goes as `Offline`, because that is what the service does with
    /// it: sending it drops whatever state was forced and leaves you at the
    /// one your clients earn you. It will not make you appear offline.
    pub fn wire(&self) -> &'static str {
        match self {
            Availability::Available => "Available",
            Availability::Busy => "Busy",
            Availability::DoNotDisturb => "DoNotDisturb",
            Availability::BeRightBack => "BeRightBack",
            Availability::Away => "Away",
            Availability::Reset => "Offline",
        }
    }

    /// Reads what someone typed. Spelling and case are not worth being strict
    /// about, so `do-not-disturb`, `donotdisturb` and `dnd` are one state.
    ///
    /// `offline` is missing on purpose: the service takes it as a reset, so
    /// anyone asking for it would get the opposite of what they meant.
    pub fn parse(value: &str) -> Option<Availability> {
        let flat: String = value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match flat.as_str() {
            "available" | "online" => Some(Availability::Available),
            "busy" => Some(Availability::Busy),
            "donotdisturb" | "dnd" => Some(Availability::DoNotDisturb),
            "berightback" | "brb" => Some(Availability::BeRightBack),
            "away" => Some(Availability::Away),
            "reset" | "auto" => Some(Availability::Reset),
            _ => None,
        }
    }
}

impl TeamsClient {
    /// State ourselves as `availability`, the way the menu in Teams does.
    ///
    /// It outlives the process: the service holds the state until something
    /// replaces it, so this is a statement about you, not about this run.
    pub async fn set_availability(&self, availability: Availability) -> Result<()> {
        let url = self.regional().await.ups_force_availability_url();
        let body = json!({"availability": availability.wire()});
        let token = self.get_token(SCOPE_PRESENCE).await?;
        let res = self
            .ups_request(self.http.put(&url), &token.value)
            .body(body.to_string())
            .send()
            .await?;
        ups_result(res, "availability").await.map(|_| ())
    }

    /// Presence of the people named, ourselves included.
    ///
    /// The answer holds what the rest of the company sees, so it is also how
    /// you check whether you look offline to them.
    pub async fn get_ups_presences(&self, user_ids: &[String]) -> Result<Presences> {
        let url = self.regional().await.ups_getpresence_url();
        let body: Vec<_> = user_ids
            .iter()
            .map(|id| json!({"mri": format!("8:orgid:{id}")}))
            .collect();
        let token = self.get_token(SCOPE_PRESENCE).await?;
        let res = self
            .ups_request(self.http.post(&url), &token.value)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;
        let text = ups_result(res, "presence").await?;
        // The service answers with a bare array, where the push frame carries
        // an object, so it is wrapped here rather than given a second type.
        Ok(Presences {
            presence: serde_json::from_str(&text)?,
            is_snapshot: Some(true),
        })
    }

    /// The headers every presence call carries. The service reads the client
    /// type and the endpoint id off them.
    fn ups_request(
        &self,
        builder: reqwest::RequestBuilder,
        bearer: &str,
    ) -> reqwest::RequestBuilder {
        builder
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", bearer))
            .header("x-ms-client-user-agent", "Teams-V2-Web")
            .header("x-ms-client-version", super::CLIENT_VERSION)
            .header("x-ms-client-type", "cdlworker")
            .header("x-ms-endpoint-id", self.trouter_epid())
            .header("x-ms-correlation-id", uuid::Uuid::new_v4().to_string())
    }
}

/// Body of a presence answer, or an error carrying what the service said. A
/// status alone does not tell a wrong path from a refused token: the gateway
/// answers 401 to any route or verb it does not have.
async fn ups_result(res: reqwest::Response, what: &str) -> Result<String> {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(body)
    } else {
        Err(anyhow!("{what} returned {status}: {body}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_reads_the_spellings_people_use() {
        assert_eq!(Availability::parse("dnd"), Some(Availability::DoNotDisturb));
        assert_eq!(
            Availability::parse("Do Not Disturb"),
            Some(Availability::DoNotDisturb)
        );
        assert_eq!(
            Availability::parse("be-right-back"),
            Some(Availability::BeRightBack)
        );
        assert_eq!(
            Availability::parse("AVAILABLE"),
            Some(Availability::Available)
        );
        assert_eq!(Availability::parse("lunch"), None);
    }

    #[test]
    fn offline_is_not_a_state_you_can_ask_for() {
        assert_eq!(Availability::parse("offline"), None);
    }

    #[test]
    fn a_reset_goes_as_the_value_that_clears_a_forced_state() {
        assert_eq!(Availability::Reset.wire(), "Offline");
        assert_eq!(Availability::DoNotDisturb.wire(), "DoNotDisturb");
    }
}
