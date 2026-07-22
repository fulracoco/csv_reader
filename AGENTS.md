# Repository Guidelines

## Project Structure & Module Organization

This repository is a native Rust desktop application built with `eframe` and `egui`. `src/main.rs` is the executable entry point, `src/app.rs` owns the window, virtual table, interaction, search, editing, export, and settings UI, and `src/csv_engine.rs` owns CSV parsing, memory-mapped access, indexing, caching, encoding detection, and file mutation. Application icons live in `icons/`. CI release builds are defined in `.github/workflows/build.yml`; icon generation helpers are in `scripts/`.

Large local `*.csv` fixtures, `dist/`, `node_modules/`, `target/`, and `gen/` are ignored. Do not commit generated bundles, build outputs, or test datasets.

## Build, Test, and Development Commands

- `cargo run`: launch the native desktop application in development mode.
- `cargo build --release`: build an optimized native binary for the current platform.
- `cargo packager --release`: package an already-built release binary for the current platform.
- `cargo test`: compile and run Rust tests.
- `cargo fmt -- --check`: verify Rust formatting.
- `cargo clippy --all-targets -- -D warnings`: catch Rust issues before review.

## Coding Style & Naming Conventions

Use standard `rustfmt` output and four-space indentation. Follow Rust conventions: `snake_case` functions/modules, `PascalCase` types, and `SCREAMING_SNAKE_CASE` constants. Keep rendering and interaction logic in `app.rs`; keep parsing and file access independent of `egui` in `csv_engine.rs`. Avoid blocking the UI thread with whole-file work, and preserve the virtualized rendering approach for large datasets.

## Testing Guidelines

Add focused `#[cfg(test)]` unit tests beside Rust parsing, indexing, and UI helper code, with descriptive names such as `parses_quoted_delimiter`. Run `cargo test` and manually exercise open, search, virtual scrolling, cell editing, and export. Include empty, quoted, wide, Unicode, legacy-encoded, and large CSV inputs where relevant.

## Commit & Pull Request Guidelines

History uses short, imperative summaries in English or Chinese, often prefixed with `Add`, `Fix`, `Optimize`, or `Bump`. Keep each commit focused and describe the observable change. Pull requests should explain behavior and performance impact, link related issues, list verification commands and platforms, and include screenshots for UI changes. Note any changes to packaging or native window behavior explicitly.
