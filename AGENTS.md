# Repository Guidelines

## Project Structure & Module Organization

This repository is a Tauri 2 desktop application. The static UI lives in `frontend/`: `index.html` defines the layout, `styles.css` owns presentation, and `renderer.js` handles virtual scrolling, selection, search, editing, export, and Tauri IPC. Rust code is under `src-tauri/src/`; keep CSV parsing and memory-mapped access in `csv_engine.rs`, IPC/menu handlers in `commands.rs`, and application setup in `lib.rs`. Tauri configuration, capabilities, and platform icons also live under `src-tauri/`. CI release builds are defined in `.github/workflows/build.yml`; icon generation is in `scripts/`.

Large local `*.csv` fixtures, `dist/`, `node_modules/`, and `src-tauri/target/` are ignored. Do not commit generated bundles or test datasets.

## Build, Test, and Development Commands

- `npm install`: install the Tauri CLI and JavaScript dependencies.
- `npm run dev`: launch the desktop app with the local frontend.
- `npm run build`: create a release bundle for the current platform.
- `npm run build:win`, `build:mac-x64`, `build:mac-arm64`, or `build:linux`: build a specific target.
- `cargo test --manifest-path src-tauri/Cargo.toml`: compile and run Rust tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: verify Rust formatting.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: catch Rust issues before review.

## Coding Style & Naming Conventions

Use standard `rustfmt` output and four-space indentation in Rust. Follow Rust conventions: `snake_case` functions/modules, `PascalCase` types, and `SCREAMING_SNAKE_CASE` constants. Frontend files use two-space indentation, `camelCase` JavaScript identifiers, single-quoted strings, and kebab-case DOM IDs. Keep IPC command names aligned between Rust and `frontend/renderer.js`. Preserve the existing dependency-free frontend unless a new dependency has a clear benefit.

## Testing Guidelines

There is no established automated test suite yet. Add focused `#[cfg(test)]` unit tests beside Rust parsing/indexing code, with descriptive names such as `parses_quoted_delimiter`. Run `cargo test` and manually exercise open, search, virtual scrolling, cell editing, and export. Include empty, quoted, wide, Unicode, and large CSV inputs where relevant.

## Commit & Pull Request Guidelines

History uses short, imperative summaries in English or Chinese, often prefixed with `Add`, `Fix`, `Optimize`, or `Bump`. Keep each commit focused and describe the observable change. Pull requests should explain behavior and performance impact, link related issues, list verification commands and platforms, and include screenshots for UI changes. Note any changes to packaging, capabilities, or WebView2 behavior explicitly.
