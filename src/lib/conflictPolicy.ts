import type {
  ConflictChoice,
  ConflictPolicy,
  FileEntry,
  SyncAction,
  SyncPlan,
} from '../types';

export type PathChangeKind =
  | 'inSync'
  | 'newOnLeft'
  | 'newOnRight'
  | 'deletedOnLeft'
  | 'deletedOnRight'
  | 'leftChanged'
  | 'rightChanged'
  | 'bothChanged';

export type ConflictAction = Extract<SyncAction, { kind: 'conflict' }>;

export const CONFLICT_POLICY_OPTIONS: {
  value: ConflictPolicy;
  label: string;
  hint: string;
}[] = [
  {
    value: 'newerWins',
    label: 'Newer wins',
    hint: 'Copy the file with the latest modification time.',
  },
  {
    value: 'left',
    label: 'Prefer left',
    hint: 'Always copy from the left folder.',
  },
  {
    value: 'right',
    label: 'Prefer right',
    hint: 'Always copy from the right folder.',
  },
  {
    value: 'keepBoth',
    label: 'Keep both',
    hint: 'Skip conflicting paths and leave both copies in place.',
  },
  {
    value: 'ask',
    label: 'Ask me',
    hint: 'Prompt before each conflict when you run a sync.',
  },
];

export function entriesEqual(a: FileEntry, b: FileEntry): boolean {
  return (
    a.isDir === b.isDir &&
    a.size === b.size &&
    a.modifiedSecs === b.modifiedSecs
  );
}

/**
 * Classify how a path changed relative to the last-sync snapshot.
 * Distinguishes new files, deletes, one-sided updates, and true conflicts.
 */
export function classifyPathChange(
  left: FileEntry | undefined,
  right: FileEntry | undefined,
  snapshot: FileEntry | undefined,
): PathChangeKind {
  if (left && right) {
    if (entriesEqual(left, right)) {
      return 'inSync';
    }
    if (!snapshot) {
      return 'bothChanged';
    }
    const leftChanged = !entriesEqual(left, snapshot);
    const rightChanged = !entriesEqual(right, snapshot);
    if (leftChanged && rightChanged) {
      return 'bothChanged';
    }
    if (leftChanged) {
      return 'leftChanged';
    }
    if (rightChanged) {
      return 'rightChanged';
    }
    return 'inSync';
  }

  if (left && !right) {
    if (!snapshot) {
      return 'newOnLeft';
    }
    if (entriesEqual(left, snapshot)) {
      return 'deletedOnRight';
    }
    return 'bothChanged';
  }

  if (!left && right) {
    if (!snapshot) {
      return 'newOnRight';
    }
    if (entriesEqual(right, snapshot)) {
      return 'deletedOnLeft';
    }
    return 'bothChanged';
  }

  return 'inSync';
}

export function pathChangeKindLabel(kind: PathChangeKind): string {
  switch (kind) {
    case 'newOnLeft':
      return 'New on left';
    case 'newOnRight':
      return 'New on right';
    case 'deletedOnLeft':
      return 'Deleted on left';
    case 'deletedOnRight':
      return 'Deleted on right';
    case 'leftChanged':
      return 'Updated on left';
    case 'rightChanged':
      return 'Updated on right';
    case 'bothChanged':
      return 'Conflict';
    default:
      return 'In sync';
  }
}

export function resolveConflictAction(
  policy: ConflictPolicy,
  path: string,
  left: FileEntry,
  right: FileEntry,
): SyncAction {
  switch (policy) {
    case 'ask':
      return { kind: 'conflict', path, left, right };
    case 'left':
      return { kind: 'copyLeftToRight', path };
    case 'right':
      return { kind: 'copyRightToLeft', path };
    case 'keepBoth':
      return {
        kind: 'skip',
        path,
        reason: 'keep both (conflict policy)',
      };
    case 'newerWins':
    default: {
      if (
        left.modifiedSecs > right.modifiedSecs ||
        (left.modifiedSecs === right.modifiedSecs && left.size !== right.size)
      ) {
        return { kind: 'copyLeftToRight', path };
      }
      if (right.modifiedSecs > left.modifiedSecs) {
        return { kind: 'copyRightToLeft', path };
      }
      return { kind: 'copyLeftToRight', path };
    }
  }
}

export function applyConflictChoice(
  action: ConflictAction,
  choice: ConflictChoice,
): SyncAction {
  const { path, left, right } = action;
  switch (choice) {
    case 'left':
      return { kind: 'copyLeftToRight', path };
    case 'right':
      return { kind: 'copyRightToLeft', path };
    case 'keepBoth':
      return { kind: 'skip', path, reason: 'keep both (user choice)' };
    case 'skip':
      return { kind: 'skip', path, reason: 'skipped by user' };
    default:
      return resolveConflictAction('newerWins', path, left, right);
  }
}

export function listConflictActions(plan: SyncPlan): ConflictAction[] {
  return plan.actions.filter(
    (action): action is ConflictAction => action.kind === 'conflict',
  );
}

export function applyResolutionsToPlan(
  plan: SyncPlan,
  resolutions: Record<string, ConflictChoice>,
): SyncPlan {
  return {
    ...plan,
    actions: plan.actions.map((action) => {
      if (action.kind !== 'conflict') {
        return action;
      }
      const choice = resolutions[action.path];
      if (!choice) {
        return action;
      }
      return applyConflictChoice(action, choice);
    }),
  };
}

export function allConflictsResolved(
  conflicts: ConflictAction[],
  resolutions: Record<string, ConflictChoice>,
): boolean {
  return conflicts.every((c) => resolutions[c.path] !== undefined);
}
