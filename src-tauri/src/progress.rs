use std::time::{Duration, Instant};

use crate::engine::{ProgressEvent, ProgressUpdate, SyncProgress};

/// Maximum rate for non-terminal progress events crossing the Tauri bridge.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Coalesces high-frequency progress updates while preserving phase and terminal events.
pub struct ProgressCoalescer<F, C = fn() -> Instant> {
    emit: F,
    clock: C,
    last_emit: Option<Instant>,
    last_phase: Option<String>,
    pending: Option<PendingProgress>,
    terminal: bool,
}

struct PendingProgress {
    run_id: String,
    pair_id: String,
    phase: String,
    current: u32,
    total: u32,
    path: Option<String>,
    message: Option<String>,
}

impl<F> ProgressCoalescer<F>
where
    F: FnMut(SyncProgress),
{
    pub fn system(emit: F) -> Self {
        Self::with_clock(emit, Instant::now)
    }
}

impl<F, C> ProgressCoalescer<F, C>
where
    F: FnMut(SyncProgress),
    C: FnMut() -> Instant,
{
    pub fn with_clock(emit: F, clock: C) -> Self {
        Self { emit, clock, last_emit: None, last_phase: None, pending: None, terminal: false }
    }

    #[cfg(test)]
    pub fn push(&mut self, progress: SyncProgress) {
        self.push_owned(progress);
    }

    pub fn push_event(&mut self, event: ProgressEvent<'_>) {
        match event {
            ProgressEvent::Owned(progress) => self.push_owned(progress),
            ProgressEvent::Update(update) => self.push_update(update),
        }
    }

    fn push_update(&mut self, update: ProgressUpdate<'_>) {
        if self.terminal {
            return;
        }
        let phase_changed = self.last_phase.as_deref() != Some(update.phase);
        let now = (self.clock)();
        let due = self.last_emit.is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
        if phase_changed || due {
            self.pending = None;
            let phase = update.phase.to_owned();
            (self.emit)(materialize(update));
            self.last_emit = Some(now);
            self.last_phase = Some(phase);
        } else {
            self.retain_pending(update);
        }
    }

    fn push_owned(&mut self, progress: SyncProgress) {
        if self.terminal {
            return;
        }
        let now = (self.clock)();
        let phase_changed = self.last_phase.as_deref() != Some(progress.phase.as_str());
        let terminal = progress.report.is_some()
            || matches!(progress.phase.as_str(), "completed" | "failed" | "cancelled");
        let due = self.last_emit.is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
        if phase_changed || terminal || due {
            if terminal {
                self.emit_pending();
            } else {
                self.pending = None;
            }
            (self.emit)(progress.clone());
            self.last_emit = Some(now);
            self.last_phase = Some(progress.phase);
            if terminal {
                self.terminal = true;
            }
        } else {
            self.pending = Some(PendingProgress::from_owned(progress));
        }
    }

    /// Flushes the latest throttled update when a caller ends without a terminal event.
    pub fn flush(&mut self) {
        if self.terminal {
            self.pending = None;
            return;
        }
        self.emit_pending();
    }

    fn retain_pending(&mut self, update: ProgressUpdate<'_>) {
        if let Some(pending) = self.pending.as_mut() {
            pending.current = update.current;
            pending.total = update.total;
            replace_string(&mut pending.phase, update.phase);
            replace_optional_string(&mut pending.path, update.path);
            replace_optional_string(&mut pending.message, update.message);
        } else {
            self.pending = Some(PendingProgress::from_update(update));
        }
    }

    fn emit_pending(&mut self) {
        if let Some(progress) = self.pending.take() {
            let now = (self.clock)();
            (self.emit)(progress.into_owned());
            self.last_emit = Some(now);
            self.last_phase = None;
        }
    }
}

impl PendingProgress {
    fn from_update(update: ProgressUpdate<'_>) -> Self {
        Self {
            run_id: update.run_id.to_owned(),
            pair_id: update.pair_id.to_owned(),
            phase: update.phase.to_owned(),
            current: update.current,
            total: update.total,
            path: update.path.map(str::to_owned),
            message: update.message.map(str::to_owned),
        }
    }

    fn from_owned(progress: SyncProgress) -> Self {
        Self {
            run_id: progress.run_id,
            pair_id: progress.pair_id,
            phase: progress.phase,
            current: progress.current,
            total: progress.total,
            path: progress.path,
            message: progress.message,
        }
    }

    fn into_owned(self) -> SyncProgress {
        SyncProgress {
            run_id: self.run_id,
            pair_id: self.pair_id,
            phase: self.phase,
            current: self.current,
            total: self.total,
            path: self.path,
            message: self.message,
            report: None,
        }
    }
}

fn replace_string(target: &mut String, value: &str) {
    target.clear();
    target.push_str(value);
}

fn replace_optional_string(target: &mut Option<String>, value: Option<&str>) {
    match (target.as_mut(), value) {
        (Some(target), Some(value)) => replace_string(target, value),
        (Some(_), None) => *target = None,
        (None, Some(value)) => *target = Some(value.to_owned()),
        (None, None) => {}
    }
}

fn materialize(update: ProgressUpdate<'_>) -> SyncProgress {
    SyncProgress {
        run_id: update.run_id.to_owned(),
        pair_id: update.pair_id.to_owned(),
        phase: update.phase.to_owned(),
        current: update.current,
        total: update.total,
        path: update.path.map(str::to_owned),
        message: update.message.map(str::to_owned),
        report: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn progress(phase: &str, current: u32, report: bool) -> SyncProgress {
        SyncProgress {
            run_id: "run".into(),
            pair_id: "pair".into(),
            phase: phase.into(),
            current,
            total: 10_000,
            path: Some(format!("file-{current}")),
            message: None,
            report: report.then_some(crate::models::RunReport {
                run_id: "run".into(),
                pair_id: "pair".into(),
                started_at: 0,
                finished_at: Some(1),
                status: crate::models::RunStatus::Completed,
                files_copied: 0,
                files_deleted: 0,
                bytes_transferred: 0,
                errors: vec![],
            }),
        }
    }

    #[test]
    fn fake_clock_bounds_instantaneous_events_and_keeps_latest_terminal_report() {
        let now = Rc::new(Cell::new(Instant::now()));
        let emitted = Rc::new(Cell::new(0usize));
        let last_current = Rc::new(Cell::new(0u32));
        let emitted_for_sink = Rc::clone(&emitted);
        let last_for_sink = Rc::clone(&last_current);
        let now_for_clock = Rc::clone(&now);
        let mut sink = ProgressCoalescer::with_clock(
            move |event| {
                emitted_for_sink.set(emitted_for_sink.get() + 1);
                last_for_sink.set(event.current);
            },
            move || now_for_clock.get(),
        );
        sink.push(progress("running", 0, false));
        for current in 1..10_000 {
            sink.push(progress("running", current, false));
        }
        assert_eq!(emitted.get(), 1);
        now.set(now.get() + PROGRESS_INTERVAL);
        sink.push(progress("running", 5_000, false));
        assert_eq!(emitted.get(), 2);
        sink.push(progress("completed", 10_000, true));
        sink.push(progress("running", 10_000, false));
        assert_eq!(emitted.get(), 3);
        assert_eq!(last_current.get(), 10_000);
    }

    #[test]
    fn borrowed_updates_are_coalesced_and_suppressed_after_terminal() {
        let mut emitted = 0usize;
        let mut sink = ProgressCoalescer::with_clock(|_| emitted += 1, || Instant::now());
        sink.push_event(ProgressEvent::Update(ProgressUpdate {
            run_id: "run",
            pair_id: "pair",
            phase: "running",
            current: 0,
            total: 2,
            path: Some("a"),
            message: None,
        }));
        sink.push_event(ProgressEvent::Update(ProgressUpdate {
            run_id: "run",
            pair_id: "pair",
            phase: "running",
            current: 1,
            total: 2,
            path: Some("b"),
            message: None,
        }));
        sink.push(progress("completed", 2, true));
        sink.push_event(ProgressEvent::Update(ProgressUpdate {
            run_id: "run",
            pair_id: "pair",
            phase: "running",
            current: 2,
            total: 2,
            path: Some("c"),
            message: None,
        }));
        assert_eq!(emitted, 3);
    }
}
