use crate::{
    ContextLine, MatchSpan, MatchedFile, ResultMode, SearchMatch, SearchReport, SearchWarning,
};
use std::collections::{BTreeSet, HashSet};
use std::io::{self, Write};

/// Stable built-in report serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Grep-style deterministic text records.
    #[default]
    Text,
    /// Self-contained newline-delimited JSON records.
    JsonLines,
}

/// ANSI color policy for deterministic text output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    /// Never write ANSI escape sequences.
    #[default]
    Never,
    /// Highlight paths, line numbers, and matching spans.
    Always,
}

/// Presentation policy for built-in report serialization.
///
/// Independent switches are intentional here because CLI and library users
/// compose them freely rather than selecting one exclusive presentation mode.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputOptions {
    /// Text or JSON Lines serialization.
    pub format: OutputFormat,
    /// Print one path heading followed by line records.
    pub heading: bool,
    /// Include one-based line numbers in text records.
    pub line_number: bool,
    /// Include one-based UTF-8 byte columns for matching records.
    pub column: bool,
    /// Emit each matched substring instead of its complete matching line.
    pub only_matching: bool,
    /// Terminate file paths with NUL in text output.
    pub null: bool,
    /// ANSI color policy for text output.
    pub color: ColorChoice,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            format: OutputFormat::Text,
            heading: false,
            line_number: true,
            column: false,
            only_matching: false,
            null: false,
            color: ColorChoice::Never,
        }
    }
}

/// Writes a report in a stable built-in format.
///
/// Text output contains match, count, or path records according to the
/// report's result mode. JSON Lines additionally contains warnings and a final
/// summary record.
///
/// # Errors
///
/// Returns an error when the destination cannot accept the complete output.
pub fn write_report(
    report: &SearchReport,
    format: OutputFormat,
    mut destination: impl Write,
) -> io::Result<()> {
    write_report_with(
        report,
        &OutputOptions {
            format,
            ..OutputOptions::default()
        },
        &mut destination,
    )
}

/// Writes a report using explicit presentation policy.
///
/// # Errors
///
/// Returns an error when the destination cannot accept the complete output.
pub fn write_report_with(
    report: &SearchReport,
    options: &OutputOptions,
    mut destination: impl Write,
) -> io::Result<()> {
    let options = *options;
    match options.format {
        OutputFormat::Text => write_text(report, options, &mut destination),
        OutputFormat::JsonLines => write_json_lines(report, &mut destination),
    }
}

/// Writes deterministic human-readable warnings.
///
/// # Errors
///
/// Returns an error when the destination cannot accept the complete output.
pub fn write_warnings(report: &SearchReport, mut destination: impl Write) -> io::Result<()> {
    for warning in &report.warnings {
        writeln!(
            destination,
            "weavatrix-search: {}: {}",
            warning.path, warning.message
        )?;
    }
    if report.warnings_dropped > 0 {
        writeln!(
            destination,
            "weavatrix-search: {} additional warnings omitted",
            report.warnings_dropped
        )?;
    }
    Ok(())
}

fn write_text(
    report: &SearchReport,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    match report.result_mode {
        ResultMode::Matches => write_matches(report, options, destination)?,
        ResultMode::Count => {
            for file in &report.matched_files {
                let path = display_path(report, file.root_index, &file.path);
                write_path(destination, &path, options)?;
                writeln!(destination, "{}", file.matching_lines)?;
            }
        }
        ResultMode::Files => {
            for file in &report.matched_files {
                let path = display_path(report, file.root_index, &file.path);
                if options.null {
                    write!(destination, "{path}\0")?;
                } else {
                    writeln!(destination, "{path}")?;
                }
            }
        }
        ResultMode::Quiet => {}
    }
    Ok(())
}

fn write_matches(
    report: &SearchReport,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    let matching_lines = report
        .matches
        .iter()
        .flat_map(|found| {
            (found.line_number..=found.end_line_number)
                .map(move |line| (found.root_index, found.path.as_str(), line))
        })
        .collect::<HashSet<_>>();
    let mut emitted_context = BTreeSet::new();
    let mut current_heading = None;
    for found in &report.matches {
        let path = display_path(report, found.root_index, &found.path);
        if options.heading && current_heading.as_deref() != Some(path.as_str()) {
            if current_heading.is_some() {
                writeln!(destination)?;
            }
            write_heading(destination, &path, options)?;
            current_heading = Some(path.clone());
        }
        write_context_lines(
            found,
            &found.before,
            &path,
            &matching_lines,
            &mut emitted_context,
            options,
            destination,
        )?;
        write_found(found, &path, options, destination)?;
        write_context_lines(
            found,
            &found.after,
            &path,
            &matching_lines,
            &mut emitted_context,
            options,
            destination,
        )?;
    }
    Ok(())
}

fn write_context_lines(
    found: &SearchMatch,
    lines: &[ContextLine],
    path: &str,
    matching_lines: &HashSet<(usize, &str, u64)>,
    emitted: &mut BTreeSet<(usize, String, u64)>,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    for context in lines {
        let key = (found.root_index, found.path.as_str(), context.line_number);
        if !matching_lines.contains(&key)
            && emitted.insert((found.root_index, found.path.clone(), context.line_number))
        {
            write_text_line(
                destination,
                TextRecord {
                    path,
                    line_number: context.line_number,
                    column: None,
                    separator: '-',
                    text: &context.text,
                    spans: &[],
                },
                options,
            )?;
        }
    }
    Ok(())
}

fn write_found(
    found: &SearchMatch,
    path: &str,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    if options.only_matching {
        for span in &found.spans {
            let text = found.line.get(span.start..span.end).unwrap_or_default();
            let (line_number, column) = line_and_column(&found.line, found.line_number, span.start);
            write_text_line(
                destination,
                TextRecord {
                    path,
                    line_number,
                    column: Some(column),
                    separator: ':',
                    text,
                    spans: &[],
                },
                options,
            )?;
        }
        return Ok(());
    }

    let preview = found.replacement_preview.as_deref();
    let column = found
        .spans
        .first()
        .map(|span| line_and_column(&found.line, found.line_number, span.start).1);
    write_text_line(
        destination,
        TextRecord {
            path,
            line_number: found.line_number,
            column,
            separator: ':',
            text: preview.unwrap_or(&found.line),
            spans: if preview.is_some() { &[] } else { &found.spans },
        },
        options,
    )
}

#[derive(Clone, Copy)]
struct TextRecord<'a> {
    path: &'a str,
    line_number: u64,
    column: Option<usize>,
    separator: char,
    text: &'a str,
    spans: &'a [MatchSpan],
}

fn write_text_line(
    destination: &mut impl Write,
    record: TextRecord<'_>,
    options: OutputOptions,
) -> io::Result<()> {
    if !options.heading {
        write_colored(destination, record.path, "\x1b[35m", options.color)?;
        if options.null {
            write!(destination, "\0")?;
        } else {
            write!(destination, "{}", record.separator)?;
        }
    }
    if options.line_number {
        write_colored(
            destination,
            &record.line_number.to_string(),
            "\x1b[32m",
            options.color,
        )?;
        write!(destination, "{}", record.separator)?;
    }
    if options.column && record.separator == ':' {
        write_colored(
            destination,
            &record.column.unwrap_or(1).to_string(),
            "\x1b[32m",
            options.color,
        )?;
        write!(destination, "{}", record.separator)?;
    }
    write_highlighted(destination, record.text, record.spans, options.color)?;
    if !record.text.ends_with('\n') {
        writeln!(destination)?;
    }
    Ok(())
}

fn write_json_lines(report: &SearchReport, destination: &mut impl Write) -> io::Result<()> {
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
        "{{\"type\":\"summary\",\"data\":{{\"matching_lines\":{},\"occurrences\":{},\"files_with_matches\":{},\"files_searched\":{},\"bytes_searched\":{},\"truncated\":{},\"warnings_dropped\":{},\"roots\":",
        report.matching_lines,
        report.occurrences,
        report.files_with_matches,
        report.files_searched,
        report.bytes_searched,
        report.truncated,
        report.warnings_dropped,
    )?;
    write!(destination, "[")?;
    for (index, root) in report.roots.iter().enumerate() {
        if index > 0 {
            write!(destination, ",")?;
        }
        write_json_string(destination, &root.to_string_lossy())?;
    }
    writeln!(destination, "]}}}}")
}

fn display_path(report: &SearchReport, root_index: usize, relative: &str) -> String {
    if report.roots.len() <= 1 {
        return relative.to_owned();
    }
    report
        .roots
        .get(root_index)
        .map_or_else(
            || format!("{root_index}/{relative}"),
            |root| root.join(relative).to_string_lossy().into_owned(),
        )
        .replace('\\', "/")
}

fn line_and_column(text: &str, first_line: u64, offset: usize) -> (u64, usize) {
    let prefix = text.get(..offset).unwrap_or_default();
    let extra_lines =
        u64::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).unwrap_or(u64::MAX);
    let line_start = prefix
        .rfind('\n')
        .map_or(0, |index| index.saturating_add(1));
    (
        first_line.saturating_add(extra_lines),
        offset.saturating_sub(line_start).saturating_add(1),
    )
}

fn write_heading(
    destination: &mut impl Write,
    path: &str,
    options: OutputOptions,
) -> io::Result<()> {
    write_colored(destination, path, "\x1b[35m", options.color)?;
    if options.null {
        write!(destination, "\0")
    } else {
        writeln!(destination)
    }
}

fn write_path(destination: &mut impl Write, path: &str, options: OutputOptions) -> io::Result<()> {
    write_colored(destination, path, "\x1b[35m", options.color)?;
    if options.null {
        write!(destination, "\0")
    } else {
        write!(destination, ":")
    }
}

fn write_colored(
    destination: &mut impl Write,
    text: &str,
    color: &str,
    choice: ColorChoice,
) -> io::Result<()> {
    if choice == ColorChoice::Always {
        write!(destination, "{color}{text}\x1b[0m")
    } else {
        write!(destination, "{text}")
    }
}

fn write_highlighted(
    destination: &mut impl Write,
    text: &str,
    spans: &[MatchSpan],
    choice: ColorChoice,
) -> io::Result<()> {
    if choice == ColorChoice::Never || spans.is_empty() {
        return write!(destination, "{text}");
    }
    let mut cursor = 0;
    for span in spans {
        let Some(prefix) = text.get(cursor..span.start) else {
            return write!(destination, "{text}");
        };
        let Some(matched) = text.get(span.start..span.end) else {
            return write!(destination, "{text}");
        };
        write!(destination, "{prefix}\x1b[1;31m{matched}\x1b[0m")?;
        cursor = span.end;
    }
    write!(destination, "{}", text.get(cursor..).unwrap_or_default())
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
