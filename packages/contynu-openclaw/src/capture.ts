import type { ContynuCli } from './cli';
import type { AgentMapping } from './agent-mapping';
import type { IngestEvent } from './types';
import { parseModelSpec } from './model-detection';

/**
 * Message structure from OpenClaw's afterTurn context.
 * Simplified — real OpenClaw messages may have richer tool_use content.
 */
interface TurnMessage {
  role: 'user' | 'assistant' | 'tool';
  content?: string;
  tool_use?: { name: string; input: Record<string, unknown> };
  tool_result?: { status: string; output?: string };
}

interface TurnContext {
  agentId: string;
  agent: { config: { model: { primary: string } } };
  turn: { messages: TurnMessage[] };
  workspace: string;
}

/**
 * Handle the afterTurn lifecycle hook: capture conversation to Contynu.
 * Errors are caught and logged — never thrown to avoid crashing the gateway.
 */
export async function handleAfterTurn(
  ctx: TurnContext,
  cli: ContynuCli,
  mapping: AgentMapping
): Promise<void> {
  try {
    const projectId = await mapping.resolveProject(ctx.agentId, cli);
    const { model } = parseModelSpec(ctx.agent.config.model.primary);

    const events: IngestEvent[] = [];

    for (const msg of ctx.turn.messages) {
      if (msg.role === 'user' && msg.content) {
        events.push({
          event_type: 'message_input',
          actor: 'user',
          payload: { content: [{ type: 'text', text: msg.content }] },
        });
      } else if (msg.role === 'assistant' && msg.content) {
        events.push({
          event_type: 'message_output',
          actor: 'assistant',
          payload: { content: [{ type: 'text', text: msg.content }] },
        });
      } else if (msg.role === 'assistant' && msg.tool_use) {
        events.push({
          event_type: 'tool_call',
          actor: 'assistant',
          payload: {
            tool_name: msg.tool_use.name,
            arguments: msg.tool_use.input,
          },
        });
      } else if (msg.role === 'tool' && msg.tool_result) {
        events.push({
          event_type: 'tool_result',
          actor: 'tool',
          payload: {
            status: msg.tool_result.status,
            output: msg.tool_result.output ?? '',
          },
        });
      }
    }

    if (events.length === 0) return;

    await cli.ingest(projectId, events, {
      adapter: 'openclaw',
      model,
      deriveMemory: true,
    });
  } catch (err) {
    console.error(
      `[contynu-openclaw] afterTurn failed: ${err instanceof Error ? err.message : err}`
    );
  }
}
