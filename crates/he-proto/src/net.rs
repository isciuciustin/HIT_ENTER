//! How an endpoint finds the rest of the world — the sovereignty ladder,
//! as a struct (PLAN §4).
//!
//! Behind the off-by-default `net` feature, for the same reason [`crate::io`]
//! is: the default build of this crate is types with no runtime, and turning
//! a [`NetworkConfig`] into a bound endpoint needs all of iroh.
//!
//! It lives here rather than in `he-client` or `he-server` because **a host
//! runs both**. One settings file configures the endpoint the app dials out
//! of and the endpoint it hosts on, and two copies of "apply this config to a
//! builder" is two chances for those to quietly disagree about which relays
//! exist — which would show up as a space that is reachable from the internet
//! but not from the machine hosting it.
//!
//! Every rung of the ladder is reachable without us:
//!
//! 1. **LAN only** — mDNS, no relay, no DNS. Works with the internet
//!    unplugged.
//! 2. **Default** — n0's discovery and relays. Zero config; hole punching
//!    means the relay is usually only a fallback.
//! 3. **Your own relay** — [`Relays::Custom`]. The relay binary is open
//!    source and in the iroh repository.
//!
//! We never ship a relay *we* operate as a default. n0's are a documented,
//! replaceable convenience, not a dependency on this project.

use serde::{Deserialize, Serialize};

/// Which relays to use when a direct path cannot be punched.
///
/// A relay forwards bytes it cannot read: the connection through it is still
/// end-to-end encrypted by QUIC + TLS 1.3. What a relay operator learns is
/// metadata — who talks to whom, and when (PLAN §4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Relays {
    /// n0's public relays. Free, rate-limited, and documented by n0 as having
    /// no uptime guarantee. The default because it works with no
    /// configuration, not because it is anybody's infrastructure of record.
    #[default]
    N0,
    /// Relays you run. See the iroh repository for the relay binary.
    Custom { urls: Vec<String> },
    /// No relay at all. Peers that cannot be punched to are unreachable —
    /// which on a LAN, with mDNS, is fine.
    Disabled,
}

impl Relays {
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }
}

/// Relay and discovery settings, shared by the client endpoint and the hosted
/// server endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub relays: Relays,
    /// Publish to and resolve from n0's DNS (`dns.iroh.link`). This is how a
    /// bare `EndpointId` becomes an address when there are no hints in the
    /// ticket and no mDNS on the wire.
    pub n0_discovery: bool,
    /// Announce on the local network and listen for announcements. Needs no
    /// infrastructure whatsoever, and is the rung of the ladder that survives
    /// the internet being unplugged.
    pub mdns_discovery: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            relays: Relays::N0,
            n0_discovery: true,
            mdns_discovery: true,
        }
    }
}

impl NetworkConfig {
    /// Nothing outside this machine's LAN. What the end-to-end tests use, and
    /// the honest name for what `--offline` means.
    pub fn lan_only() -> Self {
        Self {
            relays: Relays::Disabled,
            n0_discovery: false,
            mdns_discovery: true,
        }
    }

    /// True when nothing here would ever contact infrastructure we did not
    /// configure ourselves. Worth surfacing in the UI, because "nobody can
    /// take this away from you" is the pitch and this is the check.
    pub fn is_self_contained(&self) -> bool {
        !self.n0_discovery && !matches!(self.relays, Relays::N0)
    }

    /// A one-line description for the settings pane.
    pub fn describe(&self) -> String {
        let relays = match &self.relays {
            Relays::N0 => "n0 relays".to_string(),
            Relays::Custom { urls } if urls.len() == 1 => "your relay".to_string(),
            Relays::Custom { urls } => format!("{} of your relays", urls.len()),
            Relays::Disabled => "no relays".to_string(),
        };
        let mut discovery = Vec::new();
        if self.n0_discovery {
            discovery.push("n0 DNS");
        }
        if self.mdns_discovery {
            discovery.push("local network");
        }
        if discovery.is_empty() {
            return format!("{relays}, no discovery");
        }
        format!("{relays}, discovery via {}", discovery.join(" and "))
    }
}

#[cfg(feature = "net")]
mod apply {
    use iroh::address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver};
    use iroh::endpoint::{Builder, presets};
    use iroh::{RelayMode, RelayUrl};
    use iroh_mdns_address_lookup::MdnsAddressLookup;

    use super::{NetworkConfig, Relays};

    /// A relay URL in the settings file that is not a URL.
    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    #[error("{0:?} is not a relay URL")]
    pub struct BadRelayUrl(pub String);

    impl NetworkConfig {
        /// Configures an endpoint builder from these settings.
        ///
        /// Starts from [`presets::Minimal`] — which sets only the mandatory
        /// crypto provider — and adds each service deliberately. Using
        /// [`presets::N0`] and then subtracting would mean a future addition
        /// to that preset silently switched itself on for everybody.
        pub fn apply(&self, mut builder: Builder) -> Result<Builder, BadRelayUrl> {
            builder = builder.preset(presets::Minimal);

            if self.n0_discovery {
                builder = builder
                    .address_lookup(PkarrPublisher::n0_dns())
                    .address_lookup(PkarrResolver::n0_dns())
                    .address_lookup(DnsAddressLookup::n0_dns());
            }
            if self.mdns_discovery {
                builder = builder.address_lookup(MdnsAddressLookup::builder());
            }

            builder = builder.relay_mode(self.relay_mode()?);
            Ok(builder)
        }

        fn relay_mode(&self) -> Result<RelayMode, BadRelayUrl> {
            match &self.relays {
                Relays::N0 => Ok(iroh::endpoint::default_relay_mode()),
                Relays::Disabled => Ok(RelayMode::Disabled),
                Relays::Custom { urls } => {
                    let parsed = urls
                        .iter()
                        .map(|url| {
                            url.parse::<RelayUrl>()
                                .map_err(|_| BadRelayUrl(url.clone()))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    // An empty custom list is "I meant to turn relays off",
                    // not "use n0's after all".
                    Ok(if parsed.is_empty() {
                        RelayMode::Disabled
                    } else {
                        RelayMode::custom(parsed)
                    })
                }
            }
        }
    }
}

#[cfg(feature = "net")]
pub use apply::BadRelayUrl;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_zero_config_rung() {
        let config = NetworkConfig::default();
        assert_eq!(config.relays, Relays::N0);
        assert!(config.n0_discovery);
        assert!(
            config.mdns_discovery,
            "the LAN rung costs nothing and is the one that works with the \
             internet unplugged"
        );
        assert!(!config.is_self_contained());
    }

    #[test]
    fn a_config_that_touches_nobody_elses_infrastructure_says_so() {
        let config = NetworkConfig {
            relays: Relays::Custom {
                urls: vec!["https://relay.example.invalid".into()],
            },
            n0_discovery: false,
            mdns_discovery: true,
        };
        assert!(config.is_self_contained());
        assert!(NetworkConfig::lan_only().is_self_contained());
    }

    #[test]
    fn settings_round_trip_through_the_settings_file() {
        for config in [
            NetworkConfig::default(),
            NetworkConfig::lan_only(),
            NetworkConfig {
                relays: Relays::Custom {
                    urls: vec!["https://relay.example.invalid".into()],
                },
                n0_discovery: false,
                mdns_discovery: false,
            },
        ] {
            let json = serde_json::to_string(&config).expect("serialisable");
            assert_eq!(
                serde_json::from_str::<NetworkConfig>(&json).expect("parsable"),
                config
            );
        }
    }

    #[test]
    fn a_settings_file_from_an_older_version_still_loads() {
        // `#[serde(default)]` on the struct: a field added later must not
        // make an existing installation fail to start.
        let config: NetworkConfig = serde_json::from_str("{}").expect("parsable");
        assert_eq!(config, NetworkConfig::default());
    }

    #[test]
    fn every_configuration_describes_itself() {
        assert_eq!(
            NetworkConfig::default().describe(),
            "n0 relays, discovery via n0 DNS and local network"
        );
        assert_eq!(
            NetworkConfig::lan_only().describe(),
            "no relays, discovery via local network"
        );
        assert_eq!(
            NetworkConfig {
                relays: Relays::Disabled,
                n0_discovery: false,
                mdns_discovery: false,
            }
            .describe(),
            "no relays, no discovery"
        );
    }
}
