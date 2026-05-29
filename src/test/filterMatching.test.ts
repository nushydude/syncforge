import { describe, expect, it } from 'vitest';
import {
  globToRegExp,
  normalizePattern,
  pathMatchesFilters,
  pathMatchesPattern,
} from '../lib/filterMatching';

describe('filterMatching', () => {
  it('normalizes bare patterns to any depth', () => {
    expect(normalizePattern('*.txt')).toBe('**/*.txt');
    expect(normalizePattern('sub/*.doc')).toBe('sub/*.doc');
  });

  it('matches paths with wildcards', () => {
    expect(pathMatchesPattern('notes.txt', '*.txt')).toBe(true);
    expect(pathMatchesPattern('deep/notes.txt', '*.txt')).toBe(true);
    expect(pathMatchesPattern('notes.tmp', '*.txt')).toBe(false);
  });

  it('applies include and exclude lists', () => {
    const filters = {
      include: ['*.txt'],
      exclude: ['*.tmp'],
    };
    expect(pathMatchesFilters('a.txt', filters)).toBe(true);
    expect(pathMatchesFilters('dir/a.txt', filters)).toBe(true);
    expect(pathMatchesFilters('a.tmp', filters)).toBe(false);
    expect(pathMatchesFilters('pic.png', filters)).toBe(false);
  });

  it('allows all paths when include is empty', () => {
    expect(
      pathMatchesFilters('anything.bin', { include: [], exclude: ['*.tmp'] }),
    ).toBe(true);
    expect(
      pathMatchesFilters('skip.tmp', { include: [], exclude: ['*.tmp'] }),
    ).toBe(false);
  });

  it('builds regex for double-star segments', () => {
    expect(globToRegExp('**/*.txt').test('a/b/c.txt')).toBe(true);
  });
});
