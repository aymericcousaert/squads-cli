//! Teams serves each tenant from one region, and the region is part of most
//! API URLs. This module holds the known regions and builds every regional URL,
//! so no call site has to spell one out.

use std::sync::LazyLock;

use regex::Regex;

/// Region used until discovery says otherwise.
pub const DEFAULT_REGION: &str = "emea";

/// Pins the region, for debugging or for a tenant we resolve wrongly.
pub const REGION_ENV: &str = "SQUADS_REGION";

/// Regions Teams runs. A value outside this set is a parsing mistake, not a
/// new datacentre, so it is rejected rather than used.
pub const KNOWN_REGIONS: [&str; 3] = ["emea", "amer", "apac"];

/// Region-less csa endpoint. Teams answers it with a redirect to the tenant's
/// own region, which is the cheapest way to learn it.
pub const CSA_PROBE_URL: &str = "https://teams.microsoft.com/api/csa/api/v2/teams/users/me";

static REGION_PATTERNS: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(r"/api/(?:csa|chatsvc)/(\w+)/").unwrap(),
        Regex::new(r"https://(\w+)\.notifications\.skype\.net").unwrap(),
    ]
});

/// A validated Teams region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region(&'static str);

impl Default for Region {
    fn default() -> Self {
        Region(DEFAULT_REGION)
    }
}

impl Region {
    /// Accept a region name, or nothing if it is not one we know.
    pub fn parse(value: &str) -> Option<Region> {
        let value = value.trim().to_ascii_lowercase();
        KNOWN_REGIONS
            .iter()
            .find(|known| **known == value)
            .map(|known| Region(known))
    }

    pub fn name(&self) -> &'static str {
        self.0
    }

    /// Short code Teams uses inside hostnames, as opposed to URL paths.
    pub fn code(&self) -> &'static str {
        match self.0 {
            "amer" => "us",
            "apac" => "ap",
            _ => "eu",
        }
    }

    /// Messaging service: chats, messages, read horizons.
    pub fn chatsvc_base(&self) -> String {
        format!(
            "https://teams.microsoft.com/api/chatsvc/{}/v1/users/ME",
            self.0
        )
    }

    /// The same service without the `/users/ME` prefix. A thread's own
    /// endpoints hang off the service root, not off your user.
    pub fn chatsvc_thread_base(&self) -> String {
        format!("https://teams.microsoft.com/api/chatsvc/{}/v1", self.0)
    }

    /// Same service on the host the web client uses for reactions. Only that
    /// one call accepts it, so it is kept apart from `chatsvc_base`.
    pub fn chatsvc_cloud_base(&self) -> String {
        format!(
            "https://teams.cloud.microsoft/api/chatsvc/{}/v1/users/ME",
            self.0
        )
    }

    /// Aggregator service: teams, channels, the user's own chat list.
    pub fn csa_base(&self) -> String {
        format!(
            "https://teams.microsoft.com/api/csa/{}/api/v2/teams",
            self.0
        )
    }

    /// Presence pubsub subscription for a Trouter endpoint.
    pub fn ups_subscription_url(&self, endpoint_id: &str) -> String {
        format!(
            "https://teams.cloud.microsoft/ups/{}/v1/pubsub/subscriptions/{}",
            self.0, endpoint_id
        )
    }

    /// Animated custom emoji image.
    pub fn custom_emoji_url(&self, tenant_id: &str, object_id: &str) -> String {
        format!(
            "https://{}-prod.asyncgw.teams.microsoft.com/v1/{}/objects/{}/views/imgt2_anim",
            self.code(),
            tenant_id,
            object_id
        )
    }

    /// Trouter websocket to use when the service gives us no reconnect URL.
    pub fn trouter_default_url(&self) -> String {
        format!("wss://go-{}.trouter.teams.microsoft.com/v4/c", self.code())
    }
}

/// Find the region in any text holding Teams URLs: a redirect target, a chat
/// payload, a notification link.
pub fn extract_region_from_url(url: &str) -> Option<Region> {
    REGION_PATTERNS
        .iter()
        .flat_map(|pattern| pattern.captures_iter(url))
        .find_map(|caps| Region::parse(&caps[1]))
}

/// Region to start from, before any network call: the env override first, then
/// what an earlier run discovered. `None` means discovery still has to run.
pub fn initial_region(env: Option<&str>, persisted: Option<&str>) -> Option<Region> {
    [env, persisted]
        .into_iter()
        .flatten()
        .find_map(Region::parse)
}

/// Read the env override.
pub fn env_region() -> Option<String> {
    std::env::var(REGION_ENV).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_each_known_region_from_a_path() {
        for (url, expected) in [
            (
                "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/contacts/8:orgid:u1",
                "emea",
            ),
            (
                "https://teams.microsoft.com/api/csa/amer/api/v2/teams/users/me",
                "amer",
            ),
            (
                "https://teams.microsoft.com/api/chatsvc/apac/v1/users/ME/conversations/19:x",
                "apac",
            ),
        ] {
            assert_eq!(
                extract_region_from_url(url).map(|r| r.name()),
                Some(expected)
            );
        }
    }

    #[test]
    fn extracts_region_from_a_notifications_host() {
        let url = "https://emea.notifications.skype.net/v1/users/ME/conversations/19:x/messages/17";
        assert_eq!(extract_region_from_url(url).map(|r| r.name()), Some("emea"));
    }

    #[test]
    fn rejects_an_unknown_region() {
        assert_eq!(
            extract_region_from_url("https://teams.microsoft.com/api/csa/moon/api/v2/teams"),
            None
        );
        assert_eq!(
            extract_region_from_url("https://notifications.skype.net/v1/users/ME/contacts/8:x"),
            None
        );
        assert_eq!(extract_region_from_url("no url here"), None);
    }

    #[test]
    fn finds_the_region_inside_a_response_body() {
        let body = r#"{"chats":[{"lastMessage":{"conversationLink":"https://teams.microsoft.com/api/chatsvc/apac/v1/users/ME/conversations/19:x"}}]}"#;
        assert_eq!(
            extract_region_from_url(body).map(|r| r.name()),
            Some("apac")
        );
    }

    #[test]
    fn maps_regions_to_host_codes() {
        assert_eq!(Region::parse("emea").unwrap().code(), "eu");
        assert_eq!(Region::parse("amer").unwrap().code(), "us");
        assert_eq!(Region::parse("apac").unwrap().code(), "ap");
        assert_eq!(Region::default().code(), "eu");
    }

    #[test]
    fn parse_is_case_insensitive_and_validating() {
        assert_eq!(Region::parse(" AMER ").map(|r| r.name()), Some("amer"));
        assert_eq!(Region::parse("euw"), None);
        assert_eq!(Region::parse(""), None);
    }

    #[test]
    fn builds_every_regional_url() {
        let region = Region::parse("amer").unwrap();
        assert_eq!(
            region.chatsvc_base(),
            "https://teams.microsoft.com/api/chatsvc/amer/v1/users/ME"
        );
        assert_eq!(
            region.chatsvc_cloud_base(),
            "https://teams.cloud.microsoft/api/chatsvc/amer/v1/users/ME"
        );
        assert_eq!(
            region.csa_base(),
            "https://teams.microsoft.com/api/csa/amer/api/v2/teams"
        );
        assert_eq!(
            region.ups_subscription_url("ep1"),
            "https://teams.cloud.microsoft/ups/amer/v1/pubsub/subscriptions/ep1"
        );
        assert_eq!(
            region.custom_emoji_url("t1", "o1"),
            "https://us-prod.asyncgw.teams.microsoft.com/v1/t1/objects/o1/views/imgt2_anim"
        );
        assert_eq!(
            region.trouter_default_url(),
            "wss://go-us.trouter.teams.microsoft.com/v4/c"
        );
    }

    #[test]
    fn default_region_keeps_emea_urls_unchanged() {
        let region = Region::default();
        assert_eq!(region.name(), "emea");
        assert_eq!(
            region.chatsvc_base(),
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME"
        );
        assert_eq!(
            region.csa_base(),
            "https://teams.microsoft.com/api/csa/emea/api/v2/teams"
        );
    }

    #[test]
    fn env_override_wins_over_persisted() {
        assert_eq!(
            initial_region(Some("apac"), Some("amer")).map(|r| r.name()),
            Some("apac")
        );
    }

    #[test]
    fn persisted_is_used_without_an_env_override() {
        assert_eq!(
            initial_region(None, Some("amer")).map(|r| r.name()),
            Some("amer")
        );
    }

    #[test]
    fn a_bad_env_override_falls_through_to_persisted() {
        assert_eq!(
            initial_region(Some("moon"), Some("amer")).map(|r| r.name()),
            Some("amer")
        );
    }

    #[test]
    fn nothing_known_leaves_discovery_to_run() {
        assert_eq!(initial_region(None, None), None);
        assert_eq!(initial_region(Some("moon"), Some("mars")), None);
    }

    /// A fresh install with nothing pinned and nothing cached. Discovery may
    /// also fail, so every URL has to build on the default alone.
    #[test]
    fn a_fresh_install_falls_back_to_the_default() {
        let region = initial_region(None, None).unwrap_or_default();
        assert_eq!(region.name(), DEFAULT_REGION);
        assert_eq!(
            region.chatsvc_base(),
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME"
        );
        assert_eq!(
            region.csa_base(),
            "https://teams.microsoft.com/api/csa/emea/api/v2/teams"
        );
    }
}
