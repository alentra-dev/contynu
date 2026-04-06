const { describe, it } = require('node:test');
const assert = require('node:assert');

// Since we test the JS output, import from dist
// For CI: run `npm run build` first
const { updateMemoryMd } = require('../dist/writeback');

describe('updateMemoryMd', () => {
  const newContent =
    '<!-- contynu-memory-sync:start -->\n## Project Memory\n- fact\n<!-- contynu-memory-sync:end -->';

  it('appends to empty file', () => {
    const result = updateMemoryMd('', newContent);
    assert.ok(result.includes('contynu-memory-sync:start'));
    assert.ok(result.includes('fact'));
  });

  it('appends to existing content', () => {
    const existing = '# My Notes\n\nSome user content here.';
    const result = updateMemoryMd(existing, newContent);
    assert.ok(result.includes('My Notes'));
    assert.ok(result.includes('contynu-memory-sync:start'));
  });

  it('replaces existing contynu section', () => {
    const existing =
      '# Notes\n\n<!-- contynu-memory-sync:start -->\nold stuff\n<!-- contynu-memory-sync:end -->\n\n# More notes';
    const result = updateMemoryMd(existing, newContent);
    assert.ok(!result.includes('old stuff'));
    assert.ok(result.includes('fact'));
    assert.ok(result.includes('More notes'));
  });

  it('skips write when file is too large', () => {
    const existing = 'x'.repeat(20_000);
    const result = updateMemoryMd(existing, newContent);
    assert.strictEqual(result, existing); // unchanged — already at limit
  });

  it('preserves dreaming markers', () => {
    const existing =
      '<!-- openclaw-memory-promotion:abc -->\n- User likes TypeScript\n\nSome other content';
    const result = updateMemoryMd(existing, newContent);
    assert.ok(result.includes('openclaw-memory-promotion:abc'));
    assert.ok(result.includes('contynu-memory-sync:start'));
  });
});
