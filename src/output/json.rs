use crate::report::{
    ContextLine, MatchSpan, MatchedFile, SearchBackend, SearchMatch, SearchReport, SearchWarning,
};
use std::io::{self, Write};

pub(super) fn write_json_lines(
    report: &SearchReport,
    destination: &mut impl Write,
) -> io::Result<()> {
    for found in &report.matches {
        write_match_json(destination, found)?;
    }
    for file in &report.matched_files {
        write_file_json(destination, file)?;
    }
    for warning in &report.warnings {
        write_warning_json(destination, warning)?;
    }
    write!(
        destination,
        "{{\"type\":\"summary\",\"data\":{{\"matching_lines\":{},\"occurrences\":{},\"files_with_matches\":{},\"files_searched\":{},\"bytes_searched\":{},\"truncated\":{},\"warnings_dropped\":{},\"backend\":\"{}\",\"index\":",
        report.matching_lines,
        report.occurrences,
        report.files_with_matches,
        report.files_searched,
        report.bytes_searched,
        report.truncated,
        report.warnings_dropped,
        match report.backend {
            SearchBackend::Filesystem => "filesystem",
            SearchBackend::PersistentIndex => "persistent-index",
            SearchBackend::LiveIndex => "live-index",
        },
    )?;
    if let Some(index) = &report.index {
        write!(
            destination,
            "{{\"revision\":\"{}\",\"indexed_files\":{},\"candidate_files\":{},\"prefiltered\":{}}}",
            index.revision, index.indexed_files, index.candidate_files, index.prefiltered
        )?;
    } else {
        write!(destination, "null")?;
    }
    write!(destination, ",\"roots\":")?;
    write!(destination, "[")?;
    for (index, root) in report.roots.iter().enumerate() {
        if index > 0 {
            write!(destination, ",")?;
        }
        write_json_string(destination, &root.to_string_lossy())?;
    }
    writeln!(destination, "]}}}}")
}

fn write_match_json(destination: &mut impl Write, found: &SearchMatch) -> io::Result<()> {
    write!(destination, "{{\"type\":\"match\",\"data\":{{\"path\":")?;
    write_json_string(destination, &found.path)?;
    write!(
        destination,
        ",\"root_index\":{},\"line_number\":{},\"end_line_number\":{},\"decoded_byte_offset\":{},\"source_byte_offset\":",
        found.root_index, found.line_number, found.end_line_number, found.decoded_byte_offset,
    )?;
    write_optional_u64(destination, found.source_byte_offset)?;
    write!(destination, ",\"line\":")?;
    write_json_string(destination, &found.line)?;
    write!(destination, ",\"replacement_preview\":")?;
    write_optional_string(destination, found.replacement_preview.as_deref())?;
    write!(destination, ",\"spans\":")?;
    write_spans(destination, &found.spans)?;
    write!(destination, ",\"before\":")?;
    write_context(destination, &found.before)?;
    write!(destination, ",\"after\":")?;
    write_context(destination, &found.after)?;
    write!(destination, ",\"encoding\":")?;
    write_json_string(destination, &found.encoding)?;
    writeln!(
        destination,
        ",\"lossy\":{},\"archive\":{}}}}}",
        found.lossy, found.archive
    )
}

fn write_file_json(destination: &mut impl Write, file: &MatchedFile) -> io::Result<()> {
    write!(destination, "{{\"type\":\"file\",\"data\":{{\"path\":")?;
    write_json_string(destination, &file.path)?;
    writeln!(
        destination,
        ",\"root_index\":{},\"matching_lines\":{},\"occurrences\":{},\"archive\":{}}}}}",
        file.root_index, file.matching_lines, file.occurrences, file.archive
    )
}

fn write_warning_json(destination: &mut impl Write, warning: &SearchWarning) -> io::Result<()> {
    write!(destination, "{{\"type\":\"warning\",\"data\":{{\"path\":")?;
    write_json_string(destination, &warning.path)?;
    write!(destination, ",\"kind\":")?;
    write_json_string(destination, warning_kind(warning.kind))?;
    write!(destination, ",\"message\":")?;
    write_json_string(destination, &warning.message)?;
    writeln!(destination, "}}}}")
}

const fn warning_kind(kind: crate::SearchWarningKind) -> &'static str {
    match kind {
        crate::SearchWarningKind::Binary => "binary",
        crate::SearchWarningKind::Encoding => "encoding",
        crate::SearchWarningKind::LineTooLong => "line_too_long",
        crate::SearchWarningKind::Archive => "archive",
        crate::SearchWarningKind::Limit => "limit",
    }
}

fn write_spans(destination: &mut impl Write, spans: &[MatchSpan]) -> io::Result<()> {
    write!(destination, "[")?;
    for (index, span) in spans.iter().enumerate() {
        if index > 0 {
            write!(destination, ",")?;
        }
        write!(
            destination,
            "{{\"pattern_index\":{},\"start\":{},\"end\":{}}}",
            span.pattern_index, span.start, span.end
        )?;
    }
    write!(destination, "]")
}

fn write_context(destination: &mut impl Write, context: &[ContextLine]) -> io::Result<()> {
    write!(destination, "[")?;
    for (index, line) in context.iter().enumerate() {
        if index > 0 {
            write!(destination, ",")?;
        }
        write!(
            destination,
            "{{\"line_number\":{},\"text\":",
            line.line_number
        )?;
        write_json_string(destination, &line.text)?;
        write!(destination, ",\"lossy\":{}}}", line.lossy)?;
    }
    write!(destination, "]")
}

fn write_optional_u64(destination: &mut impl Write, value: Option<u64>) -> io::Result<()> {
    match value {
        Some(value) => write!(destination, "{value}"),
        None => write!(destination, "null"),
    }
}

fn write_optional_string(destination: &mut impl Write, value: Option<&str>) -> io::Result<()> {
    match value {
        Some(value) => write_json_string(destination, value),
        None => write!(destination, "null"),
    }
}

fn write_json_string(destination: &mut impl Write, value: &str) -> io::Result<()> {
    write!(destination, "\"")?;
    for character in value.chars() {
        match character {
            '"' => write!(destination, "\\\"")?,
            '\\' => write!(destination, "\\\\")?,
            '\u{08}' => write!(destination, "\\b")?,
            '\u{0C}' => write!(destination, "\\f")?,
            '\n' => write!(destination, "\\n")?,
            '\r' => write!(destination, "\\r")?,
            '\t' => write!(destination, "\\t")?,
            character if character <= '\u{1F}' => {
                write!(destination, "\\u{:04x}", u32::from(character))?;
            }
            character => write!(destination, "{character}")?,
        }
    }
    write!(destination, "\"")
}
