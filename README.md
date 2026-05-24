# CSV Reader

A cross-platform desktop application for reading and exporting large CSV files. Built with Electron.

Handles files of any size (tested with 300MB+) by using file-backed byte-offset indexing — the entire file is never loaded into memory.

## Features

- **File-backed indexing** — scans the file once to build a byte-offset index, then reads only visible rows on demand
- **Virtual scrolling** — renders only rows in the viewport, smooth even with millions of rows
- **Large cell support** — cells containing several MB of text are handled efficiently: preview in table, full content in detail panel on click
- **Uniform column widths** — all columns have equal width with a 120px minimum; horizontal scrollbar appears when needed
- **Row/column selection** — click row numbers or column headers to select (Shift for range, Ctrl to toggle)
- **CSV export** — select columns and row range, export to a new CSV file with proper quoting and BOM for Excel compatibility
- **Dark theme** — easy on the eyes for long data sessions

## Prerequisites

[Node.js](https://nodejs.org/) v18 or later.

## Quick Start

```bash
git clone git@github.com:fulracoco/csv_reader.git
cd csv-reader
npm install
npm start
```

## Usage

| Action | How |
|---|---|
| Open file | Click **Open CSV File** or the folder icon |
| Scroll | Mouse wheel / drag scrollbar (virtual scrolling, only visible rows loaded) |
| View cell content | Click any cell to open the detail panel on the right |
| Copy cell | Select cell, press `Ctrl+C`, or click **Copy** in the detail panel |
| Adjust row height | Use the dropdown in the toolbar (28px–100px) |
| Select rows | Click row numbers (Shift/Ctrl for multi-select) |
| Select columns | Click column headers (Shift/Ctrl for multi-select) |
| Export CSV | Click **Export** → choose columns + row range → save |

## Architecture

| File | Purpose |
|---|---|
| `main.js` | Electron main process: `CsvEngine` class (byte-offset index, batch reads, LRU cache, export streaming), IPC handlers |
| `preload.js` | Context bridge — safe API exposed to renderer |
| `index.html` | UI layout: welcome screen, toolbar, virtual table, detail panel, export modal |
| `styles.css` | Dark theme, table styling, modal, scrollbar customization |
| `renderer.js` | Virtual scrolling engine, DOM pool recycling, cell interaction, export dialog |

### How It Handles Large Files

1. **One-pass indexing** — reads the file byte-by-byte, records row start positions. For 200K rows this takes ~100ms and uses ~1.5MB of memory.
2. **On-demand reading** — only rows visible in the viewport are read from disk (~30 rows).
3. **Batch I/O** — consecutive uncached rows are read in a single disk operation.
4. **LRU cache** — 3,000 most recently accessed rows kept in memory.
5. **Truncated IPC** — the main process sends only the first 500 characters of each cell to the renderer. For a file with ~800KB cells, this reduces IPC transfer from 24MB to 30KB per viewport (99.9% reduction). Full content is loaded on demand when a cell is clicked.
6. **Streaming export** — writes directly to the output file row by row, never holds the full dataset in memory.

## Keyboard Shortcuts

| Key | Action |
|---|---|
| `Escape` | Close detail panel / clear selection |
| `Ctrl+C` | Copy selected cell content |

## License

MIT
