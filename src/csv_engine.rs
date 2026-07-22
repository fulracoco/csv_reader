use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
#[cfg(test)]
use encoding_rs::GBK;
use encoding_rs::{Encoding, GB18030, UTF_8};
use memchr::{memchr2_iter, memchr_iter, memmem};
use memmap2::Mmap;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

const MAX_CELL_PREVIEW: usize = 500;
const CACHE_MAX_ROWS: usize = 500;
const CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
const ENCODING_UTF8: &str = "utf8";
const ENCODING_UTF16_LE: &str = "utf16le";
const ENCODING_UTF16_BE: &str = "utf16be";
const ENCODING_GBK: &str = "gbk";
const ENCODING_GB18030: &str = "gb18030";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

// ─── Public data types ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OpenResult {
    pub file_path: String,
    pub file_name: String,
    pub file_size: u64,
    pub row_count: u32,
    pub column_count: u32,
    pub headers: Vec<String>,
    pub encoding: String,
}

#[derive(Debug, Clone)]
pub struct RowData {
    pub cells: Vec<String>,
    #[allow(dead_code)]
    pub lengths: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub row_index: u32,
    pub matches: Vec<CellMatch>,
}

#[derive(Debug, Clone)]
pub struct CellMatch {
    pub col_index: u32,
    pub col_name: String,
    pub cell_text: String,
}

#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub done: u32,
    pub total: u32,
}

#[derive(Debug)]
enum RowOffsets {
    U32(Vec<u32>),
    U64(Vec<u64>),
}

impl RowOffsets {
    fn empty() -> Self {
        Self::U32(Vec::new())
    }

    fn with_capacity(file_size: usize, capacity: usize) -> Self {
        if u32::try_from(file_size).is_ok() {
            Self::U32(Vec::with_capacity(capacity))
        } else {
            Self::U64(Vec::with_capacity(capacity))
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::U32(offsets) => offsets.len(),
            Self::U64(offsets) => offsets.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, index: usize) -> u64 {
        match self {
            Self::U32(offsets) => u64::from(offsets[index]),
            Self::U64(offsets) => offsets[index],
        }
    }

    fn push(&mut self, offset: u64) {
        match self {
            Self::U32(offsets) => offsets.push(offset as u32),
            Self::U64(offsets) => offsets.push(offset),
        }
    }

    fn pop(&mut self) {
        match self {
            Self::U32(offsets) => {
                offsets.pop();
            }
            Self::U64(offsets) => {
                offsets.pop();
            }
        }
    }

    fn last(&self) -> Option<u64> {
        match self {
            Self::U32(offsets) => offsets.last().copied().map(u64::from),
            Self::U64(offsets) => offsets.last().copied(),
        }
    }
}

// ─── CsvEngine ───────────────────────────────────────────────────────────────

pub struct CsvEngine {
    mmap: Option<Arc<Mmap>>,
    file_path: String,
    file_size: u64,
    offsets: Arc<RowOffsets>,
    headers: Vec<String>,
    delimiter: u8,
    encoding: String,
    bom_offset: u64,
    cache: HashMap<u32, Vec<String>>,
    cache_order: Vec<u32>,
    cache_bytes: usize,
}

impl CsvEngine {
    pub fn new() -> Self {
        CsvEngine {
            mmap: None,
            file_path: String::new(),
            file_size: 0,
            offsets: Arc::new(RowOffsets::empty()),
            headers: Vec::new(),
            delimiter: b',',
            encoding: String::from(ENCODING_UTF8),
            bom_offset: 0,
            cache: HashMap::with_capacity(CACHE_MAX_ROWS),
            cache_order: Vec::with_capacity(CACHE_MAX_ROWS),
            cache_bytes: 0,
        }
    }

    pub fn open(&mut self, file_path: &str) -> Result<OpenResult, String> {
        self.close();

        let file = File::open(file_path).map_err(|e| format!("Cannot open file: {}", e))?;

        let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("Cannot mmap file: {}", e))? };

        let file_size = mmap.len() as u64;
        let (encoding, bom_offset) = detect_encoding(&mmap);

        let offsets = build_index(&mmap, bom_offset, &encoding);

        if offsets.is_empty() {
            self.mmap = Some(Arc::new(mmap));
            self.file_path = file_path.to_string();
            self.file_size = file_size;
            self.offsets = Arc::new(RowOffsets::empty());
            self.headers = Vec::new();
            self.delimiter = b',';
            self.encoding = encoding;
            self.bom_offset = bom_offset;
            return Ok(OpenResult {
                file_path: self.file_path.clone(),
                file_name: Path::new(&self.file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                file_size,
                row_count: 0,
                column_count: 0,
                headers: Vec::new(),
                encoding: encoding_display_name(&self.encoding),
            });
        }

        let header_text = read_row_text(&mmap, &offsets, 0, file_size, &encoding);
        let delimiter = detect_delimiter(&header_text);
        let headers = parse_csv_line(&header_text, delimiter);

        let row_count = offsets.len().saturating_sub(1) as u32;
        let column_count = headers.len() as u32;

        self.mmap = Some(Arc::new(mmap));
        self.file_path = file_path.to_string();
        self.file_size = file_size;
        self.offsets = Arc::new(offsets);
        self.headers = headers.clone();
        self.delimiter = delimiter as u8;
        self.encoding = encoding;
        self.bom_offset = bom_offset;

        Ok(OpenResult {
            file_path: self.file_path.clone(),
            file_name: Path::new(&self.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            file_size,
            row_count,
            column_count,
            headers,
            encoding: encoding_display_name(&self.encoding),
        })
    }

    pub fn get_rows(
        &mut self,
        start_row: u32,
        count: u32,
        col_start: u32,
        col_count: u32,
    ) -> Result<Vec<RowData>, String> {
        let delimiter = self.delimiter as char;

        let mut results = Vec::with_capacity(count as usize);
        let mut cache_updates: Vec<(u32, Option<Vec<String>>)> = Vec::with_capacity(count as usize);

        {
            let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;

            for i in 0..count {
                let row_idx = start_row + i + 1;
                if row_idx as usize >= self.offsets.len() {
                    break;
                }

                if let Some(cached) = self.cache.get(&row_idx) {
                    cache_updates.push((row_idx, None));
                    results.push(row_data_for_columns(cached, col_start, col_count));
                } else {
                    let text = read_row_text(
                        mmap,
                        &self.offsets,
                        row_idx as usize,
                        self.file_size,
                        &self.encoding,
                    );
                    let parsed = parse_csv_line(&text, delimiter);
                    let row_data = row_data_for_columns(&parsed, col_start, col_count);
                    cache_updates.push((row_idx, Some(parsed)));
                    results.push(row_data);
                }
            }
        }

        for (row_idx, data_opt) in cache_updates {
            match data_opt {
                Some(data) => self.add_to_cache(row_idx, data),
                None => self.touch_cache(row_idx),
            }
        }

        Ok(results)
    }

    #[allow(dead_code)]
    pub fn get_rows_by_index(&mut self, indices: &[u32]) -> Result<Vec<Vec<String>>, String> {
        if indices.is_empty() {
            return Ok(Vec::new());
        }

        let delimiter = self.delimiter as char;

        let mut sorted: Vec<(usize, u32)> =
            indices.iter().enumerate().map(|(i, &r)| (i, r)).collect();
        sorted.sort_by_key(|&(_, r)| r);

        let mut results: Vec<Option<Vec<String>>> = vec![None; indices.len()];
        let mut cache_updates: Vec<(u32, Option<Vec<String>>)> = Vec::new();

        {
            let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;

            for &(orig_idx, data_row) in &sorted {
                let offset_idx = data_row + 1;
                if offset_idx as usize >= self.offsets.len() {
                    continue;
                }

                if let Some(cached) = self.cache.get(&offset_idx) {
                    cache_updates.push((offset_idx, None));
                    results[orig_idx] = Some(cached.clone());
                } else {
                    let text = read_row_text(
                        mmap,
                        &self.offsets,
                        offset_idx as usize,
                        self.file_size,
                        &self.encoding,
                    );
                    let parsed = parse_csv_line(&text, delimiter);
                    cache_updates.push((offset_idx, Some(parsed.clone())));
                    results[orig_idx] = Some(parsed);
                }
            }
        }

        for (row_idx, data_opt) in cache_updates {
            match data_opt {
                Some(data) => self.add_to_cache(row_idx, data),
                None => self.touch_cache(row_idx),
            }
        }

        Ok(results.into_iter().map(|r| r.unwrap_or_default()).collect())
    }

    pub fn get_cell_content(&mut self, row_index: u32, col_index: u32) -> Result<String, String> {
        let delimiter = self.delimiter as char;
        let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;

        let data_row = row_index + 1;
        if data_row as usize >= self.offsets.len() {
            return Ok(String::new());
        }

        let text = read_row_text(
            mmap,
            &self.offsets,
            data_row as usize,
            self.file_size,
            &self.encoding,
        );
        let parsed = parse_csv_line(&text, delimiter);
        Ok(parsed.get(col_index as usize).cloned().unwrap_or_default())
    }

    pub fn update_cell(
        &mut self,
        row_index: u32,
        col_index: u32,
        new_content: &str,
    ) -> Result<(), String> {
        let delimiter = self.delimiter as char;
        let data_row = (row_index + 1) as usize;

        if data_row >= self.offsets.len() {
            return Err("Row out of range".to_string());
        }

        let (row_start, row_end, replacement, line_ending) = {
            let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;
            let row_start = self.offsets.get(data_row) as usize;
            let row_end = if data_row + 1 < self.offsets.len() {
                self.offsets.get(data_row + 1) as usize
            } else {
                self.file_size as usize
            };
            let text = read_row_text(
                mmap,
                &self.offsets,
                data_row,
                self.file_size,
                &self.encoding,
            );
            let mut parsed = parse_csv_line(&text, delimiter);
            while parsed.len() <= col_index as usize {
                parsed.push(String::new());
            }
            parsed[col_index as usize] = new_content.to_string();
            let replacement = format_csv_row(&parsed, delimiter);
            let ending_len = line_ending_len(&mmap[row_start..row_end], &self.encoding);
            let line_ending = mmap[row_end - ending_len..row_end].to_vec();
            (row_start, row_end, replacement, line_ending)
        };

        let source_path = PathBuf::from(&self.file_path);
        let permissions = fs::metadata(&source_path)
            .map_err(|e| format!("Cannot read file metadata: {e}"))?
            .permissions();
        let (temp_path, temp_file) = create_temp_file(&source_path)?;
        let write_result = (|| -> Result<(), String> {
            let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;
            let mut writer = BufWriter::new(temp_file);
            writer
                .write_all(&mmap[..row_start])
                .map_err(|e| format!("Write error: {e}"))?;
            write_encoded(&mut writer, &replacement, &self.encoding)
                .map_err(|e| format!("Write error: {e}"))?;
            writer
                .write_all(&line_ending)
                .map_err(|e| format!("Write error: {e}"))?;
            writer
                .write_all(&mmap[row_end..])
                .map_err(|e| format!("Write error: {e}"))?;
            writer.flush().map_err(|e| format!("Flush error: {e}"))?;
            writer
                .get_ref()
                .sync_all()
                .map_err(|e| format!("Sync error: {e}"))?;
            Ok(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }

        if let Err(error) = fs::set_permissions(&temp_path, permissions) {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("Cannot preserve file permissions: {error}"));
        }
        self.mmap = None;
        if let Err(error) = replace_file(&source_path, &temp_path) {
            let _ = fs::remove_file(&temp_path);
            let reopen_error = self.reopen().err();
            return Err(match reopen_error {
                Some(reopen_error) => {
                    format!(
                        "Cannot replace source file: {error}; cannot reopen source: {reopen_error}"
                    )
                }
                None => format!("Cannot replace source file: {error}"),
            });
        }

        self.reopen()?;

        Ok(())
    }

    pub fn export_csv(
        &self,
        output_path: &str,
        col_indices: &[u32],
        start_row: u32,
        end_row: u32,
    ) -> Result<(), String> {
        let delimiter = self.delimiter as char;
        let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;

        let mut file = BufWriter::new(
            File::create(output_path).map_err(|e| format!("Cannot create file: {}", e))?,
        );

        file.write_all(&[0xEF, 0xBB, 0xBF])
            .map_err(|e| format!("Write error: {}", e))?;

        let header_line: Vec<String> = col_indices
            .iter()
            .map(|&i| {
                let h = self.headers.get(i as usize).cloned().unwrap_or_default();
                csv_quote_value(&h, ',')
            })
            .collect();
        file.write_all(header_line.join(",").as_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        file.write_all(b"\n")
            .map_err(|e| format!("Write error: {}", e))?;

        for r in start_row..=end_row {
            let data_row = (r + 1) as usize;
            if data_row >= self.offsets.len() {
                break;
            }
            let text = read_row_text(
                mmap,
                &self.offsets,
                data_row,
                self.file_size,
                &self.encoding,
            );
            let parsed = parse_csv_line(&text, delimiter);
            let line: Vec<String> = col_indices
                .iter()
                .map(|&i| {
                    let val = parsed.get(i as usize).cloned().unwrap_or_default();
                    csv_quote_value(&val, ',')
                })
                .collect();
            file.write_all(line.join(",").as_bytes())
                .map_err(|e| format!("Write error: {}", e))?;
            file.write_all(b"\n")
                .map_err(|e| format!("Write error: {}", e))?;
        }

        file.flush().map_err(|e| format!("Flush error: {}", e))?;

        Ok(())
    }

    pub fn close(&mut self) {
        self.mmap = None;
        self.cache.clear();
        self.cache_order.clear();
        self.cache_bytes = 0;
        self.offsets = Arc::new(RowOffsets::empty());
        self.headers.clear();
        self.file_path.clear();
        self.file_size = 0;
        self.delimiter = b',';
        self.encoding = String::from(ENCODING_UTF8);
        self.bom_offset = 0;
    }

    #[allow(dead_code)]
    pub fn is_open(&self) -> bool {
        self.mmap.is_some()
    }

    #[allow(dead_code)]
    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    #[allow(dead_code)]
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    #[allow(dead_code)]
    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    #[allow(dead_code)]
    pub fn row_count(&self) -> u32 {
        self.offsets.len().saturating_sub(1) as u32
    }

    #[allow(dead_code)]
    pub fn column_count(&self) -> u32 {
        self.headers.len() as u32
    }

    pub fn search_with_progress(
        &self,
        query: &str,
        col_filter: Option<u32>,
        case_sensitive: bool,
        max_results: u32,
        on_progress: impl Fn(u32, u32) + Send + Sync,
    ) -> Result<Vec<SearchResult>, String> {
        let mmap = Arc::clone(self.mmap.as_ref().ok_or("No file open")?);
        let offsets = Arc::clone(&self.offsets);
        let encoding = self.encoding.clone();
        let delimiter = self.delimiter;
        let file_size = self.file_size;
        let headers = self.headers.clone();
        if offsets.len() <= 1 || query.is_empty() {
            return Ok(Vec::new());
        }

        let total = (offsets.len() - 1) as u32;
        let is_utf8 = encoding == ENCODING_UTF8;
        let query_is_ascii = query.is_ascii();
        let query_bytes = query.as_bytes();
        let query_lower = (!case_sensitive).then(|| query.to_lowercase());

        let scanned = AtomicU32::new(0);
        let collected = AtomicU32::new(0);
        let cancelled = AtomicBool::new(false);

        let results: Vec<SearchResult> = (1..offsets.len())
            .into_par_iter()
            .filter_map(|i| {
                if cancelled.load(Ordering::Relaxed) {
                    return None;
                }

                let start = offsets.get(i) as usize;
                let end = if i + 1 < offsets.len() {
                    offsets.get(i + 1) as usize
                } else {
                    file_size as usize
                };
                if end <= start || start >= mmap.len() {
                    return None;
                }
                let row_bytes = &mmap[start..end.min(mmap.len())];

                // SIMD-accelerated byte search via memchr
                let row_contains = if is_utf8 && query_is_ascii && case_sensitive {
                    memmem::find(row_bytes, query_bytes).is_some()
                } else if is_utf8 && query_is_ascii {
                    contains_ascii_case_insensitive(row_bytes, query_bytes)
                } else if is_utf8 {
                    let row_text = String::from_utf8_lossy(row_bytes);
                    contains_text(&row_text, query, query_lower.as_deref())
                } else {
                    let row_text = read_row_text(&mmap, &offsets, i, file_size, &encoding);
                    if row_text.is_empty() {
                        return None;
                    }
                    contains_text(&row_text, query, query_lower.as_deref())
                };

                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(16384) || done == total {
                    on_progress(done, total);
                }

                if !row_contains {
                    return None;
                }

                let row_text = read_row_text(&mmap, &offsets, i, file_size, &encoding);
                let cells = parse_csv_line(&row_text, delimiter as char);

                let matches: Vec<CellMatch> = if let Some(ci) = col_filter {
                    let ci = ci as usize;
                    if let Some(cell_text) = cells.get(ci) {
                        let found = contains_text(cell_text, query, query_lower.as_deref());
                        if found {
                            vec![CellMatch {
                                col_index: ci as u32,
                                col_name: headers.get(ci).cloned().unwrap_or_default(),
                                cell_text: extract_match_context(
                                    cell_text,
                                    query,
                                    query_lower.as_deref(),
                                ),
                            }]
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    }
                } else {
                    cells
                        .iter()
                        .enumerate()
                        .filter_map(|(ci, cell_text)| {
                            let found = contains_text(cell_text, query, query_lower.as_deref());
                            if found {
                                Some(CellMatch {
                                    col_index: ci as u32,
                                    col_name: headers.get(ci).cloned().unwrap_or_default(),
                                    cell_text: extract_match_context(
                                        cell_text,
                                        query,
                                        query_lower.as_deref(),
                                    ),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                if matches.is_empty() {
                    return None;
                }

                let nth = collected.fetch_add(1, Ordering::Relaxed);
                if nth >= max_results {
                    cancelled.store(true, Ordering::Relaxed);
                    return None;
                }

                Some(SearchResult {
                    row_index: (i - 1) as u32,
                    matches,
                })
            })
            .collect();

        on_progress(total, total);
        Ok(results)
    }
}

// ─── Cache helpers ───────────────────────────────────────────────────────────

impl CsvEngine {
    fn touch_cache(&mut self, key: u32) {
        if let Some(pos) = self.cache_order.iter().position(|&k| k == key) {
            self.cache_order.remove(pos);
        }
        self.cache_order.push(key);
    }

    fn add_to_cache(&mut self, key: u32, data: Vec<String>) {
        let data_size = cache_entry_size(&data);
        if let Some(previous) = self.cache.insert(key, data) {
            self.cache_bytes = self.cache_bytes.saturating_sub(cache_entry_size(&previous));
        }
        self.cache_bytes = self.cache_bytes.saturating_add(data_size);
        if let Some(pos) = self.cache_order.iter().position(|&k| k == key) {
            self.cache_order.remove(pos);
        }
        self.cache_order.push(key);
        while self.cache_order.len() > CACHE_MAX_ROWS || self.cache_bytes > CACHE_MAX_BYTES {
            if let Some(oldest) = self.cache_order.first().copied() {
                self.cache_order.remove(0);
                if let Some(removed) = self.cache.remove(&oldest) {
                    self.cache_bytes = self.cache_bytes.saturating_sub(cache_entry_size(&removed));
                }
            }
        }
    }

    fn reopen(&mut self) -> Result<(), String> {
        self.cache.clear();
        self.cache_order.clear();
        self.cache_bytes = 0;

        let file = File::open(&self.file_path).map_err(|e| format!("Cannot reopen file: {}", e))?;

        let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("Cannot mmap file: {}", e))? };

        self.file_size = mmap.len() as u64;
        self.offsets = Arc::new(build_index(&mmap, self.bom_offset, &self.encoding));
        self.mmap = Some(Arc::new(mmap));

        Ok(())
    }
}

impl Default for CsvEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Utility functions ───────────────────────────────────────────────────────

fn cache_entry_size(cells: &[String]) -> usize {
    std::mem::size_of::<Vec<String>>()
        + std::mem::size_of_val(cells)
        + cells.iter().map(|cell| cell.capacity()).sum::<usize>()
}

fn row_data_for_columns(cells: &[String], col_start: u32, col_count: u32) -> RowData {
    let start = (col_start as usize).min(cells.len());
    let end = start.saturating_add(col_count as usize).min(cells.len());
    let visible = &cells[start..end];
    RowData {
        cells: visible
            .iter()
            .map(|cell| truncate_cell(cell, MAX_CELL_PREVIEW))
            .collect(),
        lengths: visible.iter().map(|cell| cell.len() as u32).collect(),
    }
}

fn create_temp_file(source_path: &Path) -> Result<(PathBuf, File), String> {
    let parent = source_path
        .parent()
        .ok_or_else(|| "Source file has no parent directory".to_string())?;
    let file_name = source_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();

    for _ in 0..100 {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{file_name}.csv-reader-{}-{counter}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot create temporary file: {error}")),
        }
    }

    Err("Cannot create a unique temporary file".to_string())
}

fn write_encoded(writer: &mut impl Write, text: &str, encoding: &str) -> std::io::Result<()> {
    match encoding {
        ENCODING_UTF16_LE => {
            for unit in text.encode_utf16() {
                writer.write_all(&unit.to_le_bytes())?;
            }
        }
        ENCODING_UTF16_BE => {
            for unit in text.encode_utf16() {
                writer.write_all(&unit.to_be_bytes())?;
            }
        }
        ENCODING_UTF8 => writer.write_all(text.as_bytes())?,
        _ => {
            let codec = codec_for_encoding(encoding);
            let (encoded, _, had_errors) = codec.encode(text);
            if had_errors {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Text contains characters that cannot be represented in {}",
                        encoding_display_name(encoding)
                    ),
                ));
            }
            writer.write_all(encoded.as_ref())?;
        }
    }
    Ok(())
}

fn line_ending_len(row_bytes: &[u8], encoding: &str) -> usize {
    match encoding {
        ENCODING_UTF16_LE => {
            if row_bytes.ends_with(&[0x0D, 0x00, 0x0A, 0x00]) {
                4
            } else if row_bytes.ends_with(&[0x0A, 0x00]) || row_bytes.ends_with(&[0x0D, 0x00]) {
                2
            } else {
                0
            }
        }
        ENCODING_UTF16_BE => {
            if row_bytes.ends_with(&[0x00, 0x0D, 0x00, 0x0A]) {
                4
            } else if row_bytes.ends_with(&[0x00, 0x0A]) || row_bytes.ends_with(&[0x00, 0x0D]) {
                2
            } else {
                0
            }
        }
        _ => {
            if row_bytes.ends_with(b"\r\n") {
                2
            } else if row_bytes.ends_with(b"\n") || row_bytes.ends_with(b"\r") {
                1
            } else {
                0
            }
        }
    }
}

#[cfg(not(windows))]
fn replace_file(source_path: &Path, temp_path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, source_path)
}

#[cfg(windows)]
fn replace_file(source_path: &Path, temp_path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    let source: Vec<u16> = source_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let replacement: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result = unsafe {
        ReplaceFileW(
            source.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn truncate_cell(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        let mut end = max_len;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    } else {
        text.to_string()
    }
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    let first_lower = needle[0].to_ascii_lowercase();
    let first_upper = needle[0].to_ascii_uppercase();
    let is_match = |start: usize| {
        haystack
            .get(start..start + needle.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
    };

    if first_lower == first_upper {
        memchr_iter(first_lower, haystack).any(is_match)
    } else {
        memchr2_iter(first_lower, first_upper, haystack).any(is_match)
    }
}

fn contains_text(text: &str, query: &str, query_lower: Option<&str>) -> bool {
    match query_lower {
        Some(lower) => text.to_lowercase().contains(lower),
        None => text.contains(query),
    }
}

fn find_case_insensitive(text: &str, query_lower: &str) -> Option<(usize, usize)> {
    for (start, _) in text.char_indices() {
        let mut candidate = String::new();
        for (relative_end, ch) in text[start..].char_indices() {
            candidate.extend(ch.to_lowercase());
            let end = start + relative_end + ch.len_utf8();
            if candidate.starts_with(query_lower) {
                return Some((start, end));
            }
            if !query_lower.starts_with(&candidate) {
                break;
            }
        }
    }
    None
}

fn extract_match_context(cell_text: &str, query: &str, query_lower: Option<&str>) -> String {
    let match_range = if let Some(lower) = query_lower {
        find_case_insensitive(cell_text, lower)
    } else {
        cell_text
            .find(query)
            .map(|start| (start, start + query.len()))
    };

    let (match_start, match_end) = match match_range {
        Some(range) => range,
        None => return truncate_cell(cell_text, 200),
    };

    let radius = 90usize;
    let mut preview_start = match_start.saturating_sub(radius);
    while !cell_text.is_char_boundary(preview_start) {
        preview_start += 1;
    }
    let mut preview_end = (match_end + radius).min(cell_text.len());
    while !cell_text.is_char_boundary(preview_end) {
        preview_end -= 1;
    }

    let mut preview = cell_text[preview_start..preview_end].to_string();

    if preview_start > 0 {
        preview.insert_str(0, "...");
    }
    if preview_end < cell_text.len() {
        preview.push_str("...");
    }

    if preview.len() > 250 {
        preview = truncate_cell(&preview, 250);
    }

    preview
}

fn detect_encoding(data: &[u8]) -> (String, u64) {
    if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        (String::from(ENCODING_UTF8), 3)
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        (String::from(ENCODING_UTF16_LE), 2)
    } else if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        (String::from(ENCODING_UTF16_BE), 2)
    } else if let Some(encoding) = detect_utf16_without_bom(data) {
        (String::from(encoding), 0)
    } else if std::str::from_utf8(data).is_ok() {
        (String::from(ENCODING_UTF8), 0)
    } else if contains_gb18030_four_byte_sequence(data) {
        (String::from(ENCODING_GB18030), 0)
    } else {
        // ISO-2022-JP can contain CSV syntax bytes while in a shifted state,
        // which is incompatible with this engine's byte-level random-access index.
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
        detector.feed(data, true);
        let detected = detector.guess(None, Utf8Detection::Deny).name();
        let encoding = if detected.eq_ignore_ascii_case("GBK") {
            ENCODING_GBK
        } else {
            detected
        };
        (encoding.to_string(), 0)
    }
}

fn detect_utf16_without_bom(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 || !data.len().is_multiple_of(2) {
        return None;
    }
    let sample_len = data.len().min(64 * 1024) & !1;
    let pairs = sample_len / 2;
    let mut even_zeroes = 0usize;
    let mut odd_zeroes = 0usize;
    for pair in data[..sample_len].chunks_exact(2) {
        even_zeroes += usize::from(pair[0] == 0);
        odd_zeroes += usize::from(pair[1] == 0);
    }
    let minimum_zeroes = (pairs / 10).max(2);
    if odd_zeroes >= minimum_zeroes && odd_zeroes >= even_zeroes.saturating_mul(4) {
        Some(ENCODING_UTF16_LE)
    } else if even_zeroes >= minimum_zeroes && even_zeroes >= odd_zeroes.saturating_mul(4) {
        Some(ENCODING_UTF16_BE)
    } else {
        None
    }
}

fn contains_gb18030_four_byte_sequence(data: &[u8]) -> bool {
    data.windows(4).any(|bytes| {
        (0x81..=0xFE).contains(&bytes[0])
            && bytes[1].is_ascii_digit()
            && (0x81..=0xFE).contains(&bytes[2])
            && bytes[3].is_ascii_digit()
    })
}

fn encoding_display_name(encoding: &str) -> String {
    match encoding {
        ENCODING_UTF8 => "UTF-8".to_string(),
        ENCODING_UTF16_LE => "UTF-16 LE".to_string(),
        ENCODING_UTF16_BE => "UTF-16 BE".to_string(),
        ENCODING_GBK | "GBK" => "GBK".to_string(),
        ENCODING_GB18030 => "GB18030".to_string(),
        _ => encoding.to_string(),
    }
}

fn codec_for_encoding(encoding: &str) -> &'static Encoding {
    if encoding == ENCODING_GB18030 {
        GB18030
    } else {
        Encoding::for_label(encoding.as_bytes()).unwrap_or(UTF_8)
    }
}

fn build_index(data: &[u8], bom_offset: u64, encoding: &str) -> RowOffsets {
    let file_size = data.len();
    let mut offsets = RowOffsets::with_capacity(file_size, 65536);
    offsets.push(bom_offset);

    let bom = bom_offset as usize;
    if bom >= file_size {
        return offsets;
    }

    match encoding {
        ENCODING_UTF16_LE => build_index_utf16le(data, bom, &mut offsets, file_size),
        ENCODING_UTF16_BE => build_index_utf16be(data, bom, &mut offsets, file_size),
        _ => build_index_utf8(data, bom, &mut offsets, file_size),
    }

    while offsets.len() > 1
        && offsets
            .last()
            .is_some_and(|offset| offset >= file_size as u64)
    {
        offsets.pop();
    }

    offsets
}

fn build_index_utf8(data: &[u8], start: usize, offsets: &mut RowOffsets, file_size: usize) {
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
            let offset = (pos + 1) as u64;
            if offset < file_size as u64 {
                offsets.push(offset);
            }
        } else if ch == b'\r' && !in_quotes {
            if pos + 1 < file_size && data[pos + 1] == b'\n' {
                pos += 1;
                let offset = (pos + 1) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 1) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 1;
    }
}

fn build_index_utf16le(data: &[u8], start: usize, offsets: &mut RowOffsets, file_size: usize) {
    let mut pos = start;
    let mut in_quotes = false;
    if !pos.is_multiple_of(2) && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let lo = data[pos];
        let hi = data[pos + 1];

        if lo == 0x22 && hi == 0x00 {
            if in_quotes && pos + 3 < file_size && data[pos + 2] == 0x22 && data[pos + 3] == 0x00 {
                pos += 2;
            } else {
                in_quotes = !in_quotes;
            }
        } else if lo == 0x0A && hi == 0x00 && !in_quotes {
            let offset = (pos + 2) as u64;
            if offset < file_size as u64 {
                offsets.push(offset);
            }
        } else if lo == 0x0D && hi == 0x00 && !in_quotes {
            if pos + 3 < file_size && data[pos + 2] == 0x0A && data[pos + 3] == 0x00 {
                pos += 2;
                let offset = (pos + 2) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 2) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 2;
    }
}

fn build_index_utf16be(data: &[u8], start: usize, offsets: &mut RowOffsets, file_size: usize) {
    let mut pos = start;
    let mut in_quotes = false;
    if !pos.is_multiple_of(2) && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let hi = data[pos];
        let lo = data[pos + 1];

        if hi == 0x00 && lo == 0x22 {
            if in_quotes && pos + 3 < file_size && data[pos + 2] == 0x00 && data[pos + 3] == 0x22 {
                pos += 2;
            } else {
                in_quotes = !in_quotes;
            }
        } else if hi == 0x00 && lo == 0x0A && !in_quotes {
            let offset = (pos + 2) as u64;
            if offset < file_size as u64 {
                offsets.push(offset);
            }
        } else if hi == 0x00 && lo == 0x0D && !in_quotes {
            if pos + 3 < file_size && data[pos + 2] == 0x00 && data[pos + 3] == 0x0A {
                pos += 2;
                let offset = (pos + 2) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            } else {
                let offset = (pos + 2) as u64;
                if offset < file_size as u64 {
                    offsets.push(offset);
                }
            }
        }

        pos += 2;
    }
}

fn read_row_text(
    mmap: &Mmap,
    offsets: &RowOffsets,
    row_index: usize,
    file_size: u64,
    encoding: &str,
) -> String {
    if row_index >= offsets.len() {
        return String::new();
    }

    let start = offsets.get(row_index) as usize;
    let end = if row_index + 1 < offsets.len() {
        offsets.get(row_index + 1) as usize
    } else {
        file_size as usize
    };

    if end <= start {
        return String::new();
    }

    let bytes = &mmap[start..end];

    match encoding {
        ENCODING_UTF16_LE => {
            if bytes.len() < 2 {
                return String::new();
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        ENCODING_UTF16_BE => {
            if bytes.len() < 2 {
                return String::new();
            }
            let swapped: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16_lossy(&swapped)
        }
        ENCODING_UTF8 => String::from_utf8_lossy(bytes).into_owned(),
        _ => codec_for_encoding(encoding)
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

fn parse_csv_line(text: &str, delimiter: char) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.next_if_eq(&'"').is_some() {
                    current.push('"');
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
    }
    result.push(current);
    result
}

fn detect_delimiter(text: &str) -> char {
    let candidates = [',', '\t', ';', '|'];
    let mut counts = [0u32; 4];
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            if in_quotes && chars.next_if_eq(&'"').is_some() {
                continue;
            }
            in_quotes = !in_quotes;
        } else if !in_quotes {
            if let Some(index) = candidates.iter().position(|&candidate| candidate == ch) {
                counts[index] += 1;
            }
        }
    }

    let mut best_index = 0;
    for index in 1..counts.len() {
        if counts[index] > counts[best_index] {
            best_index = index;
        }
    }

    if counts[best_index] == 0 {
        ','
    } else {
        candidates[best_index]
    }
}

fn csv_quote_value(val: &str, delimiter: char) -> String {
    if val.contains(delimiter) || val.contains('"') || val.contains('\n') || val.contains('\r') {
        format!("\"{}\"", val.replace('"', "\"\""))
    } else {
        val.to_string()
    }
}

fn format_csv_row(cells: &[String], delimiter: char) -> String {
    cells
        .iter()
        .map(|c| csv_quote_value(c, delimiter))
        .collect::<Vec<_>>()
        .join(&delimiter.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::{BIG5, EUC_JP, EUC_KR, SHIFT_JIS, WINDOWS_1252};
    use rayon::ThreadPoolBuilder;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "csv-reader-{label}-{}-{unique}.csv",
            std::process::id()
        ))
    }

    fn utf16_fixture(text: &str, little_endian: bool) -> Vec<u8> {
        let mut bytes = if little_endian {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        bytes
    }

    fn legacy_fixture(text: &str, encoding: &'static encoding_rs::Encoding) -> Vec<u8> {
        let (bytes, _, had_errors) = encoding.encode(text);
        assert!(!had_errors, "fixture must be representable");
        bytes.into_owned()
    }

    #[test]
    fn detects_common_csv_encodings_without_manual_selection() {
        assert_eq!(
            detect_encoding("姓名,城市\n张三,北京".as_bytes()),
            (ENCODING_UTF8.to_string(), 0)
        );

        let gbk = legacy_fixture("姓名,城市\n张三,北京", GBK);
        assert_eq!(detect_encoding(&gbk), (ENCODING_GBK.to_string(), 0));

        let utf16_le: Vec<u8> = "name,城市\nvalue,北京"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            detect_encoding(&utf16_le),
            (ENCODING_UTF16_LE.to_string(), 0)
        );

        let gb18030 = legacy_fixture("name,value\nemoji,😀", GB18030);
        assert!(contains_gb18030_four_byte_sequence(&gb18030));
        assert_eq!(detect_encoding(&gb18030), (ENCODING_GB18030.to_string(), 0));
    }

    #[test]
    fn opens_searches_edits_and_exports_gbk_csv() {
        let path = temp_path("gbk");
        let export_path = temp_path("gbk-export");
        fs::write(
            &path,
            legacy_fixture("姓名,城市\r\n张三,北京\r\n李四,上海", GBK),
        )
        .expect("GBK fixture should be writable");

        let mut engine = CsvEngine::new();
        let info = engine
            .open(path.to_str().expect("temporary path should be valid UTF-8"))
            .expect("GBK fixture should open");
        assert_eq!(info.encoding, "GBK");
        assert_eq!(info.headers, ["姓名", "城市"]);
        assert_eq!(
            engine.get_rows(0, 2, 0, 2).expect("GBK rows should decode")[0].cells,
            ["张三", "北京"]
        );

        let results = engine
            .search_with_progress("上海", None, false, 10, |_, _| {})
            .expect("GBK text should be searchable");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_index, 1);

        engine
            .update_cell(0, 1, "广州")
            .expect("GBK cell should be editable");
        let source_bytes = fs::read(&path).expect("edited fixture should be readable");
        assert!(std::str::from_utf8(&source_bytes).is_err());
        assert_eq!(
            GBK.decode_without_bom_handling(&source_bytes).0,
            "姓名,城市\r\n张三,广州\r\n李四,上海"
        );

        engine
            .export_csv(
                export_path
                    .to_str()
                    .expect("temporary export path should be valid UTF-8"),
                &[0, 1],
                0,
                1,
            )
            .expect("GBK rows should export as UTF-8");
        let exported = fs::read(&export_path).expect("export should be readable");
        assert!(exported.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert_eq!(
            std::str::from_utf8(&exported[3..]).expect("export should be UTF-8"),
            "姓名,城市\n张三,广州\n李四,上海\n"
        );

        engine.close();
        fs::remove_file(path).expect("fixture should be removable");
        fs::remove_file(export_path).expect("export should be removable");
    }

    #[test]
    fn detects_additional_legacy_encodings() {
        let samples = [
            (
                BIG5,
                "姓名,城市,備註\n陳大文,臺北,繁體中文資料\n林美玲,高雄,測試內容\n王小明,臺中,聯絡資訊\n",
            ),
            (
                SHIFT_JIS,
                "名前,都市,備考\n山田太郎,東京,日本語データ\n鈴木花子,大阪,テスト内容\n佐藤一郎,京都,連絡情報\n",
            ),
            (
                EUC_JP,
                "名前,都市,備考\n山田太郎,東京,日本語データ\n鈴木花子,大阪,テスト内容\n佐藤一郎,京都,連絡情報\n",
            ),
            (
                EUC_KR,
                "이름,도시,비고\n김민수,서울,한국어 자료\n이영희,부산,테스트 내용\n박철수,대구,연락 정보\n",
            ),
            (
                WINDOWS_1252,
                "name,city,notes\nAndré,Paris,résumé\nJürgen,München,größer\nFrançois,Zürich,élève\n",
            ),
        ];

        for (encoding, text) in samples {
            let bytes = legacy_fixture(text, encoding);
            let (detected, bom_offset) = detect_encoding(&bytes);
            assert_eq!(bom_offset, 0);
            assert_eq!(
                codec_for_encoding(&detected),
                encoding,
                "failed to detect {} (reported {detected})",
                encoding.name()
            );
            assert_eq!(
                codec_for_encoding(&detected)
                    .decode_without_bom_handling(&bytes)
                    .0,
                text
            );
        }
    }

    #[test]
    fn opens_and_edits_big5_csv() {
        let path = temp_path("big5");
        fs::write(
            &path,
            legacy_fixture(
                "姓名,城市,備註\r\n陳大文,臺北,繁體中文資料\r\n林美玲,高雄,測試內容",
                BIG5,
            ),
        )
        .expect("Big5 fixture should be writable");

        let mut engine = CsvEngine::new();
        let info = engine
            .open(path.to_str().expect("temporary path should be valid UTF-8"))
            .expect("Big5 fixture should open");
        assert_eq!(info.encoding, BIG5.name());
        assert_eq!(info.headers, ["姓名", "城市", "備註"]);
        assert_eq!(
            engine.get_rows(0, 1, 0, 3).expect("Big5 row should decode")[0].cells,
            ["陳大文", "臺北", "繁體中文資料"]
        );

        engine
            .update_cell(0, 1, "臺中")
            .expect("Big5 cell should be editable");
        let edited = fs::read(&path).expect("edited Big5 fixture should be readable");
        assert_eq!(
            BIG5.decode_without_bom_handling(&edited).0,
            "姓名,城市,備註\r\n陳大文,臺中,繁體中文資料\r\n林美玲,高雄,測試內容"
        );

        engine.close();
        fs::remove_file(path).expect("fixture should be removable");
    }

    #[test]
    fn parses_quoted_delimiter_and_escaped_quote() {
        assert_eq!(
            parse_csv_line("alpha,\"bravo,charlie\",\"say \"\"hi\"\"\"\r\n", ','),
            vec!["alpha", "bravo,charlie", "say \"hi\""]
        );
    }

    #[test]
    fn detects_delimiter_outside_quotes() {
        assert_eq!(detect_delimiter("name;\"notes,with,commas\";value"), ';');
        assert_eq!(detect_delimiter("plain text"), ',');
    }

    #[test]
    fn uses_compact_offsets_for_files_up_to_four_gibibytes() {
        let offsets = build_index(b"header\none\ntwo\n", 0, "utf8");
        assert!(matches!(offsets, RowOffsets::U32(_)));
        assert_eq!(offsets.get(2), 11);

        if usize::BITS > 32 {
            let offsets = RowOffsets::with_capacity(u32::MAX as usize + 1, 0);
            assert!(matches!(offsets, RowOffsets::U64(_)));
        }
    }

    #[test]
    fn cache_respects_row_and_byte_budgets() {
        let mut engine = CsvEngine::new();
        for key in 0..CACHE_MAX_ROWS as u32 + 10 {
            engine.add_to_cache(key, vec![String::new()]);
        }
        assert_eq!(engine.cache.len(), CACHE_MAX_ROWS);
        assert!(engine.cache_bytes <= CACHE_MAX_BYTES);

        engine.add_to_cache(u32::MAX, vec!["x".repeat(CACHE_MAX_BYTES)]);
        assert!(engine.cache_bytes <= CACHE_MAX_BYTES);
        assert!(!engine.cache.contains_key(&u32::MAX));
    }

    #[test]
    fn row_data_contains_only_requested_columns() {
        let cells = vec![
            "zero".to_string(),
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
        ];
        let row = row_data_for_columns(&cells, 1, 2);
        assert_eq!(row.cells, ["one", "two"]);
        assert_eq!(row.lengths, [3, 3]);
    }

    #[test]
    fn truncates_unicode_at_character_boundary() {
        let text = "a".repeat(499) + "中tail";
        let truncated = truncate_cell(&text, 500);
        assert_eq!(truncated, "a".repeat(499));
    }

    #[test]
    fn extracts_case_insensitive_unicode_context_safely() {
        let text = "前".repeat(120) + "TARGET" + &"后".repeat(120);
        let context = extract_match_context(&text, "target", Some("target"));
        assert!(context.contains("TARGET"));
        assert!(context.starts_with("..."));
        assert!(context.ends_with("..."));
    }

    #[test]
    fn filtered_search_counts_only_matching_column() {
        let path = temp_path("search");
        fs::write(&path, "target,other\nnone,needle\nneedle,value\n")
            .expect("fixture should be writable");

        let mut engine = CsvEngine::new();
        engine
            .open(path.to_str().expect("temporary path should be valid UTF-8"))
            .expect("fixture should open");
        let pool = ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("test pool should build");
        let results = pool
            .install(|| engine.search_with_progress("needle", Some(0), false, 1, |_, _| {}))
            .expect("search should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_index, 1);

        engine.close();
        fs::remove_file(path).expect("fixture should be removable");
    }

    #[test]
    fn update_cell_streams_utf8_and_preserves_line_endings() {
        let path = temp_path("update-utf8");
        fs::write(&path, b"first,second\r\none,two\r\nthree,four")
            .expect("fixture should be writable");

        let mut engine = CsvEngine::new();
        engine
            .open(path.to_str().expect("temporary path should be valid UTF-8"))
            .expect("fixture should open");
        engine
            .update_cell(0, 1, "new,value")
            .expect("cell update should succeed");
        engine.close();

        assert_eq!(
            fs::read(&path).expect("fixture should be readable"),
            b"first,second\r\none,\"new,value\"\r\nthree,four"
        );
        fs::remove_file(path).expect("fixture should be removable");
    }

    #[test]
    fn update_cell_preserves_utf16_endianness_and_bom() {
        for little_endian in [true, false] {
            let label = if little_endian { "utf16le" } else { "utf16be" };
            let path = temp_path(label);
            fs::write(
                &path,
                utf16_fixture("first,second\r\n一,二\r\n三,四", little_endian),
            )
            .expect("fixture should be writable");

            let mut engine = CsvEngine::new();
            engine
                .open(path.to_str().expect("temporary path should be valid UTF-8"))
                .expect("fixture should open");
            engine
                .update_cell(0, 1, "新,值")
                .expect("cell update should succeed");
            engine.close();

            assert_eq!(
                fs::read(&path).expect("fixture should be readable"),
                utf16_fixture("first,second\r\n一,\"新,值\"\r\n三,四", little_endian)
            );
            fs::remove_file(path).expect("fixture should be removable");
        }
    }

    #[test]
    fn utf16_index_ignores_newlines_inside_quotes() {
        for (little_endian, encoding) in [(true, "utf16le"), (false, "utf16be")] {
            let bytes = utf16_fixture(
                "first,second\r\n\"multi\r\nline\",value\r\nlast,row",
                little_endian,
            );
            let offsets = build_index(&bytes, 2, encoding);
            assert_eq!(offsets.len(), 3);
            assert_eq!(
                parse_csv_line(&decode_row_bytes(&bytes, &offsets, 1, encoding), ',',),
                ["multi\r\nline", "value"]
            );
        }
    }

    fn decode_row_bytes(
        bytes: &[u8],
        offsets: &RowOffsets,
        row_index: usize,
        encoding: &str,
    ) -> String {
        let start = offsets.get(row_index) as usize;
        let end = if row_index + 1 < offsets.len() {
            offsets.get(row_index + 1) as usize
        } else {
            bytes.len()
        };
        let units: Vec<u16> = bytes[start..end]
            .chunks_exact(2)
            .map(|chunk| {
                if encoding == "utf16le" {
                    u16::from_le_bytes([chunk[0], chunk[1]])
                } else {
                    u16::from_be_bytes([chunk[0], chunk[1]])
                }
            })
            .collect();
        String::from_utf16(&units).expect("fixture should contain valid UTF-16")
    }
}
