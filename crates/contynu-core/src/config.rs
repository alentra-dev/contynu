use std::collections::{BTreeMap, BTreeSet};
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
    pub use_pty: bool,
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
            return Ok(Self::with_builtin_launchers());
        }
        let raw = fs::read_to_string(path)?;
        let config = serde_json::from_str::<Self>(&raw)?;
        config.validate()?;
        Ok(config)
    }

    pub fn ensure_exists(path: &Path) -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, Self::default_file_contents()?)?;
        Ok(())
    }

    pub fn default_file_contents() -> Result<String> {
        Ok(serde_json::to_string_pretty(
            &Self::with_builtin_launchers(),
        )?)
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for launcher in &self.llm_launchers {
            if launcher.command.trim().is_empty() {
                return Err(crate::error::ContynuError::Validation(
                    "launcher command must not be empty".into(),
                ));
            }
            for name in std::iter::once(&launcher.command).chain(launcher.aliases.iter()) {
                if !seen.insert(name.clone()) {
                    return Err(crate::error::ContynuError::Validation(format!(
                        "duplicate launcher name or alias `{name}`"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn find_llm_launcher(&self, command: &str) -> Option<&ConfiguredLlmLauncher> {
        self.llm_launchers.iter().find(|launcher| {
            launcher.command == command || launcher.aliases.iter().any(|alias| alias == command)
        })
    }

    pub fn with_builtin_launchers() -> Self {
        Self {
            llm_launchers: builtin_launchers(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn builtin_launchers() -> Vec<ConfiguredLlmLauncher> {
    vec![
        ConfiguredLlmLauncher {
            command: "codex".into(),
            aliases: vec!["codex-cli".into()],
            hydrate: true,
            use_pty: true,
            hydration_delivery: HydrationDelivery::EnvOnly,
            hydration_args: vec!["{launcher_prompt}".into()],
            extra_env: BTreeMap::new(),
        },
        ConfiguredLlmLauncher {
            command: "claude".into(),
            aliases: vec!["claude-code".into()],
            hydrate: true,
            use_pty: true,
            hydration_delivery: HydrationDelivery::EnvOnly,
            hydration_args: vec!["--append-system-prompt".into(), "{launcher_prompt}".into()],
            extra_env: BTreeMap::new(),
        },
        ConfiguredLlmLauncher {
            command: "gemini".into(),
            aliases: vec!["gemini-cli".into()],
            hydrate: true,
            use_pty: true,
            hydration_delivery: HydrationDelivery::EnvOnly,
            hydration_args: vec!["--prompt-interactive".into(), "{launcher_prompt}".into()],
            extra_env: BTreeMap::new(),
        },
    ]
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
                  "use_pty": true,
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
        assert!(config.find_llm_launcher("futurellm").unwrap().use_pty);
        assert_eq!(
            config
                .find_llm_launcher("futurellm")
                .unwrap()
                .hydration_args,
            vec!["--context-file", "{prompt_file}"]
        );
    }

    #[test]
    fn ensure_exists_writes_builtin_launchers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");

        ContynuConfig::ensure_exists(&path).unwrap();

        let config = ContynuConfig::load(&path).unwrap();
        assert_eq!(
            config
                .find_llm_launcher("codex")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvOnly
        );
        assert_eq!(
            config
                .find_llm_launcher("claude")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvOnly
        );
        assert_eq!(
            config
                .find_llm_launcher("gemini")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvOnly
        );
    }

    #[test]
    fn duplicate_launcher_names_are_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "llm_launchers": [
                {"command": "codex"},
                {"command": "codex"}
              ]
            }"#,
        )
        .unwrap();

        assert!(ContynuConfig::load(&path).is_err());
    }
}
