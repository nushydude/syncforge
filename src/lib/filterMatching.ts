/**
 * Client-side glob matching for filter editor hints and Vitest parity checks.
 * The Rust scanner (`globset` in `scanner.rs`) is authoritative for real scans and preview.
 */
import type { Filters } from '../types';

/** Normalizes a glob pattern for path matching (non-recursive patterns match at any depth). */
export function normalizePattern(pattern: string): string {
  const trimmed = pattern.trim();
  if (trimmed.includes('/') || trimmed.includes('\\')) {
    return trimmed.replace(/\\/g, '/');
  }
  return `**/${trimmed}`;
}

function compileGlob(pattern: string): RegExp {
  let regex = '^';
  let i = 0;
  while (i < pattern.length) {
    const two = pattern.slice(i, i + 2);
    if (two === '**') {
      regex += '.*';
      i += 2;
      continue;
    }
    const ch = pattern[i];
    if (ch === '*') {
      regex += '[^/]*';
    } else if (ch === '?') {
      regex += '[^/]';
    } else if ('.+^${}()|[]\\'.includes(ch)) {
      regex += `\\${ch}`;
    } else {
      regex += ch;
    }
    i += 1;
  }
  regex += '$';
  return new RegExp(regex, 'i');
}

/** Converts a glob pattern to a RegExp (supports `*`, `?`, and `**`). */
export function globToRegExp(pattern: string): RegExp {
  return compileGlob(normalizePattern(pattern));
}

export function pathMatchesPattern(relativePath: string, pattern: string): boolean {
  const path = relativePath.replace(/\\/g, '/');
  const trimmed = pattern.trim();
  const hasPathSep = trimmed.includes('/') || trimmed.includes('\\');

  const candidates = hasPathSep ? [path] : [path, path.split('/').pop() ?? path];
  const sources = hasPathSep
    ? [trimmed.replace(/\\/g, '/')]
    : [normalizePattern(trimmed), trimmed];

  return sources.some((source) =>
    candidates.some((candidate) => compileGlob(source).test(candidate)),
  );
}

export function pathMatchesFilters(relativePath: string, filters: Filters): boolean {
  const path = relativePath.replace(/\\/g, '/');

  if (filters.include.length > 0) {
    const included = filters.include.some((pattern) =>
      pathMatchesPattern(path, pattern),
    );
    if (!included) {
      return false;
    }
  }

  if (filters.exclude.some((pattern) => pathMatchesPattern(path, pattern))) {
    return false;
  }

  return true;
}
