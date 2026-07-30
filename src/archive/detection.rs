use super::ArchiveKind;
use std::path::Path;

pub(crate) fn kind(path: &str) -> Option<ArchiveKind> {
    let path = Path::new(path);
    let path = path.to_string_lossy();
    if has_suffix(&path, &[".tgz", ".tar.gz"]) {
        return Some(ArchiveKind::TarGzip);
    }
    if has_suffix(&path, &[".tbz", ".tbz2", ".tar.bz2", ".tar.bz"]) {
        return Some(ArchiveKind::TarBzip2);
    }
    if has_suffix(&path, &[".tzst", ".tar.zst", ".tar.zstd"]) {
        return Some(ArchiveKind::TarZstd);
    }
    if has_suffix(&path, &[".tar.lz4"]) {
        return Some(ArchiveKind::TarLz4);
    }
    if has_suffix(&path, &[".tlz", ".tar.lzma"]) {
        return Some(ArchiveKind::TarLzma);
    }
    if has_suffix(&path, &[".txz", ".tar.xz"]) {
        return Some(ArchiveKind::TarXz);
    }
    if has_suffix(&path, &[".tar.br"]) {
        return Some(ArchiveKind::TarBrotli);
    }
    if has_suffix(&path, &[".zip"]) {
        return Some(ArchiveKind::Zip);
    }
    if has_suffix(&path, &[".tar"]) {
        return Some(ArchiveKind::Tar);
    }
    if has_suffix(&path, &[".gz"]) {
        return Some(ArchiveKind::Gzip);
    }
    if has_suffix(&path, &[".bz2", ".bz"]) {
        return Some(ArchiveKind::Bzip2);
    }
    if has_suffix(&path, &[".zst", ".zstd"]) {
        return Some(ArchiveKind::Zstd);
    }
    if has_suffix(&path, &[".lz4"]) {
        return Some(ArchiveKind::Lz4);
    }
    if has_suffix(&path, &[".lzma"]) {
        return Some(ArchiveKind::Lzma);
    }
    if has_suffix(&path, &[".xz"]) {
        return Some(ArchiveKind::Xz);
    }
    if has_suffix(&path, &[".br"]) {
        return Some(ArchiveKind::Brotli);
    }
    None
}

pub(super) fn strip_suffix_ignore_ascii_case<'a>(
    path: &'a str,
    suffixes: &[&str],
) -> Option<&'a str> {
    suffixes.iter().find_map(|suffix| {
        path.get(path.len().checked_sub(suffix.len())?..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
            .then(|| &path[..path.len() - suffix.len()])
    })
}

fn has_suffix(path: &str, suffixes: &[&str]) -> bool {
    strip_suffix_ignore_ascii_case(path, suffixes).is_some()
}
