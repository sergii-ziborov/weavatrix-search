# Weavatrix Search

`weavatrix-search` is a bounded, deterministic repository content-search
engine for Weavatrix and other Rust applications. It uses
[`weavatrix-scan`](https://crates.io/crates/weavatrix-scan) for safe,
ignore-aware discovery and one-pass content delivery.

The crate does not invoke ripgrep or external processes at runtime. Ordinary
UTF-8 files are searched as borrowed chunks without retaining whole files.
Result memory is bounded independently of repository size, making the same API
appropriate for repositories containing hundreds of thousands of files.

## Status

The `0.1.1` release contract covers:

- ordered literal/regex query sets in one content pass;
- line-streaming and explicitly bounded multiline matching;
- stable line, decoded/source byte-offset, and submatch evidence;
- before/after context;
- UTF-8, UTF-16, BOM detection, and explicit encoding labels;
- bounded ZIP/TAR member search and GZIP, `BZip2`, Zstandard, LZ4 frame,
  raw LZMA, XZ, and Brotli stream search;
- binary, file, line, archive-entry, expansion, cancellation, result, and
  warning limits;
- deterministic output under parallel traversal;
- match, aggregate-count, matched-file, and early-exit result modes;
- a parallel multi-root API and CLI with stable root identity;
- stable JSON Lines plus configurable text headings, colors, line/column
  evidence, only-matching and NUL-path output;
- bounded, non-mutating replacement previews with numbered and named regex
  captures;
- output-parity and throughput benchmarks against ripgrep.

## Example

```rust
use weavatrix_search::{SearchOptions, SearchQuery, Searcher};

let report = Searcher::new(".", SearchQuery::literal("SelectionMatcher"))
    .options(SearchOptions::default().with_context(1, 1))
    .search()?;

for found in report.matches {
    println!("{}:{}: {}", found.path, found.line_number, found.line);
}
# Ok::<(), weavatrix_search::Error>(())
```

Multiple patterns preserve input order for leftmost-first tie breaking and
identify the winning pattern on every span:

```rust,no_run
use weavatrix_search::{ResultMode, SearchOptions, SearchQuery, Searcher};

let query = SearchQuery::any([
    SearchQuery::literal("TODO"),
    SearchQuery::regex(r"FIXME\([^)]+\)"),
]);
let report = Searcher::new(".", query)
    .add_root("../shared")
    .options(SearchOptions::default().with_result_mode(ResultMode::Count))
    .search()?;

println!("{} occurrences", report.occurrences);
# Ok::<(), weavatrix_search::Error>(())
```

Repository selection remains configurable through the re-exported
`ScanOptions`, including ignore sources, override globs, named file types,
depth/filesystem policies, byte/time limits, and cooperative cancellation.

## CLI

The included CLI keeps the library's safe defaults and never writes searched
files:

```text
weavatrix-search -F -C 2 "SelectionMatcher" .
weavatrix-search -e "TODO" -e "FIXME\([^)]+\)" --json .
weavatrix-search -F --heading --column --color auto "needle" ./app ./shared
weavatrix-search --replace '${last}, ${first}' \
  '(?<first>[A-Z][a-z]+) (?<last>[A-Z][a-z]+)' .
```

Core flags cover repeated patterns, literal/regex selection, sensitive,
insensitive and smart case, context, multiline, count/files/quiet modes,
replacement preview, encoding labels, selection globs, hidden paths, archive
control, multiple roots, headings, color, line/column fields, only-matching
records, NUL paths, statistics, and resource limits. Exit status is `0` for a
match, `1` for no match, and `2` for usage, search, or output failure. Search
roots remain native `OsString` paths; patterns and glob programs are UTF-8.

## Execution model

```text
weavatrix-scan
  -> ignore-aware bounded discovery
  -> safe parallel file open
  -> borrowed byte chunks
  -> per-worker single/query-set automata state
  -> streaming lines or bounded multiline decode
  -> context/encoding/archive/replacement evidence
  -> deterministic bounded collector
```

Ordinary UTF-8 files are not copied into whole-file buffers. A whole file is
buffered only for multiline mode, non-UTF-8 decoding, or archive inspection,
and those paths have explicit size bounds. Match records, per-file summaries,
and warnings have separate deterministic limits; non-quiet aggregate match
counts remain complete when records are omitted.

## Package boundary

- **Weavatrix Scan:** paths, ignore rules, safe content delivery, hashes,
  revisions, watcher deltas.
- **Weavatrix Search:** literal/regex sets, line and multiline matching,
  context, encodings, archives, and search-result modes.
- **Weavatrix Vector:** a later independent repository for exact and ANN vector
  candidate search.
- **Weavatrix Clone:** a later independent repository for token and clone
  detection.

## Dependency policy

Repository discovery is exclusively `weavatrix-scan`; `ignore`, `walkdir`,
`jwalk`, `wax`, and ripgrep are not runtime dependencies. Search owns its
literal, line/context, ordering, archive-safety, and evidence pipelines.
Low-level Rust primitives provide regex automata, byte scanning, decoding, and
decompression. Archive features use Rust backends and do not require native
compression libraries, helper executables, or FFI bindings.

## Functional comparison

| Capability | Weavatrix Search | ripgrep | Current status |
| --- | --- | --- | --- |
| Git-ignore-aware parallel repository search | Via Weavatrix Scan | Yes | Covered |
| Literal/regex, smart case, lines, context | Yes | Yes | Covered |
| Deterministic bounded records and warnings | Yes | Output is streamed | Weavatrix-specific |
| UTF-8, UTF-16, explicit encoding labels | Yes | Yes | Covered |
| ZIP/TAR member search without extraction | Yes | Compressed-stream search | Weavatrix-specific |
| Library-first typed evidence API | Yes | Internal crates/CLI | Weavatrix-specific |
| Multiple query patterns in one pass | Yes, with pattern IDs | Yes | Covered |
| Multiline regex | Yes, bounded explicitly | Yes | Covered |
| Count/files/quiet execution | Typed library and CLI modes | CLI modes | Covered |
| Text and JSON Lines output | Stable adapters; color, headings, columns, only-match, NUL | Rich CLI | Covered for common integration modes |
| Non-mutating replacement preview | `$0`, numeric and named captures | Yes | Covered |
| GZIP/BZip2/Zstd/LZ4/LZMA/XZ/Brotli | In-process pure Rust | Helpers may be external | Weavatrix has no process dependency |
| XZ and TAR.XZ safety | Bounded dictionary/output; concatenated streams | Helper-dependent | Covered |
| Multi-root invocation | Parallel API and CLI, stable root IDs | Multiple paths | Covered |

Ripgrep still has more presentation-specialized modes, including passthrough,
vimgrep formatting, byte offsets, and several flag combinations. Weavatrix
Search intentionally does not emulate every CLI flag; its differentiator is a
typed, bounded, deterministic library pipeline that can feed Weavatrix Search,
Graph, Clone, and hosted indexing without spawning another executable.

## Windows end-to-end benchmark

Subsecond uncached search is not a universal filesystem guarantee. Subsecond
warm-cache search is already practical for ordinary large repositories, and
ripgrep can also achieve it. The differentiated result is keeping that speed
inside a bounded, typed Rust pipeline.

The reference run used Windows 11 Enterprise `10.0.26200`, an Intel Core Ultra
7 255U, Rust `1.97.1`, release builds, crates.io `weavatrix-scan 0.4.1`, and
ripgrep `15.2.0`. Both tools were separate CLI processes and emitted JSON.
Before timing, the benchmark asserts identical normalized path, line, and
submatch-span records. Timing includes startup, ignore-aware discovery, content
search, JSON serialization, capture, and normalization.

The fixture uses 500 small Rust files per directory and excludes one
500-file directory through `.gitignore`. Five percent of selected files match.
The 20,000-file row is the median of seven interleaved runs after two warmups;
the 200,000-file row is the median of five after one warmup. The filesystem
cache was warm.

| Corpus | Selected / matching | Weavatrix Search | ripgrep | Outcome |
| --- | ---: | ---: | ---: | --- |
| 20,000 files | 19,500 / 975 | **292.5 ms** | 399.7 ms | Weavatrix 1.37x faster; both below 1 s |
| 200,000 files | 199,500 / 9,975 | **3,859.4 ms** | 6,212.9 ms | Weavatrix 1.61x faster |

This does not claim that every repository or query is faster. Antivirus,
storage, cache state, file size, output volume, encoding, and regex complexity
can dominate. A true cold-cache number requires controlled cache eviction or a
reboot and is deliberately not inferred from warm runs. Repeated subsecond
queries over 200,000+ files require a persistent/live index; that belongs in a
future indexing layer rather than weakening Scan's filesystem evidence.

The native CI parity run uses a generated 5,500-file selected corpus, five
measured runs after one warmup, ripgrep `15.2.0`, and the same normalized output
assertion. Hosted-runner timings are indicative and must only be compared within
the same row:

| GitHub runner | Weavatrix Search literal | ripgrep literal | Outcome |
| --- | ---: | ---: | --- |
| Windows | **96.0 ms** | 165.0 ms | Weavatrix 1.72x faster |
| macOS ARM64 | **27.5 ms** | 49.2 ms | Weavatrix 1.79x faster |
| Ubuntu | 26.8 ms | **20.1 ms** | ripgrep 1.34x faster |

All three native jobs also pass exact-result parity for literal, regex,
query-set, multiline, count, and files-with-matches modes.

To reproduce the end-to-end rows:

```text
cargo build --release --all-features
cargo bench --bench compare_ripgrep -- prepare <fixture-path> 20000
cargo bench --bench compare_ripgrep -- verify <fixture-path> 20000
WEAVATRIX_SEARCH_BENCH_WARMUPS=2 WEAVATRIX_SEARCH_BENCH_RUNS=7 \
  cargo bench --bench compare_ripgrep -- run-cli <fixture-path>
```

Use `200000`, one warmup, and five runs for the scale row. In PowerShell, set
the variables through `$env:WEAVATRIX_SEARCH_BENCH_WARMUPS` and
`$env:WEAVATRIX_SEARCH_BENCH_RUNS`. The separate `run` mode profiles the
in-process API across literal, regex, query-set, multiline, count, and file
workloads while checking output parity.

## Footprint

On the disclosed Windows GNU host, the release CLI measured 8,084,567 bytes
with all archive formats and 6,492,855 bytes with default features disabled.
The installed ripgrep 15.2 binary measured 4,218,880 bytes and included PCRE2.
These are uncompressed executable sizes for those exact builds, not portable
package-size guarantees. Archive support is feature-gated when a smaller
consumer is more important than in-process compressed search. Verified package
size is 25 files, 227.0 KiB unpacked / 50.2 KiB compressed; dependency source
is excluded.

## Safety

The `weavatrix-search` sources forbid unsafe Rust. Archive paths are virtual and
never extracted to disk. Expansion, entry count, entry size, source size, line
size, LZMA/XZ decoder memory, replacement previews, and retained result/warning
counts are bounded. XZ decoding is pure Rust, supports stream concatenation,
and refuses dictionary requests above the configured ceiling. Zstandard
initialization deliberately uses the decoder's checked reset path, which
rejects windows above its 100 MiB ceiling. The implementation is checked for
Windows, Linux, Intel macOS, and Apple Silicon macOS; the minimum supported
Rust version is 1.88.

## License

MIT (c) 2026 Sergii Ziborov.
