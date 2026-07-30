use super::{
    Arc, AtomicU64, ContentVisitControl, ContentVisitEvent, EntryBuilder, Error, IndexEntry,
    IndexOptions, Mutex, Ordering, has_failure, set_failure,
};

pub(super) struct BuildWorker {
    entries: Arc<Mutex<Vec<IndexEntry>>>,
    failure: Arc<Mutex<Option<Error>>>,
    retained_bytes: Arc<AtomicU64>,
    retained_entries: Arc<AtomicU64>,
    limits: IndexOptions,
    file: Option<EntryBuilder>,
}

impl BuildWorker {
    pub(super) fn new(
        entries: Arc<Mutex<Vec<IndexEntry>>>,
        failure: Arc<Mutex<Option<Error>>>,
        retained_bytes: Arc<AtomicU64>,
        retained_entries: Arc<AtomicU64>,
        limits: IndexOptions,
    ) -> Self {
        Self {
            entries,
            failure,
            retained_bytes,
            retained_entries,
            limits,
            file: None,
        }
    }

    pub(super) fn visit(&mut self, event: &ContentVisitEvent<'_>) -> ContentVisitControl {
        if has_failure(&self.failure) {
            return ContentVisitControl::Quit;
        }
        match event {
            ContentVisitEvent::FileStart { file: opened, .. } => {
                match EntryBuilder::new(opened.root_index, opened.relative, opened.bytes) {
                    Ok(builder) => self.file = Some(builder),
                    Err(error) => {
                        set_failure(&self.failure, error);
                        return ContentVisitControl::Quit;
                    }
                }
            }
            ContentVisitEvent::Chunk { bytes, .. } => {
                if let Some(builder) = &mut self.file {
                    let chunk = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    let previous = self.retained_bytes.fetch_add(chunk, Ordering::AcqRel);
                    if previous.saturating_add(chunk) > self.limits.max_content_bytes {
                        set_failure(
                            &self.failure,
                            Error::index(
                                &builder.path,
                                format!("content exceeds {} bytes", self.limits.max_content_bytes),
                            ),
                        );
                        return ContentVisitControl::Quit;
                    }
                    if let Err(error) = builder.push(bytes) {
                        set_failure(&self.failure, error);
                        return ContentVisitControl::Quit;
                    }
                }
            }
            ContentVisitEvent::FileEnd {
                status, bytes_read, ..
            } => {
                if let Some(entry) = self
                    .file
                    .take()
                    .and_then(|builder| builder.finish(*status, *bytes_read))
                {
                    let count = self.retained_entries.fetch_add(1, Ordering::AcqRel) + 1;
                    if count > self.limits.max_entries {
                        set_failure(
                            &self.failure,
                            Error::index(
                                &entry.path,
                                format!("entry count exceeds {}", self.limits.max_entries),
                            ),
                        );
                        return ContentVisitControl::Quit;
                    }
                    self.entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(entry);
                }
            }
        }
        ContentVisitControl::Continue
    }
}
