# Changelog

All notable changes to this project are documented here.

## Unreleased

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
