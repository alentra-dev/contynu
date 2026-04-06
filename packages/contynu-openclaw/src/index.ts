import * as path from 'node:path';
import { ContynuCli } from './cli';
import { AgentMapping } from './agent-mapping';
import { handleAfterTurn } from './capture';
import { handleCompactBefore } from './checkpoint';
import type { ContynuPluginConfig } from './types';

/**
 * contynu-openclaw — Permanent memory for OpenClaw agents.
 *
 * This plugin captures every conversation turn, checkpoints before
 * compaction, and writes importance-ranked facts back to MEMORY.md.
 * Agents also get MCP tools (search_memory, list_memories, search_events)
 * for on-demand deep recall of any fact from the full project history.
 *
 * Usage in OpenClaw config (~/.openclaw/openclaw.json):
 * {
 *   "plugins": {
 *     "contynu-openclaw": { "enabled": true }
 *   }
 * }
 */
export default function register(api: any, config?: ContynuPluginConfig) {
  const stateDir = config?.stateDir ?? path.join(process.cwd(), '.contynu');
  const cli = new ContynuCli(stateDir);
  const mapping = new AgentMapping(stateDir);

  // Ensure Contynu state is initialized on plugin load
  api.on('bootstrap', async () => {
    try {
      await cli.ensureInit();
    } catch (err) {
      console.error(
        `[contynu-openclaw] bootstrap failed: ${err instanceof Error ? err.message : err}`
      );
    }
  });

  // Capture every conversation turn into Contynu's permanent store
  api.on('afterTurn', (ctx: any) => handleAfterTurn(ctx, cli, mapping));

  // Checkpoint and write back to MEMORY.md before compaction fires
  api.on('session:compact:before', (ctx: any) => handleCompactBefore(ctx, cli, mapping));
}

// Named exports for testing and advanced usage
export { ContynuCli } from './cli';
export { AgentMapping } from './agent-mapping';
export { handleAfterTurn } from './capture';
export { handleCompactBefore } from './checkpoint';
export { updateMemoryMd } from './writeback';
export { parseModelSpec, promptFormatForProvider } from './model-detection';
export type {
  ContynuPluginConfig,
  IngestEvent,
  AgentProjectMap,
  ModelSpec,
} from './types';
