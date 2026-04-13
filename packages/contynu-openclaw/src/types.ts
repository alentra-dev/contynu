/** Memory write request for the new model-driven architecture. */
export interface MemoryWrite {
  kind: 'fact' | 'constraint' | 'decision' | 'todo' | 'user_fact' | 'project_knowledge';
  scope: 'user' | 'project' | 'session';
  text: string;
  importance: number;
  reason?: string;
}

/** Prompt record for the new architecture. */
export interface PromptWrite {
  verbatim: string;
  interpretation?: string;
  interpretationConfidence?: number;
}

/** Options for the contynu CLI export-memory command. */
export interface ExportOptions {
  withMarkers?: boolean;
  maxChars?: number;
}

/** Agent-to-project mapping entry. */
export interface AgentProjectEntry {
  projectId: string;
  createdAt: string;
}

/** Agent mapping file format (.contynu/openclaw-agents.json). */
export interface AgentProjectMap {
  [agentId: string]: AgentProjectEntry;
}

/** Parsed model specification from OpenClaw's "provider/model" format. */
export interface ModelSpec {
  provider: string;
  model: string;
}

/** Plugin configuration options. */
export interface ContynuPluginConfig {
  stateDir?: string;
  maxMemoryChars?: number;
}
