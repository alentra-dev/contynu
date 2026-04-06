import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import type { AgentProjectMap } from './types';
import type { ContynuCli } from './cli';

/**
 * Manages the agent-id → contynu-project-id mapping.
 * Stored in .contynu/openclaw-agents.json.
 * Each OpenClaw agent gets its own isolated Contynu project.
 */
export class AgentMapping {
  private mapPath: string;
  private cache: AgentProjectMap | null = null;

  constructor(stateDir: string) {
    this.mapPath = path.join(stateDir, 'openclaw-agents.json');
  }

  /** Get or create the Contynu project for an agent. */
  async resolveProject(agentId: string, cli: ContynuCli): Promise<string> {
    const map = await this.load();
    if (map[agentId]) return map[agentId].projectId;

    const projectId = await cli.startProject();
    map[agentId] = { projectId, createdAt: new Date().toISOString() };
    await this.save(map);
    return projectId;
  }

  private async load(): Promise<AgentProjectMap> {
    if (this.cache) return this.cache;
    try {
      const content = await fs.readFile(this.mapPath, 'utf-8');
      this.cache = JSON.parse(content);
    } catch {
      this.cache = {};
    }
    return this.cache!;
  }

  private async save(map: AgentProjectMap): Promise<void> {
    this.cache = map;
    const dir = path.dirname(this.mapPath);
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(this.mapPath, JSON.stringify(map, null, 2), 'utf-8');
  }
}
