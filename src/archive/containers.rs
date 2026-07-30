use super::{
    Arc, Collector, CompiledQuery, Component, Cursor, Error, Path, QueryCache, Read, Result,
    SearchIdentity, SearchOptions, SearchWarning, SearchWarningKind, search_complete_bytes,
};

#[cfg(feature = "archives")]
pub(super) fn search_zip(
    root_index: usize,
    outer_path: &str,
    bytes: &[u8],
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| Error::archive(outer_path, error.to_string()))?;
    let entries = archive.len().min(options.archives.max_entries);
    if archive.len() > options.archives.max_entries {
        warn_limit(
            collector,
            outer_path,
            format!(
                "archive has {} entries; only the first {} are searched",
                archive.len(),
                options.archives.max_entries
            ),
        );
    }
    let mut expanded = 0_u64;
    for index in 0..entries {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::archive(outer_path, error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name().as_deref().and_then(safe_virtual_path) else {
            collector.warn(SearchWarning {
                path: outer_path.to_owned(),
                kind: SearchWarningKind::Archive,
                message: format!("unsafe ZIP member path was skipped: {}", entry.name()),
            });
            continue;
        };
        let size = entry.size();
        if size > options.archives.max_entry_bytes
            || expanded.saturating_add(size) > options.archives.max_expanded_bytes
        {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        let content = read_limited(
            &mut entry,
            options.archives.max_entry_bytes,
            &format!("{outer_path}!{path}"),
        )?;
        let next_expanded =
            expanded.saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
        if next_expanded > options.archives.max_expanded_bytes {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        expanded = next_expanded;
        search_complete_bytes(
            SearchIdentity {
                root_index,
                path: format!("{outer_path}!{path}"),
                source_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                encoding: String::new().into(),
                archive: true,
                source_offset_base: Some(0),
                lossy: false,
            },
            &content,
            Arc::clone(query),
            query_cache,
            Arc::clone(options),
            Arc::clone(collector),
        )?;
    }
    Ok(())
}

#[cfg(feature = "archives")]
pub(super) fn search_tar<R: Read>(
    root_index: usize,
    outer_path: &str,
    reader: R,
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| Error::archive(outer_path, error.to_string()))?;
    let mut expanded = 0_u64;
    for (index, entry) in entries.enumerate() {
        if index >= options.archives.max_entries {
            warn_limit(
                collector,
                outer_path,
                format!(
                    "archive entry count exceeds {}; remaining members were skipped",
                    options.archives.max_entries
                ),
            );
            break;
        }
        let mut entry = entry.map_err(|error| Error::archive(outer_path, error.to_string()))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let member = entry
            .path()
            .map_err(|error| Error::archive(outer_path, error.to_string()))?;
        let Some(path) = safe_virtual_path(&member) else {
            collector.warn(SearchWarning {
                path: outer_path.to_owned(),
                kind: SearchWarningKind::Archive,
                message: format!(
                    "unsafe TAR member path was skipped: {}",
                    member.to_string_lossy()
                ),
            });
            continue;
        };
        let size = entry.size();
        if size > options.archives.max_entry_bytes
            || expanded.saturating_add(size) > options.archives.max_expanded_bytes
        {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        let content = read_limited(
            &mut entry,
            options.archives.max_entry_bytes,
            &format!("{outer_path}!{path}"),
        )?;
        let next_expanded =
            expanded.saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
        if next_expanded > options.archives.max_expanded_bytes {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        expanded = next_expanded;
        search_complete_bytes(
            SearchIdentity {
                root_index,
                path: format!("{outer_path}!{path}"),
                source_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                encoding: String::new().into(),
                archive: true,
                source_offset_base: Some(0),
                lossy: false,
            },
            &content,
            Arc::clone(query),
            query_cache,
            Arc::clone(options),
            Arc::clone(collector),
        )?;
    }
    Ok(())
}

#[cfg(feature = "archives")]
pub(super) fn read_limited(reader: impl Read, limit: u64, path: &str) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|error| Error::io(path, error))?;
    if u64::try_from(content.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::archive(
            path,
            format!("expanded content exceeds the {limit} byte limit"),
        ));
    }
    Ok(content)
}

#[cfg(feature = "archives")]
fn safe_virtual_path(path: &Path) -> Option<String> {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&value.to_string_lossy());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(feature = "archives")]
fn warn_limit(collector: &Collector, path: &str, message: String) {
    collector.warn(SearchWarning {
        path: path.to_owned(),
        kind: SearchWarningKind::Limit,
        message,
    });
}
