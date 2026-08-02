use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DeviceProfile {
    pub profile_version: u32,
    pub vendor: String,
    pub model: String,
    pub protocol: Protocol,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub scaling: BTreeMap<String, f64>,
    #[serde(default)]
    pub faults: BTreeMap<String, FaultDefinition>,
    #[serde(default)]
    pub parameter_groups: Vec<ParameterGroup>,
}

#[derive(Debug, Deserialize)]
pub struct Protocol {
    pub default_baud_rate: u32,
    pub default_parity: String,
    pub register_type: String,
}

#[derive(Debug, Deserialize)]
pub struct FaultDefinition {
    pub code: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct ParameterGroup {
    pub group_name: String,
    pub base_address: String,
    #[serde(default)]
    pub registers: BTreeMap<String, ParameterDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ParameterDefinition {
    pub code: String,
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub unit: Option<String>,
    pub scale: Option<f64>,
}

impl DeviceProfile {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read profile {}", path.display()))?;

        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => serde_json::from_str(&source)
                .with_context(|| format!("invalid JSON profile {}", path.display())),
            Some("toml") => toml::from_str(&source)
                .with_context(|| format!("invalid TOML profile {}", path.display())),
            _ => bail!("profile must use a .json or .toml extension"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceProfile;

    #[test]
    fn parses_minimal_json_profile() {
        let source = r#"
        {
          "profile_version": 1,
          "vendor": "Example",
          "model": "Demo",
          "protocol": {
            "default_baud_rate": 9600,
            "default_parity": "none",
            "register_type": "holding"
          }
        }
        "#;

        let profile: DeviceProfile = serde_json::from_str(source).expect("profile should parse");

        assert_eq!(profile.profile_version, 1);
        assert_eq!(profile.vendor, "Example");
        assert!(profile.aliases.is_empty());
    }
}
