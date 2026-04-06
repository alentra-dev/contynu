/** Event format for contynu ingest (JSONL). */
export interface IngestEvent {
  event_type: string;
  actor: string;
  payload: Record<string, unknown>;
  ts?: string;
}

/** Options for the contynu CLI ingest command. */
export interface IngestOptions {
  adapter?: string;
  model?: string;
  deriveMemory?: boolean;
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
  deriveMemory?: boolean;
}
