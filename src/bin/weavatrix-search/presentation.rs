use super::{
    Arguments, CliColor, ColorChoice, IsTerminal, OutputFormat, OutputOptions, io,
    write_report_with, write_warnings,
};
use std::time::Duration;

pub(super) fn write(
    arguments: &Arguments,
    report: &weavatrix_search::SearchReport,
) -> Result<(), String> {
    let color = match arguments.color {
        CliColor::Auto if io::stdout().is_terminal() => ColorChoice::Always,
        CliColor::Always => ColorChoice::Always,
        CliColor::Auto | CliColor::Never => ColorChoice::Never,
    };
    let output = OutputOptions {
        format: arguments.format,
        heading: arguments.heading,
        line_number: arguments.line_number,
        column: arguments.column,
        only_matching: arguments.only_matching,
        null: arguments.null,
        color,
    };
    write_report_with(report, &output, io::stdout().lock()).map_err(|error| error.to_string())?;
    if arguments.format == OutputFormat::Text {
        write_warnings(report, io::stderr().lock()).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn write_stats(report: &weavatrix_search::SearchReport, elapsed: Duration) {
    let index_stats = report.index.as_ref().map_or_else(String::new, |index| {
        format!(
            ", indexed {}, candidates {}, revision {}",
            index.indexed_files, index.candidate_files, index.revision
        )
    });
    eprintln!(
        "weavatrix-search: {} files, {} bytes, {} matching lines, {:.3} ms{}",
        report.files_searched,
        report.bytes_searched,
        report.matching_lines,
        elapsed.as_secs_f64() * 1_000.0,
        index_stats,
    );
}
