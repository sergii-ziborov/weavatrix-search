use super::build_worker::BuildWorker;
use super::{
    Arc, AtomicU64, Error, IndexBuildReport, IndexBuilder, IndexOptions, MAX_ROOTS, MultiScanner,
    Mutex, Path, PathBuf, PersistentIndex, Result, ScanOptions, content_bytes, revision,
    scan_options_with_storage_exclusion, take_failure, validate_unique_entries,
};

impl IndexBuilder {
    /// Creates a builder with one repository root and safe defaults.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            roots: vec![root.into()],
            scan_options: ScanOptions::default()
                .metadata_only()
                .selected_files_only()
                .with_content_discovery(weavatrix_scan::ContentDiscoveryMode::BufferedParallel)
                .with_content_validation(weavatrix_scan::ContentValidationPolicy::Fast),
            index_options: IndexOptions::default(),
        }
    }

    /// Adds another root while preserving stable root order.
    #[must_use]
    pub fn add_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }

    /// Adds several roots while preserving insertion order.
    #[must_use]
    pub fn extend_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.roots.extend(roots.into_iter().map(Into::into));
        self
    }

    /// Replaces repository discovery, ignore, and content-safety policy.
    #[must_use]
    pub fn scan_options(mut self, options: ScanOptions) -> Self {
        self.scan_options = options;
        self
    }

    /// Replaces index resource and parallelism policy.
    #[must_use]
    pub fn index_options(mut self, options: IndexOptions) -> Self {
        self.index_options = options;
        self
    }

    /// Builds an in-memory snapshot.
    ///
    /// # Errors
    ///
    /// Returns scanner, allocation, or resource-limit failures.
    pub fn build(self) -> Result<(PersistentIndex, IndexBuildReport)> {
        PersistentIndex::build(self.roots, self.scan_options, self.index_options)
    }

    /// Builds and atomically saves a snapshot.
    ///
    /// # Errors
    ///
    /// Returns build or persistence failures.
    pub fn build_and_save(
        self,
        path: impl AsRef<Path>,
    ) -> Result<(PersistentIndex, IndexBuildReport)> {
        PersistentIndex::build_and_save(path, self.roots, self.scan_options, self.index_options)
    }
}

impl PersistentIndex {
    /// Starts an ergonomic persistent-index builder.
    #[must_use]
    pub fn builder(root: impl Into<PathBuf>) -> IndexBuilder {
        IndexBuilder::new(root)
    }

    /// Builds a complete in-memory index through Weavatrix Scan.
    ///
    /// # Errors
    ///
    /// Returns scanner, allocation, limit, or invalid-policy failures.
    pub fn build<I, P>(
        roots: I,
        scan_options: ScanOptions,
        options: IndexOptions,
    ) -> Result<(Self, IndexBuildReport)>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        options.validate(Path::new("<memory>"))?;
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        if roots.is_empty() {
            return Err(Error::index("<memory>", "at least one root is required"));
        }
        if roots.len() > MAX_ROOTS {
            return Err(Error::index(
                "<memory>",
                format!("root count {} exceeds {MAX_ROOTS}", roots.len()),
            ));
        }
        let event_roots = roots
            .iter()
            .map(|root| std::path::absolute(root).map_err(|source| Error::io(root, source)))
            .collect::<Result<Vec<_>>>()?;
        let entries = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let retained_bytes = Arc::new(AtomicU64::new(0));
        let retained_entries = Arc::new(AtomicU64::new(0));
        let worker_entries = Arc::clone(&entries);
        let worker_failure = Arc::clone(&failure);
        let worker_bytes = Arc::clone(&retained_bytes);
        let worker_count = Arc::clone(&retained_entries);
        let limits = options.clone();
        let mut roots_iter = roots.iter().cloned();
        let first = roots_iter
            .next()
            .ok_or_else(|| Error::index("<memory>", "at least one root is required"))?;
        let scanner = roots_iter.fold(
            MultiScanner::new(first)
                .options(scan_options.with_content_parallelism(options.build_parallelism)),
            MultiScanner::add_root,
        );
        let scan = scanner.visit_content_streaming(move |_, _| {
            let mut worker = BuildWorker::new(
                Arc::clone(&worker_entries),
                Arc::clone(&worker_failure),
                Arc::clone(&worker_bytes),
                Arc::clone(&worker_count),
                limits.clone(),
            );
            move |event| worker.visit(&event)
        })?;
        if let Some(error) = take_failure(&failure) {
            return Err(error);
        }
        let mut entries = std::mem::take(
            &mut *entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        entries.sort_unstable_by(|left, right| left.key().cmp(&right.key()));
        validate_unique_entries(&entries, Path::new("<memory>"))?;
        let indexed_roots = scan
            .reports
            .iter()
            .map(|report| report.root.clone())
            .collect::<Vec<_>>();
        let content_bytes = content_bytes(&entries, Path::new("<memory>"))?;
        let revision = revision(&indexed_roots, &entries);
        let files = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let report = IndexBuildReport {
            roots: indexed_roots.clone(),
            files,
            content_bytes,
            revision: revision.clone(),
            scan,
        };
        Ok((
            Self {
                roots: indexed_roots,
                event_roots,
                entries,
                revision,
                content_bytes,
                options,
                storage_path: Mutex::new(None),
            },
            report,
        ))
    }

    /// Builds and atomically saves an index.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::build`] plus persistence failures.
    pub fn build_and_save<I, P>(
        path: impl AsRef<Path>,
        roots: I,
        scan_options: ScanOptions,
        options: IndexOptions,
    ) -> Result<(Self, IndexBuildReport)>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        let scan_options =
            scan_options_with_storage_exclusion(&roots, scan_options, path.as_ref())?;
        let (index, report) = Self::build(roots, scan_options, options)?;
        index.save(path)?;
        Ok((index, report))
    }
}
