import * as path from 'node:path';
import { ContynuCli } from './cli';
import { AgentMapping } from './agent-mapping';
import { handleAfterTurn } from './capture';
import { handleCompactBefore } from './checkpoint';
import type { ContynuPluginConfig } from './types';

/**
 * contynu-openclaw — Permanent memory for OpenClaw agents.
 *
 * This plugin records user prompts, writes meaningful memories from
 * assistant output, checkpoints before compaction, and writes
 * importance-ranked facts back to MEMORY.md.
 *
 * Agents also get MCP tools (write_memory, update_memory, delete_memory,
 * record_prompt, search_memory, list_memories) for direct memory management.
 */
const plugin = {
  id: 'contynu-openclaw',
  name: 'Contynu Memory',
  description: 'Permanent, model-agnostic memory for OpenClaw agents powered by Contynu.',

  register(api: any, config?: ContynuPluginConfig) {
    const stateDir = config?.stateDir ?? path.join(process.cwd(), '.contynu');
    const cli = new ContynuCli(stateDir);
    const mapping = new AgentMapping(stateDir);

    // Ensure Contynu state is initialized on plugin load
    api.on('bootstrap', async () => {
      try {
        await cli.ensureInit();
        console.log('[contynu-openclaw] Initialized. State:', stateDir);
      } catch (err) {
        console.error(
          `[contynu-openclaw] bootstrap failed: ${err instanceof Error ? err.message : err}`
        );
      }
    });

    // Record prompts and write memories after every conversation turn
    api.on('afterTurn', (ctx: any) => handleAfterTurn(ctx, cli, mapping));

    // Checkpoint and write back to MEMORY.md before compaction fires
    api.on('session:compact:before', (ctx: any) => handleCompactBefore(ctx, cli, mapping));

    console.log('[contynu-openclaw] Plugin registered.');
  },
};

export default plugin;

// Named exports for testing and advanced usage
export { ContynuCli } from './cli';
export { AgentMapping } from './agent-mapping';
export { handleAfterTurn } from './capture';
export { handleCompactBefore } from './checkpoint';
export { updateMemoryMd } from './writeback';
export { parseModelSpec, promptFormatForProvider } from './model-detection';
export type {
  ContynuPluginConfig,
  MemoryWrite,
  PromptWrite,
  AgentProjectMap,
  ModelSpec,
} from './types';
