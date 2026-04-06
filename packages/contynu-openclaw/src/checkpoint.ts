import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import type { ContynuCli } from './cli';
import type { AgentMapping } from './agent-mapping';
import { updateMemoryMd } from './writeback';

interface CompactContext {
  agentId: string;
  workspace: string;
}

/**
 * Handle the session:compact:before hook.
 * Creates a Contynu checkpoint and writes back importance-ranked
 * memories to MEMORY.md before OpenClaw's compaction fires.
 *
 * After this runs, OpenClaw can compact as aggressively as it wants —
 * Contynu has the full history and the agent gets the most important
 * facts injected via MEMORY.md on every future turn.
 */
export async function handleCompactBefore(
  ctx: CompactContext,
  cli: ContynuCli,
  mapping: AgentMapping
): Promise<void> {
  try {
    const projectId = await mapping.resolveProject(ctx.agentId, cli);

    // Checkpoint everything before compaction destroys it
    await cli.checkpoint(projectId, 'pre-compaction');

    // Export top memories and write back to MEMORY.md
    const exported = await cli.exportMemory(projectId, {
      withMarkers: true,
      maxChars: 18_000, // leave 2K buffer under OpenClaw's 20K limit
    });

    const memoryPath = path.join(ctx.workspace, 'MEMORY.md');
    const existing = await fs.readFile(memoryPath, 'utf-8').catch(() => '');
    const updated = updateMemoryMd(existing, exported);

    if (updated !== existing) {
      await fs.writeFile(memoryPath, updated, 'utf-8');
    }
  } catch (err) {
    console.error(
      `[contynu-openclaw] compact:before failed: ${err instanceof Error ? err.message : err}`
    );
  }
}
