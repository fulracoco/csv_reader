use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const MAX_CELL_PREVIEW: usize = 500;
const CACHE_MAX: usize = 500;

// ─── Public data types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenResult {
    pub file_path: String,
    pub file_name: String,
    pub file_size: u64,
    pub row_count: u32,
    pub column_count: u32,
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowData {
    pub cells: Vec<String>,
    pub lengths: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub row_index: u32,
    pub matches: Vec<CellMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellMatch {
    pub col_index: u32,
    pub col_name: String,
    pub cell_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchProgress {
    pub done: u32,
    pub total: u32,
}

// ─── CsvEngine ───────────────────────────────────────────────────────────────

pub struct CsvEngine {
    mmap: Option<Arc<Mmap>>,
    file_path: String,
    file_size: u64,
    offsets: Vec<u64>,
    headers: Vec<String>,
    delimiter: u8,
    encoding: String,
    bom_offset: u64,
    cache: HashMap<u32, Vec<String>>,
    cache_order: Vec<u32>,
}

impl CsvEngine {
    pub fn new() -> Self {
        CsvEngine {
            mmap: None,
            file_path: String::new(),
            file_size: 0,
            offsets: Vec::new(),
            headers: Vec::new(),
            delimiter: b',',
            encoding: String::from("utf8"),
            bom_offset: 0,
            cache: HashMap::with_capacity(CACHE_MAX),
            cache_order: Vec::with_capacity(CACHE_MAX),
        }
    }

    pub fn open(&mut self, file_path: &str) -> Result<OpenResult, String> {
        self.close();

        let file =
            File::open(file_path).map_err(|e| format!("Cannot open file: {}", e))?;

        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| format!("Cannot mmap file: {}", e))?
        };

        let file_size = mmap.len() as u64;
        let (encoding, bom_offset) = detect_bom(&mmap);

        let offsets = build_index(&mmap, bom_offset, &encoding);

        if offsets.is_empty() {
            self.mmap = Some(Arc::new(mmap));
            self.file_path = file_path.to_string();
            self.file_size = file_size;
            self.offsets = Vec::new();
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
        self.offsets = offsets;
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
        })
    }

    pub fn get_rows(&mut self, start_row: u32, count: u32) -> Result<Vec<RowData>, String> {
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
                    let cells: Vec<String> = cached
                        .iter()
                        .map(|c| truncate_cell(c, MAX_CELL_PREVIEW))
                        .collect();
                    let lengths: Vec<u32> = cached.iter().map(|c| c.len() as u32).collect();
                    results.push(RowData { cells, lengths });
                } else {
                    let text = read_row_text(
                        mmap,
                        &self.offsets,
                        row_idx as usize,
                        self.file_size,
                        &self.encoding,
                    );
                    let parsed = parse_csv_line(&text, delimiter);
                    let cells: Vec<String> = parsed
                        .iter()
                        .map(|c| truncate_cell(c, MAX_CELL_PREVIEW))
                        .collect();
                    let lengths: Vec<u32> = parsed.iter().map(|c| c.len() as u32).collect();
                    cache_updates.push((row_idx, Some(parsed)));
                    results.push(RowData { cells, lengths });
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

        let row_count = self.offsets.len().saturating_sub(1);

        let mut lines: Vec<String> = {
            let mmap = self.mmap.as_deref().ok_or("No file open".to_string())?;
            let mut lines = Vec::with_capacity(row_count + 1);
            for i in 0..=row_count {
                let text = read_row_text(mmap, &self.offsets, i, self.file_size, &self.encoding);
                let text = text
                    .trim_end_matches(|c: char| c == '\r' || c == '\n')
                    .to_string();
                lines.push(text);
            }
            lines
        };

        let mut parsed = parse_csv_line(&lines[data_row], delimiter);
        while parsed.len() <= col_index as usize {
            parsed.push(String::new());
        }
        parsed[col_index as usize] = new_content.to_string();
        lines[data_row] = format_csv_row(&parsed, delimiter);

        self.mmap = None;

        {
            let mut file = BufWriter::new(
                File::create(&self.file_path)
                    .map_err(|e| format!("Cannot write file: {}", e))?,
            );

            if self.encoding == "utf16le" {
                file.write_all(&[0xFF, 0xFE])
                    .map_err(|e| format!("Write error: {}", e))?;
            } else {
                file.write_all(&[0xEF, 0xBB, 0xBF])
                    .map_err(|e| format!("Write error: {}", e))?;
            }

            for (i, line) in lines.iter().enumerate() {
                if i == self.offsets.len() - 1 && line.is_empty() {
                    continue;
                }
                if self.encoding == "utf16le" {
                    let encoded: Vec<u8> =
                        line.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
                    file.write_all(&encoded)
                        .map_err(|e| format!("Write error: {}", e))?;
                    file.write_all(&[0x0A, 0x00])
                        .map_err(|e| format!("Write error: {}", e))?;
                } else {
                    file.write_all(line.as_bytes())
                        .map_err(|e| format!("Write error: {}", e))?;
                    file.write_all(b"\n")
                        .map_err(|e| format!("Write error: {}", e))?;
                }
            }
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
            File::create(output_path)
                .map_err(|e| format!("Cannot create file: {}", e))?,
        );

        file.write_all(&[0xEF, 0xBB, 0xBF])
            .map_err(|e| format!("Write error: {}", e))?;

        let header_line: Vec<String> = col_indices
            .iter()
            .map(|&i| {
                let h = self.headers.get(i as usize).cloned().unwrap_or_default();
                csv_quote_value(&h)
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
            let text =
                read_row_text(mmap, &self.offsets, data_row, self.file_size, &self.encoding);
            let parsed = parse_csv_line(&text, delimiter);
            let line: Vec<String> = col_indices
                .iter()
                .map(|&i| {
                    let val = parsed.get(i as usize).cloned().unwrap_or_default();
                    csv_quote_value(&val)
                })
                .collect();
            file.write_all(line.join(",").as_bytes())
                .map_err(|e| format!("Write error: {}", e))?;
            file.write_all(b"\n")
                .map_err(|e| format!("Write error: {}", e))?;
        }

        file.flush()
            .map_err(|e| format!("Flush error: {}", e))?;

        Ok(())
    }

    pub fn close(&mut self) {
        self.mmap = None;
        self.cache.clear();
        self.cache_order.clear();
        self.offsets.clear();
        self.headers.clear();
        self.file_path.clear();
        self.file_size = 0;
        self.delimiter = b',';
        self.encoding = String::from("utf8");
        self.bom_offset = 0;
    }

    pub fn is_open(&self) -> bool {
        self.mmap.is_some()
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    pub fn row_count(&self) -> u32 {
        self.offsets.len().saturating_sub(1) as u32
    }

    pub fn column_count(&self) -> u32 {
        self.headers.len() as u32
    }

    pub fn search_with_progress(
        &self,
        query: &str,
        col_filter: Option<u32>,
        case_sensitive: bool,
        on_progress: impl Fn(u32, u32) + Send + Sync,
    ) -> Result<Vec<SearchResult>, String> {
        let mmap = Arc::clone(self.mmap.as_ref().ok_or("No file open")?);
        let offsets = self.offsets.clone();
        let encoding = self.encoding.clone();
        let delimiter = self.delimiter;
        let file_size = self.file_size;
        let headers = self.headers.clone();

        if offsets.len() <= 1 || query.is_empty() {
            return Ok(Vec::new());
        }

        let total = (offsets.len() - 1) as u32;
        let query_owned = query.to_string();

        let is_utf8 = encoding == "utf8";
        let query_is_ascii = query.chars().all(|c| c.is_ascii());
        let can_use_raw = is_utf8 && case_sensitive && query_is_ascii;
        let query_bytes = if can_use_raw {
            Some(query.as_bytes().to_vec())
        } else {
            None
        };

        let counter = AtomicU32::new(0);

        let results: Vec<SearchResult> = (1..offsets.len())
            .into_par_iter()
            .filter_map(|i| {
                let start = offsets[i] as usize;
                let end = if i + 1 < offsets.len() {
                    offsets[i + 1] as usize
                } else {
                    file_size as usize
                };
                if end <= start || start >= mmap.len() {
                    return None;
                }
                let row_bytes = &mmap[start..end.min(mmap.len())];

                let row_contains = if let Some(ref qb) = query_bytes {
                    row_bytes
                        .windows(qb.len())
                        .any(|w| w == qb.as_slice())
                } else if is_utf8 {
                    let row_text = String::from_utf8_lossy(row_bytes);
                    if case_sensitive {
                        row_text.contains(&query_owned)
                    } else {
                        row_text.to_lowercase().contains(&query_owned.to_lowercase())
                    }
                } else {
                    let row_text = read_row_text(&mmap, &offsets, i, file_size, &encoding);
                    if row_text.is_empty() {
                        return None;
                    }
                    if case_sensitive {
                        row_text.contains(&query_owned)
                    } else {
                        row_text.to_lowercase().contains(&query_owned.to_lowercase())
                    }
                };

                if !row_contains {
                    return None;
                }

                // Row matched — decode and find matching cells
                let row_text = read_row_text(&mmap, &offsets, i, file_size, &encoding);
                let cells = parse_csv_line(&row_text, delimiter as char);

                let matches: Vec<CellMatch> = if let Some(ci) = col_filter {
                    let ci = ci as usize;
                    if let Some(cell_text) = cells.get(ci) {
                        let found = if case_sensitive {
                            cell_text.contains(&query_owned)
                        } else {
                            cell_text.to_lowercase().contains(&query_owned.to_lowercase())
                        };
                        if found {
                            vec![CellMatch {
                                col_index: ci as u32,
                                col_name: headers.get(ci).cloned().unwrap_or_default(),
                                cell_text: extract_match_context(cell_text, &query_owned, case_sensitive),
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
                            let found = if case_sensitive {
                                cell_text.contains(&query_owned)
                            } else {
                                cell_text.to_lowercase().contains(&query_owned.to_lowercase())
                            };
                            if found {
                                Some(CellMatch {
                                    col_index: ci as u32,
                                    col_name: headers.get(ci).cloned().unwrap_or_default(),
                                    cell_text: extract_match_context(cell_text, &query_owned, case_sensitive),
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

                let done = counter.fetch_add(1, Ordering::Relaxed) + 1;
                if done % 4096 == 0 || done == total {
                    on_progress(done, total);
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
        self.cache.insert(key, data);
        if let Some(pos) = self.cache_order.iter().position(|&k| k == key) {
            self.cache_order.remove(pos);
        }
        self.cache_order.push(key);
        while self.cache_order.len() > CACHE_MAX {
            if let Some(oldest) = self.cache_order.first().copied() {
                self.cache_order.remove(0);
                self.cache.remove(&oldest);
            }
        }
    }

    fn reopen(&mut self) -> Result<(), String> {
        self.cache.clear();
        self.cache_order.clear();
        self.offsets.clear();

        let file =
            File::open(&self.file_path).map_err(|e| format!("Cannot reopen file: {}", e))?;

        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| format!("Cannot mmap file: {}", e))?
        };

        self.file_size = mmap.len() as u64;
        self.offsets = build_index(&mmap, self.bom_offset, &self.encoding);
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

fn truncate_cell(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        text[..max_len].to_string()
    } else {
        text.to_string()
    }
}

fn extract_match_context(cell_text: &str, query: &str, case_sensitive: bool) -> String {
    let match_pos = if case_sensitive {
        cell_text.find(query)
    } else {
        let lower = cell_text.to_lowercase();
        let ql = query.to_lowercase();
        lower.find(&ql)
    };

    let mpos = match match_pos {
        Some(pos) => pos,
        None => return truncate_cell(cell_text, 200),
    };

    let qlen = query.len();
    let radius = 90usize;
    let cell_len = cell_text.len();

    let preview_start = mpos.saturating_sub(radius);
    let preview_end = (mpos + qlen + radius).min(cell_len);

    let mut preview = cell_text[preview_start..preview_end].to_string();

    if preview_start > 0 {
        preview.insert_str(0, "...");
    }
    if preview_end < cell_len {
        preview.push_str("...");
    }

    if preview.len() > 250 {
        preview = truncate_cell(&preview, 250);
    }

    preview
}

fn detect_bom(data: &[u8]) -> (String, u64) {
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

fn build_index(data: &[u8], bom_offset: u64, encoding: &str) -> Vec<u64> {
    let file_size = data.len();
    let mut offsets: Vec<u64> = Vec::with_capacity(65536);
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

    while offsets.len() > 1 && offsets[offsets.len() - 1] >= file_size as u64 {
        offsets.pop();
    }

    offsets
}

fn build_index_utf8(data: &[u8], start: usize, offsets: &mut Vec<u64>, file_size: usize) {
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

fn build_index_utf16le(data: &[u8], start: usize, offsets: &mut Vec<u64>, file_size: usize) {
    let mut pos = start;
    if pos % 2 != 0 && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let lo = data[pos];
        let hi = data[pos + 1];

        if lo == 0x0A && hi == 0x00 {
            let offset = (pos + 2) as u64;
            if offset < file_size as u64 {
                offsets.push(offset);
            }
        } else if lo == 0x0D && hi == 0x00 {
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

fn build_index_utf16be(data: &[u8], start: usize, offsets: &mut Vec<u64>, file_size: usize) {
    let mut pos = start;
    if pos % 2 != 0 && pos + 1 < file_size {
        pos += 1;
    }

    while pos + 1 < file_size {
        let hi = data[pos];
        let lo = data[pos + 1];

        if hi == 0x00 && lo == 0x0A {
            let offset = (pos + 2) as u64;
            if offset < file_size as u64 {
                offsets.push(offset);
            }
        } else if hi == 0x00 && lo == 0x0D {
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
    offsets: &[u64],
    row_index: usize,
    file_size: u64,
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
