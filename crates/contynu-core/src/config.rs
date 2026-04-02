use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContynuConfig {
    #[serde(default)]
    pub llm_launchers: Vec<ConfiguredLlmLauncher>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfiguredLlmLauncher {
    pub command: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default = "default_true")]
    pub hydrate: bool,
    #[serde(default)]
    pub hydration_delivery: HydrationDelivery,
    #[serde(default)]
    pub hydration_args: Vec<String>,
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HydrationDelivery {
    EnvOnly,
    StdinOnly,
    #[default]
    EnvAndStdin,
}

impl HydrationDelivery {
    pub fn includes_env(self) -> bool {
        matches!(self, Self::EnvOnly | Self::EnvAndStdin)
    }

    pub fn includes_stdin(self) -> bool {
        matches!(self, Self::StdinOnly | Self::EnvAndStdin)
    }
}

impl ContynuConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn find_llm_launcher(&self, command: &str) -> Option<&ConfiguredLlmLauncher> {
        self.llm_launchers.iter().find(|launcher| {
            launcher.command == command || launcher.aliases.iter().any(|alias| alias == command)
        })
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{ContynuConfig, HydrationDelivery};

    #[test]
    fn config_can_match_custom_launcher_aliases() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "llm_launchers": [
                {"command": "myllm", "aliases": ["myllm-cli"], "hydrate": true}
              ]
            }"#,
        )
        .unwrap();

        let config = ContynuConfig::load(&path).unwrap();
        assert!(config.find_llm_launcher("myllm").is_some());
        assert!(config.find_llm_launcher("myllm-cli").is_some());
        assert_eq!(
            config
                .find_llm_launcher("myllm")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvAndStdin
        );
    }

    #[test]
    fn config_supports_custom_hydration_delivery() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "llm_launchers": [
                {
                  "command": "futurellm",
                  "hydrate": true,
                  "hydration_delivery": "env_only",
                  "hydration_args": ["--context-file", "{prompt_file}"]
                }
              ]
            }"#,
        )
        .unwrap();

        let config = ContynuConfig::load(&path).unwrap();
        assert_eq!(
            config
                .find_llm_launcher("futurellm")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvOnly
        );
        assert_eq!(
            config
                .find_llm_launcher("futurellm")
                .unwrap()
                .hydration_args,
            vec!["--context-file", "{prompt_file}"]
        );
    }
}
