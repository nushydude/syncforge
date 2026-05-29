export function parseFilterLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export function addFilterPattern(
  patterns: readonly string[],
  pattern: string,
): string[] {
  const trimmed = pattern.trim();
  if (!trimmed) {
    return [...patterns];
  }
  if (patterns.some((p) => p === trimmed)) {
    return [...patterns];
  }
  return [...patterns, trimmed];
}

export function removeFilterPattern(
  patterns: readonly string[],
  index: number,
): string[] {
  if (index < 0 || index >= patterns.length) {
    return [...patterns];
  }
  return patterns.filter((_, i) => i !== index);
}

export function filtersToText(patterns: readonly string[]): string {
  return patterns.join("\n");
}
