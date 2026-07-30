use super::update_worker::UpdateWorker;
use super::{
    Arc, AtomicU64, BTreeMap, ChangedContentVisitOutcome, Error, IndexUpdateReport, Mutex, Path,
    PersistentIndex, Result, ScanOptions, Scanner, WatchEvent, WatchPlan, WatcherEventAdapter,
    take_failure, validate_unique_entries,
};

impl PersistentIndex {
    /// Applies a safe changed-file watcher plan without directory traversal.
    ///
    /// Plans that can affect selection rebuild the complete multi-root index.
    ///
    /// # Errors
    ///
    /// Returns invalid-root, scanner, allocation, or limit failures.
    pub fn update(
        &mut self,
        root_index: usize,
        plan: &WatchPlan,
        scan_options: ScanOptions,
    ) -> Result<IndexUpdateReport> {
        let root = self.roots.get(root_index).cloned().ok_or_else(|| {
            Error::index("<memory>", format!("root index {root_index} is invalid"))
        })?;
        if plan.full_rescan {
            return self.rebuild(scan_options);
        }
        let entries = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let retained_bytes = Arc::new(AtomicU64::new(0));
        let worker_entries = Arc::clone(&entries);
        let worker_failure = Arc::clone(&failure);
        let worker_bytes = Arc::clone(&retained_bytes);
        let limits = self.options.clone();
        let outcome = Scanner::new(root)
            .options(
                scan_options
                    .clone()
                    .with_content_parallelism(self.options.build_parallelism),
            )
            .visit_changed_content_streaming(plan, move |_| {
                let mut worker = UpdateWorker::new(
                    root_index,
                    Arc::clone(&worker_entries),
                    Arc::clone(&worker_failure),
                    Arc::clone(&worker_bytes),
                    limits.clone(),
                );
                move |event| worker.visit(&event)
            })?;
        if let Some(error) = take_failure(&failure) {
            return Err(error);
        }
        match outcome {
            ChangedContentVisitOutcome::FullRescanRequired => self.rebuild(scan_options),
            ChangedContentVisitOutcome::Visited(report) => {
                let mut changed_entries = std::mem::take(
                    &mut *entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                changed_entries.sort_unstable_by(|left, right| left.key().cmp(&right.key()));
                validate_unique_entries(&changed_entries, Path::new("<memory>"))?;
                self.apply_changed_entries(root_index, plan, changed_entries, report)
            }
        }
    }

    /// Converts raw watcher events for one root and applies their deterministic
    /// plan.
    ///
    /// # Errors
    ///
    /// Returns watcher-adapter or update failures.
    pub fn update_events<I>(
        &mut self,
        root_index: usize,
        events: I,
        scan_options: ScanOptions,
    ) -> Result<IndexUpdateReport>
    where
        I: IntoIterator<Item = WatchEvent>,
    {
        let root = self.event_roots.get(root_index).ok_or_else(|| {
            Error::index("<memory>", format!("root index {root_index} is invalid"))
        })?;
        let adapter = WatcherEventAdapter::with_options(root, &scan_options)?;
        let plan = adapter.plan(events);
        self.update(root_index, &plan, scan_options)
    }

    fn rebuild(&mut self, scan_options: ScanOptions) -> Result<IndexUpdateReport> {
        let storage_path = self
            .storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let scan_options = self.with_storage_exclusion(scan_options, storage_path.as_deref())?;
        let old = self
            .entries
            .iter()
            .map(|entry| ((entry.root_index, entry.path.clone()), entry.content_hash))
            .collect::<BTreeMap<_, _>>();
        let (new, report) =
            Self::build(self.event_roots.clone(), scan_options, self.options.clone())?;
        *new.storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = storage_path;
        let current = new
            .entries
            .iter()
            .map(|entry| ((entry.root_index, entry.path.clone()), entry.content_hash))
            .collect::<BTreeMap<_, _>>();
        let added = current.keys().filter(|key| !old.contains_key(*key)).count();
        let removed = old.keys().filter(|key| !current.contains_key(*key)).count();
        let updated = current
            .iter()
            .filter(|(key, hash)| old.get(*key).is_some_and(|old_hash| old_hash != *hash))
            .count();
        *self = new;
        Ok(IndexUpdateReport {
            added: u64::try_from(added).unwrap_or(u64::MAX),
            updated: u64::try_from(updated).unwrap_or(u64::MAX),
            removed: u64::try_from(removed).unwrap_or(u64::MAX),
            files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
            content_bytes: self.content_bytes,
            revision: self.revision.clone(),
            full_rebuild: true,
            changed_scan: report.scan.reports.into_iter().next(),
        })
    }
}
