use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};

// ─── Constants ───────────────────────────────────────────────────────────────

const MAX_CELL_PREVIEW: usize = 500;
const CACHE_MAX: usize = 500;

// ─── NAPI Object Types ───────────────────────────────────────────────────────

#[napi(object)]
pub struct OpenResult {
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub row_count: u32,
    pub column_count: u32,
    pub headers: Vec<String>,
}

#[napi(object)]
pub struct RowData {
    pub cells: Vec<String>,
    pub lengths: Vec<u32>,
}

// ─── Internal State (not exposed to JS) ──────────────────────────────────────

struct Inner {
    mmap: Option<memmap2::Mmap>,
    cache: HashMap<u32, Vec<String>>,
    cache_keys: Vec<u32>,
}

impl Inner {
    fn new() -> Self {
        Inner {
            mmap: None,
            cache: HashMap::with_capacity(CACHE_MAX),
            cache_keys: Vec::with_capacity(CACHE_MAX),
        }
    }

    fn clear(&mut self) {
        self.mmap = None;
        self.cache.clear();
        self.cache_keys.clear();
    }

    fn touch_cache(&mut self, key: u32) {
        if let Some(pos) = self.cache_keys.iter().position(|&k| k == key) {
            self.cache_keys.remove(pos);
        }
        self.cache_keys.push(key);
    }

    fn add_to_cache(&mut self, key: u32, data: Vec<String>) {
        self.cache.insert(key, data);
        if let Some(pos) = self.cache_keys.iter().position(|&k| k == key) {
            self.cache_keys.remove(pos);
        }
        self.cache_keys.push(key);
        while self.cache_keys.len() > CACHE_MAX {
            if let Some(oldest) = self.cache_keys.first().copied() {
                self.cache_keys.remove(0);
                self.cache.remove(&oldest);
            }
        }
    }
}

thread_local! {
    static INNER: RefCell<Inner> = RefCell::new(Inner::new());
}

// ─── CsvEngine (JS-visible class) ───────────────────────────────────────────

#[napi]
pub struct CsvEngine {
    file_path: String,
    file_size: i64,
    offsets: Vec<i64>,
    headers: Vec<String>,
    delimiter: String,
    encoding: String,
    bom_offset: i64,
}

#[napi]
impl CsvEngine {
    #[napi(constructor)]
    pub fn new() -> Self {
        CsvEngine {
            file_path: String::new(),
            file_size: 0,
            offsets: Vec::new(),
            headers: Vec::new(),
            delimiter: String::from(","),
            encoding: String::from("utf8"),
            bom_offset: 0,
        }
    }

    /// Open a CSV file: detect encoding, build the line-offset index, parse header.
    #[napi]
    pub fn open(&mut self, file_path: String) -> Result<OpenResult> {
        INNER.with(|inner| inner.borrow_mut().clear());

        let file = File::open(&file_path)
            .map_err(|e| Error::from_reason(format!("Cannot open file: {}", e)))?;

        let mmap = unsafe {
            memmap2::Mmap::map(&file)
                .map_err(|e| Error::from_reason(format!("Cannot mmap file: {}", e)))?
        };

        let file_size = mmap.len() as i64;
        let (encoding, bom_offset) = detect_bom(&mmap);

        let offsets = build_index(&mmap, bom_offset, &encoding);

        if offsets.is_empty() {
            self.file_path = file_path;
            self.file_size = file_size;
            self.offsets = Vec::new();
            self.headers = Vec::new();
            self.delimiter = String::from(",");
            self.encoding = encoding;
            self.bom_offset = bom_offset;
            INNER.with(|inner| inner.borrow_mut().mmap = Some(mmap));
            return Ok(OpenResult {
                file_path: self.file_path.clone(),
                file_name: std::path::Path::new(&self.file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                file_size,
                row_count: 0,
                column_count: 0,
                headers: Vec::new(),
            });
        }

        let header_text = read_row_text(&mmap, &offsets, 0, file_size, &encoding);
        let delimiter = detect_delimiter(&header_text);
        let headers = parse_csv_line(&header_text, delimiter);

        let row_count = offsets.len().saturating_sub(1) as u32;
        let column_count = headers.len() as u32;

        self.file_path = file_path;
        self.file_size = file_size;
        self.offsets = offsets;
        self.headers = headers.clone();
        self.delimiter = String::from(delimiter);
        self.encoding = encoding;
        self.bom_offset = bom_offset;
        INNER.with(|inner| inner.borrow_mut().mmap = Some(mmap));

        Ok(OpenResult {
            file_path: self.file_path.clone(),
            file_name: std::path::Path::new(&self.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            file_size,
            row_count,
            column_count,
            headers,
        })
    }

    /// Read a contiguous range of data rows (0-indexed from first data row).
    #[napi]
    pub fn get_rows(&self, start_row: u32, count: u32) -> Result<Vec<RowData>> {
        let delimiter = self.delimiter.chars().next().unwrap_or(',');

        INNER.with(|cell| {
            // Phase 1: collect data with immutable borrow
            let rows: Vec<(u32, RowData, Option<Vec<String>>)>;
            {
                let inner = cell.borrow();
                let mmap = inner.mmap.as_ref()
                    .ok_or_else(|| Error::from_reason("No file open"))?;
                let mut temp = Vec::with_capacity(count as usize);

                for i in 0..count {
                    let row_idx = start_row + i + 1;
                    if row_idx as usize >= self.offsets.len() {
                        break;
                    }

                    if let Some(cached) = inner.cache.get(&row_idx) {
                        let cells: Vec<String> = cached
                            .iter()
                            .map(|c| truncate_cell(c, MAX_CELL_PREVIEW))
                            .collect();
                        let lengths: Vec<u32> = cached.iter().map(|c| c.len() as u32).collect();
                        temp.push((row_idx, RowData { cells, lengths }, None));
                    } else {
                        let text = read_row_text(mmap, &self.offsets, row_idx as usize, self.file_size, &self.encoding);
                        let parsed = parse_csv_line(&text, delimiter);
                        let cells: Vec<String> = parsed
                            .iter()
                            .map(|c| truncate_cell(c, MAX_CELL_PREVIEW))
                            .collect();
                        let lengths: Vec<u32> = parsed.iter().map(|c| c.len() as u32).collect();
                        temp.push((row_idx, RowData { cells, lengths }, Some(parsed)));
                    }
                }
                rows = temp;
            }

            // Phase 2: update cache with mutable borrow
            let mut inner = cell.borrow_mut();
            for (row_idx, _, parsed_opt) in &rows {
                match parsed_opt {
                    Some(data) => inner.add_to_cache(*row_idx, data.clone()),
                    None => inner.touch_cache(*row_idx),
                }
            }

            Ok(rows.into_iter().map(|(_, rd, _)| rd).collect())
        })
    }

    /// Read specific rows by their data indices (0-indexed). Returns full cell content.
    #[napi]
    pub fn get_rows_by_index(&self, indices: Vec<u32>) -> Result<Vec<Vec<String>>> {
        let delimiter = self.delimiter.chars().next().unwrap_or(',');

        if indices.is_empty() {
            return Ok(Vec::new());
        }

        INNER.with(|cell| {
            let mut sorted: Vec<(usize, u32)> = indices.iter().enumerate().map(|(i, &r)| (i, r)).collect();
            sorted.sort_by_key(|&(_, r)| r);

            let mut results: Vec<Option<Vec<String>>> = vec![None; indices.len()];
            // Track cache operations: (offset_idx, Option<parsed_data>)
            let mut cache_ops: Vec<(u32, Option<Vec<String>>)> = Vec::new();

            {
                let inner = cell.borrow();
                let mmap = inner.mmap.as_ref()
                    .ok_or_else(|| Error::from_reason("No file open"))?;

                let mut range_start = 0;
                while range_start < sorted.len() {
                    let mut range_end = range_start;
                    while range_end + 1 < sorted.len()
                        && sorted[range_end + 1].1 == sorted[range_end].1 + 1
                    {
                        range_end += 1;
                    }

                    for k in range_start..=range_end {
                        let (orig_idx, data_row) = sorted[k];
                        let offset_idx = data_row + 1;
                        if (offset_idx as usize) >= self.offsets.len() {
                            continue;
                        }

                        if let Some(cached) = inner.cache.get(&offset_idx) {
                            cache_ops.push((offset_idx, None));
                            results[orig_idx] = Some(cached.clone());
                        } else {
                            let text = read_row_text(mmap, &self.offsets, offset_idx as usize, self.file_size, &self.encoding);
                            let parsed = parse_csv_line(&text, delimiter);
                            cache_ops.push((offset_idx, Some(parsed.clone())));
                            results[orig_idx] = Some(parsed);
                        }
                    }

                    range_start = range_end + 1;
                }
            }

            // Apply cache updates
            let mut inner = cell.borrow_mut();
            for (row_idx, parsed_opt) in cache_ops {
                match parsed_opt {
                    Some(data) => inner.add_to_cache(row_idx, data),
                    None => inner.touch_cache(row_idx),
                }
            }

            Ok(results.into_iter().map(|r| r.unwrap_or_default()).collect())
        })
    }

    /// Get the full content of a single cell.
    #[napi]
    pub fn get_cell_content(&self, row_index: u32, col_index: u32) -> Result<String> {
        let delimiter = self.delimiter.chars().next().unwrap_or(',');

        INNER.with(|cell| {
            let inner = cell.borrow();
            let mmap = inner.mmap.as_ref()
                .ok_or_else(|| Error::from_reason("No file open"))?;

            let data_row = row_index + 1;
            if (data_row as usize) >= self.offsets.len() {
                return Ok(String::new());
            }

            let text = read_row_text(mmap, &self.offsets, data_row as usize, self.file_size, &self.encoding);
            let parsed = parse_csv_line(&text, delimiter);
            Ok(parsed.get(col_index as usize).cloned().unwrap_or_default())
        })
    }

    /// Update a single cell. Rewrites the entire file and re-indexes.
    #[napi]
    pub fn update_cell(&mut self, row_index: u32, col_index: u32, new_content: String) -> Result<()> {
        let delimiter = self.delimiter.chars().next().unwrap_or(',');
        let data_row = (row_index + 1) as usize;

        if data_row >= self.offsets.len() {
            return Err(Error::from_reason("Row out of range"));
        }

        let row_count = self.offsets.len().saturating_sub(1);

        // Read all row texts (need mmap access)
        let lines = INNER.with(|cell| {
            let inner = cell.borrow();
            let mmap = inner.mmap.as_ref()
                .ok_or_else(|| Error::from_reason("No file open"))?;

            let mut lines: Vec<String> = Vec::with_capacity(row_count + 1);
            for i in 0..=row_count {
                let text = read_row_text(mmap, &self.offsets, i, self.file_size, &self.encoding);
                let text = text.trim_end_matches(|c: char| c == '\r' || c == '\n').to_string();
                lines.push(text);
            }
            Ok::<_, napi::Error>(lines)
        })?;

        // Modify the target line
        let mut parsed = parse_csv_line(&lines[data_row], delimiter);
        while parsed.len() <= col_index as usize {
            parsed.push(String::new());
        }
        parsed[col_index as usize] = new_content;

        let mut lines = lines;
        lines[data_row] = format_csv_row(&parsed, delimiter);

        // Drop mmap before writing to file
        INNER.with(|cell| cell.borrow_mut().mmap = None);

        // Rewrite the entire file
        {
            let mut file = BufWriter::new(
                File::create(&self.file_path)
                    .map_err(|e| Error::from_reason(format!("Cannot write file: {}", e)))?,
            );

            // Write BOM
            if self.encoding == "utf16le" {
                file.write_all(&[0xFF, 0xFE])
                    .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
            } else {
                file.write_all(&[0xEF, 0xBB, 0xBF])
                    .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
            }

            for (i, line) in lines.iter().enumerate() {
                if i == self.offsets.len() - 1 && line.is_empty() {
                    continue;
                }
                if self.encoding == "utf16le" {
                    let encoded: Vec<u8> = line
                        .encode_utf16()
                        .flat_map(|c| c.to_le_bytes())
                        .collect();
                    file.write_all(&encoded)
                        .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
                    file.write_all(&[0x0A, 0x00])
                        .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
                } else {
                    file.write_all(line.as_bytes())
                        .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
                    file.write_all(b"\n")
                        .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
                }
            }
        }

        // Re-open and re-index
        self.reopen()?;

        Ok(())
    }

    /// Export selected columns and row range to a new CSV file.
    #[napi]
    pub fn export_csv(
        &self,
        output_path: String,
        col_indices: Vec<u32>,
        start_row: u32,
        end_row: u32,
    ) -> Result<()> {
        let delimiter = self.delimiter.chars().next().unwrap_or(',');

        INNER.with(|cell| {
            let inner = cell.borrow();
            let mmap = inner.mmap.as_ref()
                .ok_or_else(|| Error::from_reason("No file open"))?;

            let mut file = BufWriter::new(
                File::create(&output_path)
                    .map_err(|e| Error::from_reason(format!("Cannot create file: {}", e)))?,
            );

            // Write BOM
            file.write_all(&[0xEF, 0xBB, 0xBF])
                .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;

            // Write header
            let header_line: Vec<String> = col_indices
                .iter()
                .map(|&i| {
                    let h = self.headers.get(i as usize).cloned().unwrap_or_default();
                    csv_quote_value(&h)
                })
                .collect();
            file.write_all(header_line.join(",").as_bytes())
                .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
            file.write_all(b"\n")
                .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;

            // Write data rows
            for r in start_row..=end_row {
                let data_row = (r + 1) as usize;
                if data_row >= self.offsets.len() {
                    break;
                }
                let text = read_row_text(mmap, &self.offsets, data_row, self.file_size, &self.encoding);
                let parsed = parse_csv_line(&text, delimiter);
                let line: Vec<String> = col_indices
                    .iter()
                    .map(|&i| {
                        let val = parsed.get(i as usize).cloned().unwrap_or_default();
                        csv_quote_value(&val)
                    })
                    .collect();
                file.write_all(line.join(",").as_bytes())
                    .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
                file.write_all(b"\n")
                    .map_err(|e| Error::from_reason(format!("Write error: {}", e)))?;
            }

            file.flush()
                .map_err(|e| Error::from_reason(format!("Flush error: {}", e)))?;

            Ok(())
        })
    }

    /// Close the current file and release resources.
    #[napi]
    pub fn close(&mut self) {
        INNER.with(|cell| cell.borrow_mut().clear());
        self.offsets.clear();
        self.headers.clear();
        self.file_path.clear();
        self.file_size = 0;
        self.delimiter = String::from(",");
        self.encoding = String::from("utf8");
        self.bom_offset = 0;
    }
}

// ─── Private Implementation ──────────────────────────────────────────────────

impl CsvEngine {
    fn reopen(&mut self) -> Result<()> {
        INNER.with(|cell| {
            let mut inner = cell.borrow_mut();
            inner.mmap = None;
            inner.cache.clear();
            inner.cache_keys.clear();
        });
        self.offsets.clear();

        let file = File::open(&self.file_path)
            .map_err(|e| Error::from_reason(format!("Cannot reopen file: {}", e)))?;

        let mmap = unsafe {
            memmap2::Mmap::map(&file)
                .map_err(|e| Error::from_reason(format!("Cannot mmap file: {}", e)))?
        };

        self.file_size = mmap.len() as i64;
        self.offsets = build_index(&mmap, self.bom_offset, &self.encoding);
        INNER.with(|cell| cell.borrow_mut().mmap = Some(mmap));

        Ok(())
    }
}

impl Default for CsvEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Utility Functions ───────────────────────────────────────────────────────

fn truncate_cell(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        text[..max_len].to_string()
    } else {
        text.to_string()
    }
}

fn detect_bom(data: &[u8]) -> (String, i64) {
    if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        (String::from("utf8"), 3)
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        (String::from("utf16le"), 2)
    } else if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        (String::from("utf16be"), 2)
    } else {
        (String::from("utf8"), 0)
    }
}

fn build_index(data: &[u8], bom_offset: i64, encoding: &str) -> Vec<i64> {
    let file_size = data.len();
    let mut offsets: Vec<i64> = Vec::with_capacity(65536);
    offsets.push(bom_offset);

    let bom = bom_offset as usize;
    if bom >= file_size {
        return offsets;
    }

    match encoding {
        "utf16le" => build_index_utf16le(data, bom, &mut offsets, file_size),
        "utf16be" => build_index_utf16be(data, bom, &mut offsets, file_size),
        _ => build_index_utf8(data, bom, &mut offsets, file_size),
    }

    while offsets.len() > 1 && offsets[offsets.len() - 1] >= file_size as i64 {
        offsets.pop();
    }

    offsets
}

fn build_index_utf8(data: &[u8], start: usize, offsets: &mut Vec<i64>, file_size: usize) {
    let mut in_quotes = false;
    let mut pos = start;

    while pos < file_size {
        let ch = data[pos];

        if ch == b'"' {
            if in_quotes && pos + 1 < file_size && data[pos + 1] == b'"' {
                pos += 1;
            } else {
                in_quotes = !in_quotes;
            }
        } else if ch == b'\n' && !in_quotes {
            let offset = (pos + 1) as i64;
            if offset < file_size as i64 {
                offsets.push(offset);
            }
        } else if ch == b'\r' && !in_quotes {
            if pos + 1 < file_size && data[pos + 1] == b'\n' {
                pos += 1;
                let offset = (pos + 1) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 1) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 1;
    }
}

fn build_index_utf16le(data: &[u8], start: usize, offsets: &mut Vec<i64>, file_size: usize) {
    let mut pos = start;
    if pos % 2 != 0 && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let lo = data[pos];
        let hi = data[pos + 1];

        if lo == 0x0A && hi == 0x00 {
            let offset = (pos + 2) as i64;
            if offset < file_size as i64 {
                offsets.push(offset);
            }
        } else if lo == 0x0D && hi == 0x00 {
            if pos + 3 < file_size && data[pos + 2] == 0x0A && data[pos + 3] == 0x00 {
                pos += 2;
                let offset = (pos + 2) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 2) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 2;
    }
}

fn build_index_utf16be(data: &[u8], start: usize, offsets: &mut Vec<i64>, file_size: usize) {
    let mut pos = start;
    if pos % 2 != 0 && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let hi = data[pos];
        let lo = data[pos + 1];

        if hi == 0x00 && lo == 0x0A {
            let offset = (pos + 2) as i64;
            if offset < file_size as i64 {
                offsets.push(offset);
            }
        } else if hi == 0x00 && lo == 0x0D {
            if pos + 3 < file_size && data[pos + 2] == 0x00 && data[pos + 3] == 0x0A {
                pos += 2;
                let offset = (pos + 2) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 2) as i64;
                if offset < file_size as i64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 2;
    }
}

fn read_row_text(
    mmap: &memmap2::Mmap,
    offsets: &[i64],
    row_index: usize,
    file_size: i64,
    encoding: &str,
) -> String {
    if row_index >= offsets.len() {
        return String::new();
    }

    let start = offsets[row_index] as usize;
    let end = if row_index + 1 < offsets.len() {
        offsets[row_index + 1] as usize
    } else {
        file_size as usize
    };

    if end <= start {
        return String::new();
    }

    let bytes = &mmap[start..end];

    match encoding {
        "utf16le" => {
            if bytes.len() < 2 {
                return String::new();
            }
            let u16_len = bytes.len() / 2;
            let u16_slice: &[u16] =
                unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, u16_len) };
            String::from_utf16_lossy(u16_slice)
        }
        "utf16be" => {
            if bytes.len() < 2 {
                return String::new();
            }
            let swapped: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16_lossy(&swapped)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn parse_csv_line(text: &str, delimiter: char) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if in_quotes {
            if ch == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    current.push('"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else if ch == '"' {
            in_quotes = true;
        } else if ch == delimiter {
            result.push(current);
            current = String::new();
        } else if ch != '\r' && ch != '\n' {
            current.push(ch);
        }
        i += 1;
    }
    result.push(current);
    result
}

fn detect_delimiter(text: &str) -> char {
    let candidates = [',', '\t', ';', '|'];
    let mut best = ',';
    let mut best_count = 0u32;

    for &delim in &candidates {
        let mut count = 0u32;
        let mut in_quotes = false;
        let chars: Vec<char> = text.chars().collect();
        let mut j = 0;

        while j < chars.len() {
            let ch = chars[j];
            if ch == '"' {
                if in_quotes && j + 1 < chars.len() && chars[j + 1] == '"' {
                    j += 1;
                } else {
                    in_quotes = !in_quotes;
                }
            } else if ch == delim && !in_quotes {
                count += 1;
            }
            j += 1;
        }

        if count > best_count {
            best_count = count;
            best = delim;
        }
    }

    best
}

fn csv_quote_value(val: &str) -> String {
    if val.contains(',') || val.contains('"') || val.contains('\n') || val.contains('\r') {
        format!("\"{}\"", val.replace('"', "\"\""))
    } else {
        val.to_string()
    }
}

fn format_csv_row(cells: &[String], delimiter: char) -> String {
    cells
        .iter()
        .map(|c| csv_quote_value(c))
        .collect::<Vec<_>>()
        .join(&delimiter.to_string())
}
