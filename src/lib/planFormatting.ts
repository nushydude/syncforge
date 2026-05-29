import type { SyncAction, SyncPlan } from '../types';

export interface FormattedAction {
  path: string;
  label: string;
  detail: string;
  tone: 'copy' | 'delete' | 'mkdir' | 'conflict' | 'skip' | 'neutral';
}

export function actionLabel(action: SyncAction): string {
  switch (action.kind) {
    case 'copyLeftToRight':
      return 'Copy → right';
    case 'copyRightToLeft':
      return 'Copy → left';
    case 'deleteLeft':
      return 'Delete on left';
    case 'deleteRight':
      return 'Delete on right';
    case 'createDirLeft':
      return 'Create folder (left)';
    case 'createDirRight':
      return 'Create folder (right)';
    case 'conflict':
      return 'Conflict';
    case 'skip':
      return 'Skip';
    default:
      return 'Unknown';
  }
}

export function actionTone(
  action: SyncAction,
): FormattedAction['tone'] {
  switch (action.kind) {
    case 'copyLeftToRight':
    case 'copyRightToLeft':
      return 'copy';
    case 'deleteLeft':
    case 'deleteRight':
      return 'delete';
    case 'createDirLeft':
    case 'createDirRight':
      return 'mkdir';
    case 'conflict':
      return 'conflict';
    case 'skip':
      return 'skip';
    default:
      return 'neutral';
  }
}

export function actionDetail(action: SyncAction): string {
  if (action.kind === 'skip') {
    return action.reason;
  }
  if (action.kind === 'conflict') {
    return `L: ${action.left.size} B @ ${action.left.modifiedSecs} · R: ${action.right.size} B @ ${action.right.modifiedSecs}`;
  }
  return '';
}

export function formatAction(action: SyncAction): FormattedAction {
  return {
    path: action.path,
    label: actionLabel(action),
    detail: actionDetail(action),
    tone: actionTone(action),
  };
}

export function formatPlanSummary(plan: SyncPlan): string {
  const counts = plan.actions.reduce<Record<string, number>>((acc, action) => {
    const key = action.kind;
    acc[key] = (acc[key] ?? 0) + 1;
    return acc;
  }, {});

  const parts: string[] = [
    `${plan.actions.length} action${plan.actions.length === 1 ? '' : 's'}`,
    `scanned L ${plan.scannedLeft} · R ${plan.scannedRight}`,
  ];

  if (counts.conflict) {
    parts.push(`${counts.conflict} conflict${counts.conflict === 1 ? '' : 's'}`);
  }

  if (plan.scanWarnings?.length) {
    parts.push(`${plan.scanWarnings.length} scan warning${plan.scanWarnings.length === 1 ? '' : 's'}`);
  }

  return parts.join(' · ');
}
