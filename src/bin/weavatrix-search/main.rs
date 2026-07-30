#![forbid(unsafe_code)]

mod execution;
mod parsing;
mod presentation;
mod values;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;
use values::{
    next_number, next_path, next_string, parse_color, parse_discovery, parse_encoding,
    parse_number, same_roots,
};
use weavatrix_search::{
    CaseMode, ColorChoice, ContentDiscoveryMode, EncodingMode, IndexOptions, OutputFormat,
    OutputOptions, PersistentIndex, ResultMode, SearchMode, SearchOptions, SearchQuery, Searcher,
    recommended_scan_options, write_report_with, write_warnings,
};

const HELP: &str = "\
weavatrix-search
Fast, bounded, ignore-aware repository content search

USAGE:
    weavatrix-search [OPTIONS] PATTERN [PATH...]
    weavatrix-search [OPTIONS] -e PATTERN... [PATH...]

OPTIONS:
    -e, --regexp PATTERN          Add a search pattern (repeatable)
    -F, --fixed-strings           Treat every pattern as literal text
    -i, --ignore-case             Search case-insensitively
    -s, --case-sensitive          Search case-sensitively (default)
    -S, --smart-case              Ignore case unless a pattern has uppercase
    -A, --after-context NUM       Print NUM lines after matches
    -B, --before-context NUM      Print NUM lines before matches
    -C, --context NUM             Print NUM lines before and after matches
    -U, --multiline               Permit matches across line boundaries
    -c, --count                   Print matching-line counts per file
    -l, --files-with-matches      Print paths with matches
    -q, --quiet                   Stop after the first observed match
        --json                    Emit deterministic JSON Lines
        --heading                 Group matches below one path heading
        --no-heading              Print the path on every match (default)
    -n, --line-number             Print one-based line numbers (default)
    -N, --no-line-number          Suppress line numbers
        --column                  Print one-based UTF-8 byte columns
    -o, --only-matching           Print only matched substrings
        --color WHEN              auto, always, or never
    -0, --null                    NUL-terminate paths
        --stats                   Print elapsed/search counters to stderr
        --index PATH              Open, or create, a persistent content index
        --rebuild-index           Replace --index before searching
        --index-workers NUM       Bound index build/query workers
        --index-status PATH       Print validated index health and exit
    -r, --replace TEMPLATE        Preview replacement; never writes files
    -g, --glob GLOB               Include GLOB, or exclude !GLOB (repeatable)
        --encoding LABEL          auto, utf-8, utf-16le, utf-16be, or label
        --hidden                  Include hidden paths
        --no-archives             Disable archive decoding
        --discovery MODE          adaptive, streaming, or buffered
        --content-workers NUM     Bound concurrent source readers
        --max-results NUM         Bound retained match/file records
        --max-file-bytes NUM      Bound one ordinary source
        --max-line-bytes NUM      Bound one logical line
        --max-multiline-bytes NUM Bound one multiline source
        --max-replacement-bytes NUM
                                  Bound one replacement preview
        --max-decoder-memory-bytes NUM
                                  Bound configurable decoder memory
    -h, --help                    Print help
    -V, --version                 Print version

EXIT STATUS:
    0 at least one match, 1 no matches, 2 usage/search/output error
";

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(io::stderr(), "weavatrix-search: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<ExitCode, String> {
    let mut parsed = Arguments::parse(arguments)?;
    if execution::handle_immediate(&parsed)? {
        return Ok(ExitCode::SUCCESS);
    }
    let query = parsed.query()?;
    let options = execution::search_options(&mut parsed);
    let scan_options = execution::scan_options(&mut parsed, &options);
    let started = Instant::now();
    let report = execution::search(&mut parsed, query, options, scan_options)?;
    presentation::write(&parsed, &report)?;
    if parsed.stats {
        presentation::write_stats(&report, started.elapsed());
    }
    Ok(if report.files_with_matches > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// CLI switches are independently composable; replacing them with binary enums
// would add noise without strengthening the parser's state model.
#[allow(clippy::struct_excessive_bools)]
struct Arguments {
    patterns: Vec<String>,
    positional: Vec<OsString>,
    syntax: QuerySyntax,
    case: CaseMode,
    before_context: usize,
    after_context: usize,
    mode: SearchMode,
    result_mode: ResultMode,
    encoding: EncodingMode,
    replacement: Option<String>,
    globs: Vec<String>,
    hidden: bool,
    archives: bool,
    format: OutputFormat,
    heading: bool,
    line_number: bool,
    column: bool,
    only_matching: bool,
    null: bool,
    color: CliColor,
    stats: bool,
    max_results: usize,
    max_file_bytes: u64,
    max_line_bytes: usize,
    max_multiline_bytes: u64,
    max_replacement_bytes: usize,
    max_decoder_memory_bytes: usize,
    discovery: Option<ContentDiscoveryMode>,
    content_workers: Option<usize>,
    index: Option<PathBuf>,
    rebuild_index: bool,
    index_workers: usize,
    index_status: Option<PathBuf>,
    roots_explicit: bool,
    action: Option<ImmediateAction>,
    roots: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuerySyntax {
    Regex,
    Literal,
}

#[derive(Clone, Copy)]
enum ImmediateAction {
    Help,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliColor {
    Auto,
    Always,
    Never,
}
