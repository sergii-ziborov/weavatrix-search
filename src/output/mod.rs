mod json;
mod text;

use crate::report::SearchReport;
use json::write_json_lines;
use std::io::{self, Write};
use text::write_text;

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
