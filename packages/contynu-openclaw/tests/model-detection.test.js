const { describe, it } = require('node:test');
const assert = require('node:assert');

const { parseModelSpec, promptFormatForProvider } = require('../dist/model-detection');

describe('parseModelSpec', () => {
  it('parses anthropic model', () => {
    const spec = parseModelSpec('anthropic/claude-sonnet-4-20250514');
    assert.strictEqual(spec.provider, 'anthropic');
    assert.strictEqual(spec.model, 'claude-sonnet-4-20250514');
  });

  it('parses openai model', () => {
    const spec = parseModelSpec('openai/gpt-5.4');
    assert.strictEqual(spec.provider, 'openai');
    assert.strictEqual(spec.model, 'gpt-5.4');
  });

  it('parses ollama model', () => {
    const spec = parseModelSpec('ollama/llama-4-scout');
    assert.strictEqual(spec.provider, 'ollama');
    assert.strictEqual(spec.model, 'llama-4-scout');
  });

  it('handles model with no provider', () => {
    const spec = parseModelSpec('some-local-model');
    assert.strictEqual(spec.provider, 'unknown');
    assert.strictEqual(spec.model, 'some-local-model');
  });

  it('handles openrouter multi-slash', () => {
    const spec = parseModelSpec('openrouter/moonshotai/kimi-k2');
    assert.strictEqual(spec.provider, 'openrouter');
    assert.strictEqual(spec.model, 'moonshotai/kimi-k2');
  });
});

describe('promptFormatForProvider', () => {
  it('returns xml for anthropic', () => {
    assert.strictEqual(promptFormatForProvider('anthropic'), 'xml');
  });

  it('returns markdown for openai', () => {
    assert.strictEqual(promptFormatForProvider('openai'), 'markdown');
  });

  it('returns structured_text for google', () => {
    assert.strictEqual(promptFormatForProvider('google'), 'structured_text');
  });

  it('returns markdown for unknown providers', () => {
    assert.strictEqual(promptFormatForProvider('ollama'), 'markdown');
    assert.strictEqual(promptFormatForProvider('mistral'), 'markdown');
    assert.strictEqual(promptFormatForProvider('deepseek'), 'markdown');
    assert.strictEqual(promptFormatForProvider('unknown'), 'markdown');
  });
});
