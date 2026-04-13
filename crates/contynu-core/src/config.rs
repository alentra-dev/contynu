use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContynuConfig {
    #[serde(default)]
    pub llm_launchers: Vec<ConfiguredLlmLauncher>,
    #[serde(default)]
    pub packet_budget: PacketBudgetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketBudgetConfig {
    #[serde(default = "default_4000")]
    pub max_total_tokens: usize,
    #[serde(default = "default_20")]
    pub max_per_category: usize,
}

impl Default for PacketBudgetConfig {
    fn default() -> Self {
        Self {
            max_total_tokens: 4000,
            max_per_category: 20,
        }
    }
}

impl PacketBudgetConfig {
    pub fn to_budget(&self) -> crate::checkpoint::PacketBudget {
        crate::checkpoint::PacketBudget {
            max_total_tokens: self.max_total_tokens,
            max_per_category: self.max_per_category,
            min_per_category: 2,
        }
    }
}

fn default_4000() -> usize {
    4000
}
fn default_20() -> usize {
    20
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
    #[serde(default)]
    pub prompt_format: Option<String>,
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
            packet_budget: PacketBudgetConfig::default(),
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
            hydration_args: Vec::new(),
            extra_env: BTreeMap::new(),
            prompt_format: None,
        },
        ConfiguredLlmLauncher {
            command: "claude".into(),
            aliases: vec!["claude-code".into()],
            hydrate: true,
            use_pty: true,
            hydration_delivery: HydrationDelivery::EnvOnly,
            hydration_args: vec![
                "--mcp-config".into(),
                ".mcp.json".into(),
                "--append-system-prompt".into(),
                "{prompt_text}".into(),
            ],
            extra_env: BTreeMap::new(),
            prompt_format: None,
        },
        ConfiguredLlmLauncher {
            command: "gemini".into(),
            aliases: vec!["gemini-cli".into()],
            hydrate: true,
            use_pty: true,
            hydration_delivery: HydrationDelivery::EnvAndStdin,
            hydration_args: vec![
                "--prompt-interactive".into(),
                "Silently read GEMINI.md in the working directory and remember its contents. Do NOT summarize it, do NOT search for anything, do NOT take any other action. Just read it and wait for the user's next message.".into(),
            ],
            extra_env: BTreeMap::new(),
            prompt_format: None,
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
    fn builtin_codex_launcher_prefers_env_only_hydration() {
        let config = ContynuConfig::with_builtin_launchers();
        assert_eq!(
            config
                .find_llm_launcher("codex")
                .unwrap()
                .hydration_delivery,
            HydrationDelivery::EnvOnly
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
            HydrationDelivery::EnvAndStdin
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
