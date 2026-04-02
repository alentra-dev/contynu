use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::checkpoint::RehydrationPacket;
use crate::error::Result;
use crate::ids::ProjectId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Terminal,
    CodexCli,
    ClaudeCli,
    GeminiCli,
}

pub trait Adapter {
    fn kind(&self) -> AdapterKind;
    fn name(&self) -> &'static str;
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

impl AdapterKind {
    pub fn detect(program: &str) -> Self {
        match program {
            "codex" | "codex-cli" => Self::CodexCli,
            "claude" | "claude-code" => Self::ClaudeCli,
            "gemini" | "gemini-cli" => Self::GeminiCli,
            _ => Self::Terminal,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::CodexCli => "codex_cli",
            Self::ClaudeCli => "claude_cli",
            Self::GeminiCli => "gemini_cli",
        }
    }

    pub fn should_hydrate(self) -> bool {
        !matches!(self, Self::Terminal)
    }

    pub fn build_launch_plan(
        self,
        executable: OsString,
        args: Vec<OsString>,
        hydration: Option<&HydrationContext>,
    ) -> Result<LaunchPlan> {
        let mut env = Vec::new();
        let mut stdin_prelude = None;

        if let Some(hydration) = hydration {
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
            stdin_prelude = Some(render_stdin_prelude(self, &hydration.packet).into_bytes());
        }

        Ok(LaunchPlan {
            executable,
            args,
            env,
            stdin_prelude,
        })
    }
}

fn render_stdin_prelude(adapter: AdapterKind, packet: &RehydrationPacket) -> String {
    let packet_json =
        serde_json::to_string_pretty(packet).expect("rehydration packet should serialize");
    format!(
        "CONTYNU REHYDRATION CONTEXT\nadapter={}\nproject_id={}\nUse this as authoritative project state when starting work.\n{}\n\n",
        adapter.as_str(),
        packet.project_id,
        packet_json
    )
}
