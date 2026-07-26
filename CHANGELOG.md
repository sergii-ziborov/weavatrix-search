# Changelog

All notable changes to this project are documented here.

## Unreleased

## 0.3.0 - 2026-07-26

- Add `SourceFileEvidence` with root/path identity, source bytes, logical lines,
  matching lines, occurrences, encoding, lossy-decode, and archive evidence.
- Add deterministic `Matched`/`All` evidence retention plus a concurrent,
  zero-retention visitor for Graph and hosted indexing consumers.
- Release evidence visitor state before returning even when the shared Scan
  worker pool retains an idle job closure.
- Compute ordinary-file line and byte metrics in the existing content pass
  without a second filesystem read or whole-file allocation.
- Preserve source evidence across multi-root, UTF-16, multiline, archive, and
  persistent-index search paths.
- Select constant-memory overlapped discovery for broad Windows/filesystem
  roots while retaining low-latency buffered discovery for repositories and
  ordinary Unix directories.
- Add CLI `--discovery` and `--content-workers` controls for reproducible
  traversal tuning.
- Extend exact-parity benchmarks with streaming and retained evidence profiles.

## 0.2.0 - 2026-07-26

- Add checksummed, revisioned persistent multi-root content indexes with atomic
  replacement, platform-safe root codecs, and explicit resource limits.
- Add conservative 512-bit per-file trigram Bloom filters with exact
  verification through the normal literal/regex/encoding/archive pipeline.
- Add bounded parallel index build/query APIs and ergonomic
  `PersistentIndex::builder`.
- Add native watcher-maintained `LiveIndex`, changed-path updates, debouncing,
  bounded event queues, overflow rebuilds, generation/dirty/error health, and
  clean-stop persistence.
- Prevent partial mutation after failed incremental limits, correctly report
  add/update/remove deltas, and avoid full-entry sorting for one-file updates.
- Preserve lexical watcher roots beside canonical identity roots so deleted
  Windows short/verbatim paths remain attributable.
- Exclude indexes and atomic-write artifacts stored below a watched root to
  prevent self-indexing and feedback loops.
- Add CLI index build/reuse/rebuild/status/worker controls and index evidence
  to text statistics and JSON summaries.
- Add exact-parity persistent/live benchmarks at 20,000 and 200,000 files.

## 0.1.1 - 2026-07-26

- Publish native Windows, macOS ARM64, and Ubuntu ripgrep parity medians.
- Install the pinned ripgrep `15.2.0` competitor in benchmark CI.
- Exclude CI configuration from the crates.io package.

## 0.1.0 - 2026-07-26

- Establish bounded literal and regular-expression repository search on top of
  the `weavatrix-scan` one-pass content pipeline.
- Add ordered multi-pattern query sets with pattern-aware spans.
- Add bounded multiline matching with merged physical-line evidence.
- Add allocation-light count, matched-file, and quiet result modes.
- Add bounded per-file summaries to count mode for stable CLI output.
- Add deterministic result limiting, line context, encoding policy, and safe
  archive expansion limits.
- Add a stable text/JSON Lines output API and dependency-light CLI with
  grep-style exit status.
- Add non-mutating replacement previews with `$0`, numeric, and named capture
  expansion under an independent byte limit.
- Add in-process pure-Rust BZip2, Zstandard, LZ4 frame, raw LZMA, and Brotli
  decoding, including TAR combinations, without native libraries or helper
  processes.
- Add bounded pure-Rust XZ and TAR.XZ decoding, including concatenated streams
  and an explicit dictionary-memory ceiling.
- Add parallel multi-root Searcher/CLI execution with stable root identity.
- Add configurable headings, ANSI color, line/column fields, only-matching,
  NUL-path, and statistics output.
- Benchmark release CLIs against ripgrep with exact JSON evidence parity,
  including 20,000- and 200,000-file Windows profiles.
- Require `weavatrix-scan 0.4.1`, which removes a redundant Windows hidden-file
  metadata query.
- Bound retained warnings independently from results and report omitted warning
  counts.
- Distinguish decoded offsets from exact source-byte offsets after transcoding.
- Add output-equivalent ripgrep parity and performance benchmarks.
- Preserve native non-UTF8 search-root arguments in the CLI.
