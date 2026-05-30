import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PreviewTable } from '../components/preview/PreviewTable';
import type { SyncPlan } from '../types';

const samplePlan: SyncPlan = {
  pairId: 'p1',
  scannedLeft: 2,
  scannedRight: 2,
  actions: [
    { kind: 'copyLeftToRight', path: 'new.txt' },
    {
      kind: 'conflict',
      path: 'both.txt',
      left: {
        relativePath: 'both.txt',
        size: 1,
        modifiedSecs: 1,
        isDir: false,
      },
      right: {
        relativePath: 'both.txt',
        size: 2,
        modifiedSecs: 2,
        isDir: false,
      },
    },
  ],
};

describe('PreviewTable', () => {
  it('shows hint when no plan', () => {
    render(<PreviewTable plan={null} />);
    expect(screen.getByText(/run preview/i)).toBeInTheDocument();
  });

  it('shows loading state', () => {
    render(<PreviewTable plan={null} loading />);
    expect(screen.getByText(/scanning folders/i)).toBeInTheDocument();
  });

  it('shows error state', () => {
    render(<PreviewTable plan={null} error="scan failed" />);
    expect(screen.getByRole('alert')).toHaveTextContent('scan failed');
  });

  it('renders action rows for a plan', () => {
    render(<PreviewTable plan={samplePlan} />);
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getByText('new.txt')).toBeInTheDocument();
    expect(screen.getByText('Conflict')).toBeInTheDocument();
    expect(screen.getByText(/2 actions/i)).toBeInTheDocument();
  });

  it('renders scan warnings when present', () => {
    const plan: SyncPlan = {
      ...samplePlan,
      scanWarnings: ['left: skipped 1 path(s) (permission denied or unreadable)'],
    };
    render(<PreviewTable plan={plan} />);
    expect(screen.getByText(/skipped 1 path/i)).toBeInTheDocument();
  });

  it('virtualizes a large plan without rendering every row in the DOM', () => {
    const largePlan: SyncPlan = {
      pairId: 'p1',
      scannedLeft: 5000,
      scannedRight: 5000,
      actions: Array.from({ length: 5000 }, (_, i) => ({
        kind: 'copyLeftToRight' as const,
        path: `file-${i}.txt`,
      })),
    };
    const { container } = render(<PreviewTable plan={largePlan} />);
    const dataRows = container.querySelectorAll('tbody tr[data-index]');
    expect(dataRows.length).toBeLessThan(5000);
    expect(dataRows.length).toBeGreaterThan(0);
    expect(screen.getByText('file-0.txt')).toBeInTheDocument();
  });
});
