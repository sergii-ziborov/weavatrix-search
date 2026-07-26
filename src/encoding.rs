use crate::collector::Collector;
use crate::error::{Error, Result};
use crate::line_search::{LineSearcher, SearchIdentity};
use crate::multiline;
use crate::options::{BinaryPolicy, EncodingMode, SearchMode, SearchOptions};
use crate::query::{CompiledQuery, QueryCache};
use crate::report::{SearchWarning, SearchWarningKind};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use std::borrow::Cow;
use std::sync::Arc;

pub(crate) fn is_streaming_utf8(mode: &EncodingMode) -> Result<bool> {
    match mode {
        EncodingMode::Auto | EncodingMode::Utf8 => Ok(true),
        EncodingMode::Utf16Le | EncodingMode::Utf16Be => Ok(false),
        EncodingMode::Label(label) => Encoding::for_label(label.as_bytes())
            .map(|encoding| encoding == UTF_8)
            .ok_or_else(|| Error::InvalidEncoding(label.clone())),
    }
}

pub(crate) fn auto_is_utf16(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xFF\xFE") || bytes.starts_with(b"\xFE\xFF")
}

pub(crate) fn utf8_bom_len(bytes: &[u8]) -> usize {
    usize::from(bytes.starts_with(b"\xEF\xBB\xBF")) * 3
}

pub(crate) fn search_complete_bytes(
    mut identity: SearchIdentity,
    bytes: &[u8],
    query: Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: Arc<SearchOptions>,
    collector: Arc<Collector>,
) -> Result<()> {
    let encoding = resolve_encoding(&options.encoding, bytes)?;
    let (decoded, had_errors) = encoding.decode_with_bom_removal(bytes);
    identity.encoding = Cow::Borrowed(encoding.name());
    identity.source_offset_base = (encoding == UTF_8)
        .then(|| u64::try_from(utf8_bom_len(bytes)).expect("UTF-8 BOM length fits in u64"));
    identity.lossy = had_errors;
    if had_errors {
        collector.warn(SearchWarning {
            path: identity.path.clone(),
            kind: SearchWarningKind::Encoding,
            message: format!(
                "{} contained malformed {} sequences; replacement characters were used",
                identity.path,
                encoding.name()
            ),
        });
    }
    if options.binary == BinaryPolicy::Skip
        && memchr::memchr(0, &decoded.as_bytes()[..decoded.len().min(8 * 1024)]).is_some()
    {
        collector.warn(SearchWarning {
            path: identity.path,
            kind: SearchWarningKind::Binary,
            message: "binary file skipped after NUL-byte detection".to_owned(),
        });
        return Ok(());
    }
    if options.mode == SearchMode::Multiline {
        multiline::search(
            identity,
            &decoded,
            &query,
            query_cache,
            &options,
            &collector,
        );
    } else {
        let mut lines = LineSearcher::new(query, options, collector, identity);
        lines.push(decoded.as_bytes(), query_cache);
        lines.finish(query_cache);
    }
    Ok(())
}

fn resolve_encoding(mode: &EncodingMode, bytes: &[u8]) -> Result<&'static Encoding> {
    match mode {
        EncodingMode::Auto => {
            if bytes.starts_with(b"\xFF\xFE") {
                Ok(UTF_16LE)
            } else if bytes.starts_with(b"\xFE\xFF") {
                Ok(UTF_16BE)
            } else {
                Ok(UTF_8)
            }
        }
        EncodingMode::Utf8 => Ok(UTF_8),
        EncodingMode::Utf16Le => Ok(UTF_16LE),
        EncodingMode::Utf16Be => Ok(UTF_16BE),
        EncodingMode::Label(label) => Encoding::for_label(label.as_bytes())
            .ok_or_else(|| Error::InvalidEncoding(label.clone())),
    }
}
