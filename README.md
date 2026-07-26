# Weavatrix Search

`weavatrix-search` is a bounded, deterministic repository content-search
engine for Weavatrix and other Rust applications. It uses
[`weavatrix-scan`](https://crates.io/crates/weavatrix-scan) for safe,
ignore-aware discovery and one-pass content delivery.

The crate does not invoke ripgrep or external processes at runtime. Ordinary
UTF-8 files are searched as borrowed chunks without retaining whole files.
Filesystem-search result memory is bounded independently of repository size.
For repeated queries, an optional persistent/live index keeps an exact,
revisioned content snapshot resident and uses conservative trigram Bloom
filters only to reject impossible candidates; every candidate is still
verified by the normal search engine.

## Status

The `0.3.0` release contract covers:

- adaptive discovery: low-latency buffered traversal for repository roots and
  ordinary Unix directories, plus overlapped constant-memory streaming for
  broad Windows roots and filesystem roots, with the same Scan selection and
  opened-handle validation contract;
- one compiled literal finder per query and allocation-free common UTF-8
  encoding evidence until a match is retained;
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
- optional `SourceFileEvidence` with source bytes, logical lines, matching
  lines, occurrences, encoding, and stable root/path identity, computed in the
  same content pass;
- deterministic bounded evidence retention or a concurrent zero-retention
  callback for Graph and hosted indexing;
- match, aggregate-count, matched-file, and early-exit result modes;
- a parallel multi-root API and CLI with stable root identity;
- stable JSON Lines plus configurable text headings, colors, line/column
  evidence, only-matching and NUL-path output;
- bounded, non-mutating replacement previews with numbered and named regex
  captures;
- checksummed, platform-tagged, atomically replaced persistent multi-root
  indexes with explicit entry/content/file-size limits;
- bounded parallel index builds and queries, exact content hashes,
  deterministic revisions, and no-false-negative literal/regex prefiltering;
- native live updates with debouncing, bounded event queues, overflow-triggered
  rebuilds, changed-path scans, observable dirty/generation/error state, and
  clean-stop persistence;
- safe handling for deleted Windows short/verbatim paths and for indexes stored
  inside a watched repository, without self-indexing or watcher feedback loops;
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

Search can emit file size and line-count evidence for Graph without rereading
source files or retaining one record per file:

```rust,no_run
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_search::{SearchOptions, SearchQuery, Searcher};

let lines = Arc::new(AtomicU64::new(0));
let sink = Arc::clone(&lines);
let options = SearchOptions::default().with_file_evidence_visitor(move |file| {
    sink.fetch_add(file.total_lines, Ordering::Relaxed);
});
let report = Searcher::new(".", SearchQuery::literal("SelectionMatcher"))
    .options(options)
    .search()?;

println!(
    "{} files, {} bytes, {} lines",
    report.files_searched,
    report.bytes_searched,
    lines.load(Ordering::Relaxed)
);
# Ok::<(), weavatrix_search::Error>(())
```

The callback may run concurrently in completion order. Consumers that require
deterministic root/path order can retain `FileEvidenceMode::Matched` or
`FileEvidenceMode::All`, bounded independently by `max_file_evidence`.

Repeated queries use a discoverable builder and the same `SearchQuery` /
`SearchOptions` contract:

```rust,no_run
use weavatrix_search::{PersistentIndex, SearchOptions, SearchQuery};

let (index, build) = PersistentIndex::builder(".")
    .build_and_save(".weavatrix/search.wvx")?;
let report = index.search(
    SearchQuery::literal("SelectionMatcher"),
    SearchOptions::default(),
)?;

println!(
    "{} indexed files, {} candidates",
    build.files,
    report.index.as_ref().unwrap().candidate_files
);
# Ok::<(), weavatrix_search::Error>(())
```

The default `live` feature adds native watchers. RAM updates are immediately
queryable; clean shutdown persists one dirty snapshot. Consumers that prefer
write-through durability can opt into `with_persist_each_batch(true)`.

```rust,no_run
# #[cfg(feature = "live")]
# fn main() -> Result<(), weavatrix_search::Error> {
use weavatrix_search::{LiveIndex, SearchOptions, SearchQuery};

let live = LiveIndex::builder(".weavatrix/search.wvx", ".").start()?;
let report = live.search(
    SearchQuery::regex(r"TODO|FIXME"),
    SearchOptions::default(),
)?;
println!("generation={}", live.status().generation);
live.stop()?;
# Ok(())
# }
# #[cfg(not(feature = "live"))]
# fn main() {}
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
weavatrix-search -F --index .weavatrix/search.wvx "needle" .
weavatrix-search --index-status .weavatrix/search.wvx
weavatrix-search --replace '${last}, ${first}' \
  '(?<first>[A-Z][a-z]+) (?<last>[A-Z][a-z]+)' .
```

Core flags cover repeated patterns, literal/regex selection, sensitive,
insensitive and smart case, context, multiline, count/files/quiet modes,
replacement preview, encoding labels, selection globs, hidden paths, archive
control, adaptive/streaming/buffered discovery, bounded content workers,
multiple roots, headings, color, line/column fields, only-matching records,
NUL paths, statistics, and resource limits. Exit status is `0` for a match,
`1` for no match, and `2` for usage, search, or output failure. Search roots
remain native `OsString` paths; patterns and glob programs are UTF-8.
`--index PATH` builds a missing index or reuses a validated snapshot;
`--rebuild-index` refreshes it explicitly, `--index-workers` bounds build/query
parallelism, and `--index-status` prints revision/file/byte/root evidence.
Resident watcher maintenance is a library API so Weavatrix and hosted services
can own process lifetime, cancellation, and health reporting.

## Execution model

```text
weavatrix-scan
  -> adaptive ignore-aware discovery
       repository/ordinary Unix root -> buffered parallel traversal
       Windows broad/filesystem root  -> constant-memory streaming
  -> safe parallel file open
  -> borrowed byte chunks
  -> per-worker single/query-set automata state
  -> streaming lines or bounded multiline decode
  -> context/encoding/archive/replacement/file evidence
  -> deterministic bounded collector
       or concurrent zero-retention evidence callback

persistent/live mode
  -> one exact content snapshot + hashes + revision
  -> 512-bit per-file trigram Bloom candidate rejection
  -> bounded parallel verification by the same search engine
  -> watcher plan -> changed paths only -> atomic RAM generation
  -> optional write-through, otherwise one durable clean-stop snapshot
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
- **Weavatrix Search Vector:** a later independent
  `weavatrix-search-vector` repository for exact and ANN vector candidate
  search; Weavatrix Semantic may use it, but it does not depend on Semantic.
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
| Per-source bytes, logical lines, encoding, match totals | Same-pass typed evidence; retained or streamed | No public typed CLI contract | Weavatrix-specific |
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
| Persistent repeated-query index | Exact snapshot, checksum, hashes, revision, bounded candidate verification | No resident index in the `rg` CLI | Weavatrix-specific |
| Native live maintenance | Debounced watcher deltas, overflow rebuild, health/generation state | Rerun traversal | Weavatrix-specific |
| Changed-file content update | No full discovery for safe file events | New `rg` invocation traverses selection | Weavatrix-specific |

Ripgrep still has more presentation-specialized modes, including passthrough,
vimgrep formatting, byte offsets, and several flag combinations. Weavatrix
Search intentionally does not emulate every CLI flag; its differentiator is a
typed, bounded, deterministic library pipeline that can feed Weavatrix Search,
Graph, Clone, and hosted indexing without spawning another executable.

## Windows end-to-end benchmark

Subsecond uncached search is not a universal filesystem guarantee. A warm
ordinary traversal can stay below one second on smaller repositories or faster
filesystems, and ripgrep can also achieve it. At 200k files on this Windows
host, stable subsecond repeated search comes from the persistent/live index,
while the ordinary filesystem path remains the no-index fallback.

The reference run used Windows 11 Enterprise `10.0.26200`, an Intel Core Ultra
7 255U, Rust `1.97.1`, release builds, crates.io `weavatrix-scan 0.4.2`, and
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
| 200,000 files | 199,500 / 9,975 | **3,181.4 ms** | 7,507.9 ms | Weavatrix 2.36x faster |

This does not claim that every repository or query is faster. Antivirus,
storage, cache state, file size, output volume, encoding, and regex complexity
can dominate. A true cold-cache number requires controlled cache eviction or a
reboot and is deliberately not inferred from warm runs.

The refreshed `0.3.0` 200k row uses adaptive broad-root streaming with 32
bounded readers. The first uncontrolled cold touch on the same fixture took
about 30 seconds while later passes took 3–5 seconds, demonstrating why a
single cold observation is not mixed into the warm-cache median.

File evidence does not introduce a second read. On a separate 5,500-file
Windows fixture, five measured runs after one warmup produced 92.5 ms for the
default literal search, 91.4 ms with the zero-retention evidence callback, and
96.9 ms while retaining all evidence; the paired ripgrep literal run was
155.4 ms. These modes searched identical content and logical-line totals.

### Persistent/live index benchmark

The persistent index closes the repeated-query gap without weakening Scan's
filesystem evidence. The benchmark first builds and atomically saves an exact
snapshot, opens it with full format/checksum/revision validation, asserts exact
normalized path/line/span parity against ripgrep, then times resident queries.
The literal has 975 matches at 20k and 9,975 at 200k; in this synthetic corpus
the Bloom filter admitted exactly those matching files.

The 20k row is the published `0.2.0` reference and uses seven measured
interleaved runs after two warmups. The refreshed 200k `0.3.0` row uses five
resident queries and updates after one warmup on the same disclosed Windows
host. It includes a fresh ripgrep parity/timing pass on the same corpus.

| Corpus | Serialized index | Build | Validated open | Resident query | One-file live update | ripgrep process |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 20,000 files (19,502 indexed) | 3.21 MB | 578.5 ms | 55.6 ms | **15.7 ms** | **9.3 ms** | 785.1 ms |
| 200,000 files (199,502 indexed) | 33.07 MB | 3,031.5 ms | **492.3 ms** | **24.4 ms** | **15.9 ms** | 4,927.2 ms |

Thus even a fresh validated open plus query remained about 517 ms at 200k;
an already resident query was about 24.4 ms. This is a scenario distinction,
not a claim that the literal engine is universally 200x faster: the 20k
resident result avoids process startup and repository traversal by design,
while the ripgrep CLI repeats both. Index RAM intentionally retains exact
source bytes plus paths, hashes, and Bloom metadata; use filesystem streaming
when that resident-memory tradeoff is not appropriate.

The native CI parity run uses a generated 5,500-file selected corpus, five
measured runs after one warmup, ripgrep `15.2.0`, and the same normalized output
assertion. Hosted-runner timings are indicative and must only be compared within
the same row:

| GitHub runner | Weavatrix Search literal | ripgrep literal | Outcome |
| --- | ---: | ---: | --- |
| Windows | **83.3 ms** | 176.5 ms | Weavatrix 2.12x faster |
| macOS ARM64 | **49.0 ms** | 82.2 ms | Weavatrix 1.68x faster |
| Ubuntu | **15.8 ms** | 16.2 ms | Weavatrix 1.03x faster |

All three native jobs also pass exact-result parity for literal, regex,
query-set, multiline, count, and files-with-matches modes.

The Ubuntu job also runs the exact literal profile at 200k scale:

| GitHub runner | Selected / matching | Weavatrix Search | ripgrep | Outcome |
| --- | ---: | ---: | ---: | --- |
| Ubuntu, 200,000 files | 199,500 / 9,975 | 606.2 ms | **448.5 ms** | Both below 1 s; ripgrep 1.35x faster |

This scale row is the median of three interleaved runs after one warmup. It
shows that buffered-parallel discovery closes the small-repository Linux gap
and reaches the subsecond 200k target, but does not claim an ordinary
filesystem win over ripgrep at that scale.

The same `0.2.1` jobs build a separate 6,000-file corpus (5,502 indexed
including control files), re-check exact resident/ripgrep output parity, and
report:

| GitHub runner | Index build | Validated open | Resident query | One-file update | ripgrep process |
| --- | ---: | ---: | ---: | ---: | ---: |
| Windows | 253.2 ms | 7.8 ms | **0.888 ms** | **0.810 ms** | 133.4 ms |
| macOS ARM64 | 59.7 ms | 11.8 ms | **0.389 ms** | **1.316 ms** | 51.1 ms |
| Ubuntu | 34.8 ms | 3.8 ms | **0.336 ms** | **0.359 ms** | 16.5 ms |

These hosted-runner resident figures use five measured runs after one warmup.
They demonstrate the traversal/startup avoided by the index, not a universal
per-byte literal-engine speedup.

To reproduce the end-to-end rows:

```text
cargo build --release --all-features
cargo bench --bench compare_ripgrep -- prepare <fixture-path> 20000
cargo bench --bench compare_ripgrep -- verify <fixture-path> 20000
WEAVATRIX_SEARCH_BENCH_WARMUPS=2 WEAVATRIX_SEARCH_BENCH_RUNS=7 \
  cargo bench --bench compare_ripgrep -- run-literal <fixture-path>
WEAVATRIX_SEARCH_BENCH_WARMUPS=2 WEAVATRIX_SEARCH_BENCH_RUNS=7 \
  cargo bench --bench compare_ripgrep -- run-cli <fixture-path>
WEAVATRIX_SEARCH_BENCH_WARMUPS=2 WEAVATRIX_SEARCH_BENCH_RUNS=7 \
  cargo bench --bench compare_ripgrep -- run-index <fixture-path>
```

Use `200000`, one warmup, and five runs for the scale row. In PowerShell, set
the variables through `$env:WEAVATRIX_SEARCH_BENCH_WARMUPS` and
`$env:WEAVATRIX_SEARCH_BENCH_RUNS`. Set
`WEAVATRIX_SEARCH_BENCH_DISCOVERY=streaming` and
`WEAVATRIX_SEARCH_BENCH_THREADS=32` to reproduce the Windows broad-root API
profile; `run-cli` uses the binary's adaptive policy directly. The separate
`run` mode profiles the in-process API across literal, regex, query-set,
multiline, count, file, and per-source-evidence workloads while checking
output parity.

## Footprint

On the disclosed Windows GNU host, the `0.2.0` release CLI measured 9,238,795
bytes with archive and live-index support and 7,214,389 bytes with default
features disabled.
The installed ripgrep 15.2 binary measured 4,218,880 bytes and included PCRE2.
These are uncompressed executable sizes for those exact builds, not portable
package-size guarantees. Archive support is feature-gated when a smaller
consumer is more important than in-process compressed search. Verified `0.2.0`
package size is 28 files, 359.0 KiB unpacked / 77.5 KiB compressed; dependency
source is excluded.

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

Persistent indexes have explicit entry, content, serialized-size, path, and
parallelism limits. Loads validate magic, format version, platform path codec,
root/entry bounds, ordering, uniqueness, content revision, whole-file SHA-256,
and trailing bytes before exposing a snapshot. Writes use a lock, a unique
temporary file, `sync_all`, and atomic replacement with rollback. Watcher queue
overflow, directory/ignore changes, and lost events conservatively trigger a
full rebuild; a `.wvx` inside a watched root and its atomic-write artifacts are
excluded from both discovery and watcher feedback.

## License

MIT (c) 2026 Sergii Ziborov.
