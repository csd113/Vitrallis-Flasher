//! Desktop choice is bound into the image manifest and its erase confirmation.
use crate::Error;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    #[default]
    Stock,
    VitrallisDefault,
}
impl Profile {
    pub const ALL: [Self; 2] = [Self::Stock, Self::VitrallisDefault];

    /// Parses the explicit, closed set of installation profiles.
    /// # Errors
    /// Rejects unknown names instead of silently changing the desktop choice.
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "stock" => Ok(Self::Stock),
            "vitrallis-default" => Ok(Self::VitrallisDefault),
            _ => Err(Error::Manifest("unknown installation profile")),
        }
    }
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stock => "stock",
            Self::VitrallisDefault => "vitrallis-default",
        }
    }
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stock => "Debian 13 + PocketHome (stock desktop)",
            Self::VitrallisDefault => "Debian 13 + Vitrallis as the default desktop",
        }
    }
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Stock => {
                "Keep PocketHome/Marshmallow as the startup desktop. Do not install Vitrallis."
            }
            Self::VitrallisDefault => {
                "Install the complete Vitrallis bundle and start it automatically. Keep PocketHome/Marshmallow available as a fallback."
            }
        }
    }
    #[must_use]
    pub const fn simulation_release(self) -> &'static str {
        match self {
            Self::Stock => "simulation-debian13",
            Self::VitrallisDefault => "simulation-debian13-vitrallis-default",
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_never_opts_into_vitrallis() {
        assert_eq!(Profile::default(), Profile::Stock);
        assert!(Profile::parse("vitrallis").is_err());
        for profile in Profile::ALL {
            assert_eq!(Profile::parse(profile.id()).ok(), Some(profile));
        }
    }
}
