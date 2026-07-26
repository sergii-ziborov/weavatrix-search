#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;
use weavatrix_scan::{ContentDiscoveryMode, ContentValidationPolicy, ScanOptions};
use weavatrix_search::{
    CaseMode, ColorChoice, EncodingMode, IndexOptions, OutputFormat, OutputOptions,
    PersistentIndex, ResultMode, SearchMode, SearchOptions, SearchQuery, Searcher,
    write_report_with, write_warnings,
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

#[allow(clippy::too_many_lines)]
fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<ExitCode, String> {
    let parsed = Arguments::parse(arguments)?;
    if let Some(action) = parsed.action {
        match action {
            ImmediateAction::Help => print!("{HELP}"),
            ImmediateAction::Version => {
                println!("weavatrix-search {}", env!("CARGO_PKG_VERSION"));
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(path) = &parsed.index_status {
        let index = PersistentIndex::open(path, IndexOptions::default())
            .map_err(|error| error.to_string())?;
        let status = index.status();
        println!(
            "revision={} files={} bytes={} roots={}",
            status.revision,
            status.files,
            status.content_bytes,
            status.roots.len()
        );
        for (root_index, root) in status.roots.iter().enumerate() {
            println!("root[{root_index}]={}", root.display());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let query = parsed.query()?;
    let mut options = SearchOptions::default()
        .with_case(parsed.case)
        .with_context(parsed.before_context, parsed.after_context)
        .with_mode(parsed.mode)
        .with_result_mode(parsed.result_mode)
        .with_encoding(parsed.encoding)
        .with_max_results(parsed.max_results)
        .with_max_file_bytes(parsed.max_file_bytes)
        .with_max_line_bytes(parsed.max_line_bytes)
        .with_max_multiline_bytes(parsed.max_multiline_bytes)
        .with_max_replacement_bytes(parsed.max_replacement_bytes);
    options.archives.enabled = parsed.archives;
    options.archives.max_decoder_memory_bytes = parsed.max_decoder_memory_bytes;
    if let Some(replacement) = parsed.replacement {
        options = options.with_replacement(replacement);
    }

    let scanner_limit = if options.archives.enabled {
        options
            .max_file_bytes
            .max(options.archives.max_archive_bytes)
    } else {
        options.max_file_bytes
    };
    let mut scan_options = ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_skip_hidden(!parsed.hidden)
        .with_content_parallelism(if cfg!(windows) { 8 } else { 16 })
        .with_content_discovery(ContentDiscoveryMode::BufferedParallel)
        .with_content_validation(ContentValidationPolicy::Fast)
        .with_override_rules(parsed.globs);
    scan_options.max_file_bytes = scanner_limit;

    let started = Instant::now();
    let report = if let Some(index_path) = parsed.index {
        let index_options = IndexOptions::default().with_parallelism(parsed.index_workers);
        let index = if parsed.rebuild_index || !index_path.exists() {
            PersistentIndex::build_and_save(&index_path, parsed.roots, scan_options, index_options)
                .map_err(|error| error.to_string())?
                .0
        } else {
            let index = PersistentIndex::open(&index_path, index_options)
                .map_err(|error| error.to_string())?;
            if parsed.roots_explicit && !same_roots(index.roots(), &parsed.roots) {
                return Err(
                    "explicit PATH arguments differ from the index; use --rebuild-index".to_owned(),
                );
            }
            index
        };
        index
            .search(query, options)
            .map_err(|error| error.to_string())?
    } else {
        let mut roots = parsed.roots.into_iter();
        let root = roots
            .next()
            .expect("argument parsing always supplies one root");
        Searcher::new(root, query)
            .extend_roots(roots)
            .options(options)
            .scan_options(scan_options)
            .search()
            .map_err(|error| error.to_string())?
    };
    let color = match parsed.color {
        CliColor::Auto if io::stdout().is_terminal() => ColorChoice::Always,
        CliColor::Always => ColorChoice::Always,
        CliColor::Auto | CliColor::Never => ColorChoice::Never,
    };
    let output = OutputOptions {
        format: parsed.format,
        heading: parsed.heading,
        line_number: parsed.line_number,
        column: parsed.column,
        only_matching: parsed.only_matching,
        null: parsed.null,
        color,
    };
    write_report_with(&report, &output, io::stdout().lock()).map_err(|error| error.to_string())?;
    if parsed.format == OutputFormat::Text {
        write_warnings(&report, io::stderr().lock()).map_err(|error| error.to_string())?;
    }
    if parsed.stats {
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
            started.elapsed().as_secs_f64() * 1_000.0,
            index_stats,
        );
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

impl Arguments {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let defaults = SearchOptions::default();
        let mut parsed = Self {
            patterns: Vec::new(),
            positional: Vec::new(),
            syntax: QuerySyntax::Regex,
            case: defaults.case,
            before_context: 0,
            after_context: 0,
            mode: SearchMode::Line,
            result_mode: ResultMode::Matches,
            encoding: EncodingMode::Auto,
            replacement: None,
            globs: Vec::new(),
            hidden: false,
            archives: defaults.archives.enabled,
            format: OutputFormat::Text,
            heading: false,
            line_number: true,
            column: false,
            only_matching: false,
            null: false,
            color: CliColor::Auto,
            stats: false,
            max_results: defaults.max_results,
            max_file_bytes: defaults.max_file_bytes,
            max_line_bytes: defaults.max_line_bytes,
            max_multiline_bytes: defaults.max_multiline_bytes,
            max_replacement_bytes: defaults.max_replacement_bytes,
            max_decoder_memory_bytes: defaults.archives.max_decoder_memory_bytes,
            index: None,
            rebuild_index: false,
            index_workers: IndexOptions::default().search_parallelism,
            index_status: None,
            roots_explicit: false,
            action: None,
            roots: Vec::new(),
        };
        let mut arguments = arguments.into_iter();
        let mut options = true;
        while let Some(argument) = arguments.next() {
            if options && argument == "--" {
                options = false;
                continue;
            }
            let is_option = options
                && argument
                    .to_str()
                    .is_some_and(|value| value.starts_with('-') && value != "-");
            if !is_option {
                parsed.positional.push(argument);
                continue;
            }
            let argument = argument
                .into_string()
                .expect("option-like arguments were verified as UTF-8");
            parsed.parse_option(&argument, &mut arguments)?;
        }
        parsed.finish()
    }

    fn parse_option(
        &mut self,
        argument: &str,
        arguments: &mut impl Iterator<Item = OsString>,
    ) -> Result<(), String> {
        match argument {
            "-h" | "--help" => self.action = Some(ImmediateAction::Help),
            "-V" | "--version" => self.action = Some(ImmediateAction::Version),
            "-F" | "--fixed-strings" => self.syntax = QuerySyntax::Literal,
            "-i" | "--ignore-case" => self.case = CaseMode::Insensitive,
            "-s" | "--case-sensitive" => self.case = CaseMode::Sensitive,
            "-S" | "--smart-case" => self.case = CaseMode::Smart,
            "-U" | "--multiline" => self.mode = SearchMode::Multiline,
            "-c" | "--count" => self.set_result_mode(ResultMode::Count)?,
            "-l" | "--files-with-matches" => self.set_result_mode(ResultMode::Files)?,
            "-q" | "--quiet" => self.set_result_mode(ResultMode::Quiet)?,
            "--json" => self.format = OutputFormat::JsonLines,
            "--heading" => self.heading = true,
            "--no-heading" => self.heading = false,
            "-n" | "--line-number" => self.line_number = true,
            "-N" | "--no-line-number" => self.line_number = false,
            "--column" => {
                self.column = true;
                self.line_number = true;
            }
            "-o" | "--only-matching" => self.only_matching = true,
            "-0" | "--null" => self.null = true,
            "--stats" => self.stats = true,
            "--index" => self.index = Some(next_path(arguments, argument)?),
            "--rebuild-index" => self.rebuild_index = true,
            "--index-workers" => self.index_workers = next_number(arguments, argument)?,
            "--index-status" => self.index_status = Some(next_path(arguments, argument)?),
            "--hidden" => self.hidden = true,
            "--no-archives" => self.archives = false,
            "-e" | "--regexp" => self.patterns.push(next_string(arguments, argument)?),
            "-r" | "--replace" => self.replacement = Some(next_string(arguments, argument)?),
            "-g" | "--glob" => self.globs.push(next_string(arguments, argument)?),
            "-A" | "--after-context" => {
                self.after_context = next_number(arguments, argument)?;
            }
            "-B" | "--before-context" => {
                self.before_context = next_number(arguments, argument)?;
            }
            "-C" | "--context" => {
                let context = next_number(arguments, argument)?;
                self.before_context = context;
                self.after_context = context;
            }
            "--encoding" => {
                self.encoding = parse_encoding(&next_string(arguments, argument)?)?;
            }
            "--color" => {
                self.color = parse_color(&next_string(arguments, argument)?)?;
            }
            "--max-results" => self.max_results = next_number(arguments, argument)?,
            "--max-file-bytes" => self.max_file_bytes = next_number(arguments, argument)?,
            "--max-line-bytes" => self.max_line_bytes = next_number(arguments, argument)?,
            "--max-multiline-bytes" => {
                self.max_multiline_bytes = next_number(arguments, argument)?;
            }
            "--max-replacement-bytes" => {
                self.max_replacement_bytes = next_number(arguments, argument)?;
            }
            "--max-decoder-memory-bytes" => {
                self.max_decoder_memory_bytes = next_number(arguments, argument)?;
            }
            _ => {
                if let Some((name, value)) = argument.split_once('=') {
                    self.parse_assignment(name, value)?;
                } else {
                    return Err(format!("unknown option {argument}; use --help"));
                }
            }
        }
        Ok(())
    }

    fn parse_assignment(&mut self, name: &str, value: &str) -> Result<(), String> {
        match name {
            "--regexp" => self.patterns.push(value.to_owned()),
            "--replace" => self.replacement = Some(value.to_owned()),
            "--glob" => self.globs.push(value.to_owned()),
            "--encoding" => self.encoding = parse_encoding(value)?,
            "--color" => self.color = parse_color(value)?,
            "--after-context" => self.after_context = parse_number(name, value)?,
            "--before-context" => self.before_context = parse_number(name, value)?,
            "--context" => {
                let context = parse_number(name, value)?;
                self.before_context = context;
                self.after_context = context;
            }
            "--max-results" => self.max_results = parse_number(name, value)?,
            "--max-file-bytes" => self.max_file_bytes = parse_number(name, value)?,
            "--max-line-bytes" => self.max_line_bytes = parse_number(name, value)?,
            "--max-multiline-bytes" => self.max_multiline_bytes = parse_number(name, value)?,
            "--max-replacement-bytes" => {
                self.max_replacement_bytes = parse_number(name, value)?;
            }
            "--max-decoder-memory-bytes" => {
                self.max_decoder_memory_bytes = parse_number(name, value)?;
            }
            "--index" => self.index = Some(PathBuf::from(value)),
            "--index-workers" => self.index_workers = parse_number(name, value)?,
            "--index-status" => self.index_status = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {name}; use --help")),
        }
        Ok(())
    }

    fn set_result_mode(&mut self, mode: ResultMode) -> Result<(), String> {
        if self.result_mode != ResultMode::Matches && self.result_mode != mode {
            return Err(
                "only one of --count, --files-with-matches, or --quiet is allowed".to_owned(),
            );
        }
        self.result_mode = mode;
        Ok(())
    }

    fn finish(mut self) -> Result<Self, String> {
        if self.action.is_some() || self.index_status.is_some() {
            return Ok(self);
        }
        if self.patterns.is_empty() {
            if self.positional.is_empty() {
                return Err("a PATTERN is required; use --help".to_owned());
            }
            self.patterns.push(
                self.positional
                    .remove(0)
                    .into_string()
                    .map_err(|_| "PATTERN must be valid UTF-8".to_owned())?,
            );
        }
        if self.positional.is_empty() {
            self.roots.push(PathBuf::from("."));
        } else {
            self.roots_explicit = true;
            self.roots
                .extend(self.positional.drain(..).map(PathBuf::from));
        }
        if self.rebuild_index && self.index.is_none() {
            return Err("--rebuild-index requires --index PATH".to_owned());
        }
        if self.replacement.is_some() && self.result_mode != ResultMode::Matches {
            return Err("--replace requires match output mode".to_owned());
        }
        if self.replacement.is_some() && self.only_matching {
            return Err("--replace and --only-matching cannot be combined".to_owned());
        }
        if self.only_matching && (self.before_context > 0 || self.after_context > 0) {
            return Err("--only-matching cannot be combined with context".to_owned());
        }
        Ok(self)
    }

    fn query(&self) -> Result<SearchQuery, String> {
        if self.patterns.iter().any(String::is_empty) {
            return Err("search patterns must not be empty".to_owned());
        }
        let mut queries = self.patterns.iter().map(|pattern| {
            if self.syntax == QuerySyntax::Literal {
                SearchQuery::literal(pattern)
            } else {
                SearchQuery::regex(pattern)
            }
        });
        let first = queries
            .next()
            .ok_or_else(|| "a PATTERN is required".to_owned())?;
        Ok(if self.patterns.len() == 1 {
            first
        } else {
            SearchQuery::any(std::iter::once(first).chain(queries))
        })
    }
}

fn next_string(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))?
        .into_string()
        .map_err(|_| format!("{option} value must be valid UTF-8"))
}

fn next_path(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{option} requires a path"))
}

fn next_number<T>(arguments: &mut impl Iterator<Item = OsString>, option: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let value = next_string(arguments, option)?;
    parse_number(option, &value)
}

fn parse_number<T>(option: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn same_roots(indexed: &[PathBuf], requested: &[PathBuf]) -> bool {
    indexed.len() == requested.len()
        && indexed.iter().zip(requested).all(|(left, right)| {
            let left = left.canonicalize().unwrap_or_else(|_| left.clone());
            let right = right.canonicalize().unwrap_or_else(|_| right.clone());
            left == right
        })
}

fn parse_encoding(value: &str) -> Result<EncodingMode, String> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(EncodingMode::Auto),
        "utf-8" | "utf8" => Ok(EncodingMode::Utf8),
        "utf-16le" | "utf16le" => Ok(EncodingMode::Utf16Le),
        "utf-16be" | "utf16be" => Ok(EncodingMode::Utf16Be),
        _ if encoding_rs::Encoding::for_label(value.as_bytes()).is_some() => {
            Ok(EncodingMode::Label(value.to_owned()))
        }
        _ => Err(format!("unknown encoding label {value}")),
    }
}

fn parse_color(value: &str) -> Result<CliColor, String> {
    match value {
        "auto" => Ok(CliColor::Auto),
        "always" => Ok(CliColor::Always),
        "never" => Ok(CliColor::Never),
        _ => Err(format!(
            "invalid color mode {value:?}; expected auto, always, or never"
        )),
    }
}
