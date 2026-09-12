//! What the app remembers between runs, other than messages.
//!
//! One file, `settings.json`, next to the device key and the mirror. It holds
//! no secrets — the device key is a separate file and passwords are never
//! stored at all — so it is ordinary JSON a user can read and edit.
//!
//! Two rules:
//!
//! - **A missing or unreadable file is not a failure.** An app that will not
//!   start because its preferences are malformed is worse than an app that
//!   starts with the defaults and says so.
//! - **Unknown fields survive nothing, but missing ones survive everything.**
//!   `#[serde(default)]` throughout, so a settings file written by an older
//!   version keeps working after an upgrade adds a field.

use std::path::{Path, PathBuf};

use he_proto::NetworkConfig;
use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Relays and discovery, for both the client endpoint and — when this
    /// machine is hosting — the server one (PLAN §4).
    pub network: NetworkConfig,
    pub hosting: Hosting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hosting {
    /// Open the space when the app opens.
    ///
    /// Off until someone deliberately hosts, because a chat app that starts
    /// accepting connections from the internet on first run would be a
    /// surprise, and surprises like that are how a "local-first" app gets a
    /// reputation it deserves.
    pub enabled: bool,
    /// The space's display name, used only when creating the database. An
    /// existing `server.db` keeps the name it was created with.
    pub space_name: String,
}

impl Default for Hosting {
    fn default() -> Self {
        Self {
            enabled: false,
            space_name: "My Space".to_string(),
        }
    }
}

impl Settings {
    /// Reads `settings.json`, falling back to the defaults.
    ///
    /// A parse failure is logged and ignored rather than propagated: the file
    /// is a convenience, and the app has to open.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "settings file is not readable; using defaults");
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "could not read settings; using defaults");
                Self::default()
            }
        }
    }

    /// Writes `settings.json`. Pretty-printed, because a user editing this
    /// file by hand is a supported way to use a self-hosted app.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(path_of(data_dir), text)
    }
}

pub fn path_of(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use he_proto::net::Relays;

    #[test]
    fn settings_round_trip_through_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = Settings {
            network: NetworkConfig {
                relays: Relays::Custom {
                    urls: vec!["https://relay.example.invalid".into()],
                },
                n0_discovery: false,
                mdns_discovery: true,
            },
            hosting: Hosting {
                enabled: true,
                space_name: "Kitchen Table".into(),
            },
        };
        settings.save(dir.path()).expect("save");
        assert_eq!(Settings::load(dir.path()), settings);
    }

    #[test]
    fn a_missing_file_is_the_defaults_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn a_corrupt_file_does_not_stop_the_app_opening() {
        // The alternative is an app that will not start because of a stray
        // comma in a preferences file it wrote itself.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(path_of(dir.path()), "{ not json").expect("write");
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn a_settings_file_from_an_older_version_keeps_working() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(path_of(dir.path()), r#"{"hosting":{"enabled":true}}"#).expect("write");
        let loaded = Settings::load(dir.path());
        assert!(loaded.hosting.enabled);
        assert_eq!(loaded.network, NetworkConfig::default());
        assert_eq!(loaded.hosting.space_name, Hosting::default().space_name);
    }

    #[test]
    fn hosting_is_off_until_somebody_asks_for_it() {
        assert!(!Settings::default().hosting.enabled);
    }
}
