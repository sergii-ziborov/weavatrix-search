use super::{
    BTreeMap, BTreeSet, Error, IndexEntry, IndexUpdateReport, Path, PersistentIndex, Result,
    WatchPlan, content_bytes, entry_position, revision, validate_unique_entries,
};

fn replace_affected_entries(
    entries: &mut Vec<IndexEntry>,
    root_index: usize,
    affected: &BTreeSet<String>,
    changed_entries: Vec<IndexEntry>,
) {
    let changed_paths = changed_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    for entry in changed_entries {
        match entry_position(entries, root_index, &entry.path) {
            Ok(position) => entries[position] = entry,
            Err(position) => entries.insert(position, entry),
        }
    }
    for path in affected.difference(&changed_paths) {
        if let Ok(position) = entry_position(entries, root_index, path) {
            entries.remove(position);
        }
    }
}

impl PersistentIndex {
    pub(super) fn apply_changed_entries(
        &mut self,
        root_index: usize,
        plan: &WatchPlan,
        changed_entries: Vec<IndexEntry>,
        report: Box<weavatrix_scan::ChangedContentVisitReport>,
    ) -> Result<IndexUpdateReport> {
        let affected = plan
            .changed
            .iter()
            .chain(&plan.removed)
            .chain(&report.removed)
            .cloned()
            .chain(changed_entries.iter().map(|entry| entry.path.clone()))
            .collect::<BTreeSet<_>>();
        let old_entries = affected
            .iter()
            .filter_map(|path| {
                entry_position(&self.entries, root_index, path)
                    .ok()
                    .map(|position| {
                        let entry = &self.entries[position];
                        (path.clone(), (entry.content_hash, entry.content.len()))
                    })
            })
            .collect::<BTreeMap<_, _>>();
        let updated = changed_entries
            .iter()
            .filter(|entry| {
                old_entries
                    .get(&entry.path)
                    .is_some_and(|(hash, _)| hash != &entry.content_hash)
            })
            .count();
        let retained = changed_entries
            .iter()
            .filter(|entry| old_entries.contains_key(&entry.path))
            .count();
        let added = changed_entries.len().saturating_sub(retained);
        let removed = old_entries.len().saturating_sub(retained);
        let old_content_bytes =
            old_entries.values().try_fold(0_u64, |total, (_, bytes)| {
                total
                    .checked_add(u64::try_from(*bytes).map_err(|_| {
                        Error::index("<memory>", "existing content size exceeds u64")
                    })?)
                    .ok_or_else(|| Error::index("<memory>", "content byte count overflow"))
            })?;
        let changed_content_bytes = content_bytes(&changed_entries, Path::new("<memory>"))?;
        let next_len = self
            .entries
            .len()
            .checked_sub(old_entries.len())
            .and_then(|count| count.checked_add(changed_entries.len()))
            .ok_or_else(|| Error::index("<memory>", "entry count overflow"))?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > self.options.max_entries {
            return Err(Error::index(
                "<memory>",
                format!("entry count exceeds {}", self.options.max_entries),
            ));
        }
        let next_content_bytes = self
            .content_bytes
            .checked_sub(old_content_bytes)
            .and_then(|bytes| bytes.checked_add(changed_content_bytes))
            .ok_or_else(|| Error::index("<memory>", "content byte count overflow"))?;
        if next_content_bytes > self.options.max_content_bytes {
            return Err(Error::index(
                "<memory>",
                format!("content exceeds {} bytes", self.options.max_content_bytes),
            ));
        }
        self.entries
            .try_reserve(changed_entries.len())
            .map_err(|error| {
                Error::index("<memory>", format!("entry allocation failed: {error}"))
            })?;
        replace_affected_entries(&mut self.entries, root_index, &affected, changed_entries);
        debug_assert!(validate_unique_entries(&self.entries, Path::new("<memory>")).is_ok());
        self.content_bytes = next_content_bytes;
        self.revision = revision(&self.roots, &self.entries);
        Ok(IndexUpdateReport {
            added: u64::try_from(added).unwrap_or(u64::MAX),
            updated: u64::try_from(updated).unwrap_or(u64::MAX),
            removed: u64::try_from(removed).unwrap_or(u64::MAX),
            files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
            content_bytes: self.content_bytes,
            revision: self.revision.clone(),
            full_rebuild: false,
            changed_scan: Some(report.content),
        })
    }
}
