use serde::{Deserialize, Serialize};

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
}
