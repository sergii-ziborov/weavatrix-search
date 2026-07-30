use super::{
    Arc, AtomicBool, Error, Event, IndexUpdateReport, Instant, LiveIndex, LiveIndexOptions,
    LiveMessage, Ordering, Path, PathBuf, PersistentIndex, Receiver, RecursiveMode, Result,
    ScanOptions, Shared, TrySendError, WatchPlan, Watcher, WatcherEventAdapter, WatcherRuntime,
    mpsc,
};

pub(super) fn prepare_watcher(
    roots: &[PathBuf],
    scan_options: &ScanOptions,
    index_path: &Path,
    event_buffer: usize,
) -> Result<WatcherRuntime> {
    if event_buffer == 0 {
        return Err(Error::index(
            index_path,
            "live event buffer must be greater than zero",
        ));
    }
    let adapters = roots
        .iter()
        .map(|root| WatcherEventAdapter::with_options(root, scan_options))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (sender, receiver) = mpsc::sync_channel(event_buffer);
    let callback_sender = sender.clone();
    let overflowed = Arc::new(AtomicBool::new(false));
    let callback_overflowed = Arc::clone(&overflowed);
    let mut watcher = notify::recommended_watcher(move |event| {
        match callback_sender.try_send(LiveMessage::Event(event)) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                callback_overflowed.store(true, Ordering::Release);
            }
        }
    })
    .map_err(|error| Error::index(index_path, format!("watcher creation failed: {error}")))?;
    for root in roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| {
                Error::index(
                    index_path,
                    format!("failed to watch {}: {error}", root.display()),
                )
            })?;
    }
    Ok(WatcherRuntime {
        sender,
        receiver,
        watcher,
        overflowed,
        adapters,
    })
}

pub(super) fn validate_roots(
    index: &PersistentIndex,
    roots: &[PathBuf],
    index_path: &Path,
) -> Result<()> {
    if roots.is_empty() {
        return Err(Error::index(index_path, "at least one root is required"));
    }
    let resolved = roots
        .iter()
        .map(|root| {
            root.canonicalize()
                .map_err(|source| Error::io(root, source))
        })
        .collect::<Result<Vec<_>>>()?;
    if resolved != index.roots() {
        return Err(Error::index(
            index_path,
            "requested roots do not match the persistent index",
        ));
    }
    Ok(())
}

impl Drop for LiveIndex {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

pub(super) fn live_worker(
    receiver: &Receiver<LiveMessage>,
    shared: &Shared,
    index_path: &Path,
    scan_options: &ScanOptions,
    options: &LiveIndexOptions,
    adapters: &[WatcherEventAdapter],
) {
    while let Ok(message) = receiver.recv() {
        let LiveMessage::Event(first) = message else {
            break;
        };
        let started = Instant::now();
        let mut events = Vec::new();
        let mut force_rescan = shared.overflowed.swap(false, Ordering::AcqRel);
        let mut stop_after_batch = false;
        collect_event(first, &mut events, &mut force_rescan, shared);
        while started.elapsed() < options.debounce {
            let remaining = options.debounce.saturating_sub(started.elapsed());
            match receiver.recv_timeout(remaining) {
                Ok(LiveMessage::Event(event)) => {
                    collect_event(event, &mut events, &mut force_rescan, shared);
                }
                Ok(LiveMessage::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stop_after_batch = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
        force_rescan |= shared.overflowed.swap(false, Ordering::AcqRel);
        if let Err(error) = apply_batch(
            shared,
            index_path,
            scan_options,
            options,
            adapters,
            &events,
            force_rescan,
        ) {
            set_last_error(shared, error.to_string());
        }
        if stop_after_batch {
            break;
        }
    }
}

fn collect_event(
    event: notify::Result<Event>,
    events: &mut Vec<Event>,
    force_rescan: &mut bool,
    shared: &Shared,
) {
    match event {
        Ok(event) => events.push(event),
        Err(error) => {
            *force_rescan = true;
            set_last_error(
                shared,
                format!("watcher reported lost or invalid events: {error}"),
            );
        }
    }
}

fn apply_batch(
    shared: &Shared,
    index_path: &Path,
    scan_options: &ScanOptions,
    options: &LiveIndexOptions,
    adapters: &[WatcherEventAdapter],
    events: &[Event],
    force_rescan: bool,
) -> Result<()> {
    let events = events
        .iter()
        .filter(|event| {
            !event
                .paths
                .iter()
                .any(|path| is_index_artifact(path, index_path))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut index = shared
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut reports = Vec::new();
    if force_rescan {
        let plan = WatchPlan {
            full_rescan: true,
            ..WatchPlan::default()
        };
        reports.push(index.update(0, &plan, scan_options.clone())?);
    } else {
        for (root_index, adapter) in adapters.iter().enumerate() {
            let plan = adapter.plan_notify(events.iter().cloned());
            if plan.full_rescan {
                reports.push(index.update(root_index, &plan, scan_options.clone())?);
                break;
            }
            if plan.changed.is_empty() && plan.removed.is_empty() {
                continue;
            }
            reports.push(index.update(root_index, &plan, scan_options.clone())?);
        }
    }
    if reports.is_empty() {
        return Ok(());
    }
    for report in &reports {
        record_update(shared, report);
    }
    if options.persist_each_batch {
        index.save(index_path)?;
        shared.dirty.store(false, Ordering::Release);
    }
    Ok(())
}

fn is_index_artifact(path: &Path, index_path: &Path) -> bool {
    if paths_equal(path, index_path) {
        return true;
    }
    let (Some(parent), Some(index_parent), Some(name), Some(index_name)) = (
        path.parent(),
        index_path.parent(),
        path.file_name(),
        index_path.file_name(),
    ) else {
        return false;
    };
    if !paths_equal(parent, index_parent) {
        return false;
    }
    let name = name.to_string_lossy();
    let index_name = index_name.to_string_lossy();
    let Some(suffix) = name.strip_prefix(index_name.as_ref()) else {
        return false;
    };
    suffix == ".lock" || suffix.starts_with(".tmp.") || suffix.starts_with(".backup.")
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = std::path::absolute(left)
        .unwrap_or_else(|_| left.to_path_buf())
        .to_string_lossy()
        .replace(r"\\?\", "");
    let right = std::path::absolute(right)
        .unwrap_or_else(|_| right.to_path_buf())
        .to_string_lossy()
        .replace(r"\\?\", "");
    left.eq_ignore_ascii_case(&right)
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    std::path::absolute(left).unwrap_or_else(|_| left.to_path_buf())
        == std::path::absolute(right).unwrap_or_else(|_| right.to_path_buf())
}

pub(super) fn record_update(shared: &Shared, report: &IndexUpdateReport) {
    shared.dirty.store(true, Ordering::Release);
    shared.batches.fetch_add(1, Ordering::AcqRel);
    shared.changed_files.fetch_add(
        report
            .added
            .saturating_add(report.updated)
            .saturating_add(report.removed),
        Ordering::AcqRel,
    );
    if report.full_rebuild {
        shared.full_rebuilds.fetch_add(1, Ordering::AcqRel);
    }
    let mut generation = shared
        .generation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *generation = generation.saturating_add(1);
    shared.changed.notify_all();
}

pub(super) fn set_last_error(shared: &Shared, error: String) {
    *shared
        .last_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
}
