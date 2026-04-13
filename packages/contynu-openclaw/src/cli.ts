import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { MemoryWrite, PromptWrite, ExportOptions } from './types';

const exec = promisify(execFile);

/**
 * JSON-RPC request/response types for MCP communication.
 */
interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number;
  method: string;
  params: Record<string, unknown>;
}

interface JsonRpcResponse {
  jsonrpc: string;
  id: number | null;
  result?: any;
  error?: { code: number; message: string };
}

/**
 * Wrapper around the contynu CLI binary.
 * Uses subprocess calls for CLI commands and MCP JSON-RPC for memory writes.
 */
export class ContynuCli {
  private stateDir: string;
  private binary: string;

  constructor(stateDir: string, binary = 'contynu') {
    this.stateDir = stateDir;
    this.binary = binary;
  }

  private baseArgs(): string[] {
    return ['--state-dir', this.stateDir];
  }

  /** Initialize the Contynu state directory if not already set up. */
  async ensureInit(): Promise<void> {
    try {
      await exec(this.binary, [...this.baseArgs(), 'init']);
    } catch {
      // May already be initialized — that's fine
    }
  }

  /** Create a new project and return its ID. */
  async startProject(): Promise<string> {
    const { stdout } = await exec(this.binary, [...this.baseArgs(), 'start-project']);
    const match = stdout.match(/prj_[a-f0-9]{32}/);
    if (!match) throw new Error(`Could not parse project ID from: ${stdout}`);
    return match[0];
  }

  /**
   * Write a memory via the MCP server.
   * Spawns a short-lived MCP server session to execute the write.
   */
  async writeMemory(projectId: string, memory: MemoryWrite, model?: string): Promise<void> {
    const args: Record<string, unknown> = {
      kind: memory.kind,
      scope: memory.scope,
      text: memory.text,
      importance: memory.importance,
    };
    if (memory.reason) args.reason = memory.reason;

    await this.mcpToolCall(projectId, 'write_memory', args, model);
  }

  /**
   * Record a user prompt via the MCP server.
   */
  async recordPrompt(projectId: string, prompt: PromptWrite, model?: string): Promise<void> {
    const args: Record<string, unknown> = {
      verbatim: prompt.verbatim,
    };
    if (prompt.interpretation) args.interpretation = prompt.interpretation;
    if (prompt.interpretationConfidence != null) {
      args.interpretation_confidence = prompt.interpretationConfidence;
    }

    await this.mcpToolCall(projectId, 'record_prompt', args, model);
  }

  /** Create a checkpoint for the given project. */
  async checkpoint(projectId: string, reason: string): Promise<void> {
    await exec(this.binary, [
      ...this.baseArgs(),
      'checkpoint',
      '--project',
      projectId,
      '--reason',
      reason,
    ]);
  }

  /** Export importance-ranked memories as Markdown with markers. */
  async exportMemory(projectId: string, opts: ExportOptions = {}): Promise<string> {
    const args = [
      ...this.baseArgs(),
      'export-memory',
      '--project',
      projectId,
    ];
    if (opts.withMarkers) args.push('--with-markers');
    if (opts.maxChars) args.push('--max-chars', String(opts.maxChars));

    const { stdout } = await exec(this.binary, args);
    return stdout;
  }

  /**
   * Execute an MCP tool call by spawning a short-lived MCP server.
   * Sends initialize → tools/call → exits.
   */
  private async mcpToolCall(
    projectId: string,
    toolName: string,
    toolArgs: Record<string, unknown>,
    model?: string,
  ): Promise<JsonRpcResponse> {
    const initRequest: JsonRpcRequest = {
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: '2025-03-26',
        capabilities: {},
        clientInfo: { name: 'contynu-openclaw', version: '0.1.0' },
      },
    };

    const toolRequest: JsonRpcRequest = {
      jsonrpc: '2.0',
      id: 2,
      method: 'tools/call',
      params: {
        name: toolName,
        arguments: toolArgs,
      },
    };

    const input = JSON.stringify(initRequest) + '\n' + JSON.stringify(toolRequest) + '\n';

    const env: Record<string, string> = {
      ...process.env as Record<string, string>,
      CONTYNU_STATE_DIR: this.stateDir,
      CONTYNU_ACTIVE_PROJECT: projectId,
    };

    const { stdout } = await exec(this.binary, ['mcp-server'], {
      env,
      maxBuffer: 10 * 1024 * 1024,
      timeout: 10_000,
    });

    // Parse the last JSON line (tool call response)
    const lines = stdout.trim().split('\n').filter(l => l.trim());
    if (lines.length < 2) {
      throw new Error(`MCP server returned unexpected output: ${stdout}`);
    }

    const response: JsonRpcResponse = JSON.parse(lines[lines.length - 1]);
    if (response.error) {
      throw new Error(`MCP tool ${toolName} failed: ${response.error.message}`);
    }

    return response;
  }
}
