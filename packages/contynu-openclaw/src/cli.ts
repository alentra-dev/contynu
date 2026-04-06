import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { IngestEvent, IngestOptions, ExportOptions } from './types';

const exec = promisify(execFile);

const MAX_PAYLOAD_BYTES = 64 * 1024; // 64KB per event payload

/**
 * Wrapper around the contynu CLI binary.
 * All operations are subprocess calls — no Rust FFI.
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

  /** Ingest events into the journal for a given project. */
  async ingest(projectId: string, events: IngestEvent[], opts: IngestOptions = {}): Promise<void> {
    const args = [...this.baseArgs(), 'ingest', '--project', projectId];
    if (opts.adapter) args.push('--adapter', opts.adapter);
    if (opts.model) args.push('--model', opts.model);
    if (opts.deriveMemory !== false) args.push('--derive-memory');

    // Truncate large payloads
    const lines = events.map((e) => {
      const json = JSON.stringify(e);
      if (json.length > MAX_PAYLOAD_BYTES) {
        const truncated = { ...e, payload: { text: '[truncated — payload exceeded 64KB]' } };
        return JSON.stringify(truncated);
      }
      return json;
    });

    const input = lines.join('\n') + '\n';
    const child = require('node:child_process').execFile(
      this.binary,
      args,
      { maxBuffer: 10 * 1024 * 1024 },
      () => {} // errors handled below
    );
    child.stdin?.write(input);
    child.stdin?.end();
    await new Promise<void>((resolve, reject) => {
      child.on('close', (code: number) => {
        if (code === 0) resolve();
        else reject(new Error(`contynu ingest exited with code ${code}`));
      });
    });
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
}
