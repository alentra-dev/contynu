import type { ModelSpec } from './types';

/**
 * Parse OpenClaw's "provider/model" format into separate fields.
 * Examples: "anthropic/claude-sonnet-4-20250514", "openai/gpt-5.4", "ollama/llama-4"
 */
export function parseModelSpec(primary: string): ModelSpec {
  const slash = primary.indexOf('/');
  if (slash === -1) return { provider: 'unknown', model: primary };
  return {
    provider: primary.substring(0, slash),
    model: primary.substring(slash + 1),
  };
}

/**
 * Map a provider to the optimal prompt format for Contynu rendering.
 * Returns the format name that contynu CLI understands.
 */
export function promptFormatForProvider(provider: string): string {
  switch (provider.toLowerCase()) {
    case 'anthropic':
      return 'xml';
    case 'openai':
      return 'markdown';
    case 'google':
    case 'google-genai':
      return 'structured_text';
    default:
      // Markdown is universally understood by all models including
      // Llama, Mistral, DeepSeek, Qwen, Ollama-hosted local models.
      return 'markdown';
  }
}
