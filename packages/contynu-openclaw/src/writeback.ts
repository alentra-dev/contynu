const START_MARKER = '<!-- contynu-memory-sync:start -->';
const END_MARKER = '<!-- contynu-memory-sync:end -->';
const MAX_TOTAL = 20_000; // OpenClaw's bootstrapMaxChars default

/**
 * Update MEMORY.md content with Contynu's exported memory section.
 * Uses HTML comment markers to find and replace the Contynu section,
 * preserving all user-written and dreaming-promoted content.
 *
 * If the Contynu section already exists, it is replaced in-place.
 * If not, the new section is appended — unless the total would exceed
 * OpenClaw's 20,000 character truncation limit, in which case the
 * write is skipped entirely.
 */
export function updateMemoryMd(existing: string, newContent: string): string {
  const startIdx = existing.indexOf(START_MARKER);
  const endIdx = existing.indexOf(END_MARKER);

  if (startIdx !== -1 && endIdx !== -1) {
    // Replace existing Contynu section, preserve everything else
    const before = existing.substring(0, startIdx);
    const after = existing.substring(endIdx + END_MARKER.length);
    return before + newContent + after;
  }

  // First write — append, but check total size
  const combined = existing.trimEnd() + '\n\n' + newContent;
  if (combined.length > MAX_TOTAL) {
    // Not enough room — return original unchanged
    console.warn(
      `[contynu-openclaw] MEMORY.md too large (${existing.length} chars), skipping write-back`
    );
    return existing;
  }

  return combined;
}
