use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::checkpoint::RehydrationPacket;
use crate::config::{ConfiguredLlmLauncher, ContynuConfig, HydrationDelivery};
use crate::error::Result;
use crate::ids::ProjectId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Terminal,
    CodexCli,
    ClaudeCli,
    GeminiCli,
    ConfiguredLlm,
}

pub trait Adapter {
    fn kind(&self) -> AdapterKind;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct AdapterSpec {
    kind: AdapterKind,
    name: String,
    should_hydrate: bool,
    hydration_delivery: HydrationDelivery,
    extra_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub executable: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    pub stdin_prelude: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct HydrationContext {
    pub project_id: ProjectId,
    pub packet: RehydrationPacket,
    pub packet_path: PathBuf,
    pub prompt_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalAdapter;

impl Adapter for TerminalAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::Terminal
    }

    fn name(&self) -> &'static str {
        "terminal"
    }
}

impl AdapterSpec {
    pub fn detect(program: &str, config: &ContynuConfig) -> Self {
        match program {
            "codex" | "codex-cli" => Self::builtin(AdapterKind::CodexCli, "codex_cli", true),
            "claude" | "claude-code" => Self::builtin(AdapterKind::ClaudeCli, "claude_cli", true),
            "gemini" | "gemini-cli" => Self::builtin(AdapterKind::GeminiCli, "gemini_cli", true),
            _ => {
                if let Some(launcher) = config.find_llm_launcher(program) {
                    Self::configured(launcher)
                } else {
                    Self::builtin(AdapterKind::Terminal, "terminal", false)
                }
            }
        }
    }

    pub fn kind(&self) -> AdapterKind {
        self.kind
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }

    pub fn should_hydrate(&self) -> bool {
        self.should_hydrate
    }

    pub fn build_launch_plan(
        &self,
        executable: OsString,
        args: Vec<OsString>,
        hydration: Option<&HydrationContext>,
    ) -> Result<LaunchPlan> {
        let mut env = self
            .extra_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let mut stdin_prelude = None;

        if let Some(hydration) = hydration {
            if self.hydration_delivery.includes_env() {
                env.push((
                    "CONTYNU_PROJECT_ID".into(),
                    hydration.project_id.as_str().to_string(),
                ));
                env.push((
                    "CONTYNU_REHYDRATION_PACKET_FILE".into(),
                    hydration.packet_path.display().to_string(),
                ));
                env.push((
                    "CONTYNU_REHYDRATION_PROMPT_FILE".into(),
                    hydration.prompt_path.display().to_string(),
                ));
                env.push((
                    "CONTYNU_REHYDRATION_SCHEMA_VERSION".into(),
                    hydration.packet.schema_version.to_string(),
                ));
            }
            if self.hydration_delivery.includes_stdin() {
                stdin_prelude =
                    Some(render_stdin_prelude(self.as_str(), &hydration.packet).into_bytes());
            }
        }

        Ok(LaunchPlan {
            executable,
            args,
            env,
            stdin_prelude,
        })
    }

    fn builtin(kind: AdapterKind, name: &str, should_hydrate: bool) -> Self {
        Self {
            kind,
            name: name.into(),
            should_hydrate,
            hydration_delivery: HydrationDelivery::EnvAndStdin,
            extra_env: BTreeMap::new(),
        }
    }

    fn configured(launcher: &ConfiguredLlmLauncher) -> Self {
        Self {
            kind: AdapterKind::ConfiguredLlm,
            name: launcher.command.clone(),
            should_hydrate: launcher.hydrate,
            hydration_delivery: launcher.hydration_delivery,
            extra_env: launcher.extra_env.clone(),
        }
    }
}

fn render_stdin_prelude(adapter_name: &str, packet: &RehydrationPacket) -> String {
    let packet_json =
        serde_json::to_string_pretty(packet).expect("rehydration packet should serialize");
    format!(
        "CONTYNU REHYDRATION CONTEXT\nadapter={}\nproject_id={}\nUse this as authoritative project state when starting work.\n{}\n\n",
        adapter_name,
        packet.project_id,
        packet_json
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use serde_json::json;

    use super::{AdapterSpec, HydrationContext};
    use crate::checkpoint::RehydrationPacket;
    use crate::config::ContynuConfig;
    use crate::ids::ProjectId;

    #[test]
    fn configured_launcher_can_disable_stdin_hydration() {
        let config = serde_json::from_value::<ContynuConfig>(json!({
            "llm_launchers": [
                {
                    "command": "futurellm",
                    "hydrate": true,
                    "hydration_delivery": "env_only"
                }
            ]
        }))
        .unwrap();
        let adapter = AdapterSpec::detect("futurellm", &config);
        let project_id = ProjectId::new();
        let packet = RehydrationPacket {
            schema_version: 1,
            project_id: project_id.clone(),
            target_model: None,
            mission: "Continue the project faithfully.".into(),
            stable_facts: Vec::new(),
            constraints: Vec::new(),
            decisions: Vec::new(),
            current_state: "No prior summary available.".into(),
            open_loops: Vec::new(),
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context: Vec::new(),
            retrieval_guidance: Vec::new(),
        };
        let hydration = HydrationContext {
            project_id,
            packet,
            packet_path: PathBuf::from("/tmp/rehydration.json"),
            prompt_path: PathBuf::from("/tmp/rehydration.txt"),
        };

        let plan = adapter
            .build_launch_plan(OsString::from("futurellm"), Vec::new(), Some(&hydration))
            .unwrap();

        assert!(plan.stdin_prelude.is_none());
        assert!(plan
            .env
            .iter()
            .any(|(key, value)| key == "CONTYNU_REHYDRATION_PACKET_FILE"
                && value == "/tmp/rehydration.json"));
    }
}
