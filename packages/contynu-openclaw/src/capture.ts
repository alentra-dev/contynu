import type { ContynuCli } from './cli';
import type { AgentMapping } from './agent-mapping';
import { parseModelSpec } from './model-detection';

/**
 * Message structure from OpenClaw's afterTurn context.
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
 * Handle the afterTurn lifecycle hook.
 *
 * Records user prompts and writes meaningful memories from assistant output.
 * The model's content is analyzed for fact-like statements — but since we're
 * in a plugin (not the model itself), we record the user prompt verbatim and
 * write assistant facts as project_knowledge.
 *
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

    // Record every user prompt verbatim
    for (const msg of ctx.turn.messages) {
      if (msg.role === 'user' && msg.content) {
        await cli.recordPrompt(projectId, {
          verbatim: msg.content,
        }, model);
      }
    }

    // Extract and write assistant content as project knowledge.
    // We write the substantive assistant content — the model in the next
    // session can refine these via update_memory/delete_memory.
    const assistantContent = ctx.turn.messages
      .filter(m => m.role === 'assistant' && m.content)
      .map(m => m.content!)
      .join('\n');

    if (assistantContent.length > 50) {
      // Write a concise summary as project knowledge
      // Truncate to a reasonable size — the model can refine later
      const text = assistantContent.length > 2000
        ? assistantContent.substring(0, 2000)
        : assistantContent;

      await cli.writeMemory(projectId, {
        kind: 'project_knowledge',
        scope: 'project',
        text,
        importance: 0.6,
        reason: `Captured from ${model} turn via OpenClaw plugin`,
      }, model);
    }
  } catch (err) {
    console.error(
      `[contynu-openclaw] afterTurn failed: ${err instanceof Error ? err.message : err}`
    );
  }
}
