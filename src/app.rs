use crate::csv_engine::{CsvEngine, OpenResult, RowData, SearchProgress, SearchResult};
use eframe::egui::{
    self, Align, Color32, Context, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily,
    FontId, Key, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, TextEdit, TextStyle, Ui,
    Vec2,
};
use rfd::FileDialog;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

const ROW_HEADER_WIDTH: f32 = 64.0;
const HEADER_HEIGHT: f32 = 31.0;
const MIN_COL_WIDTH: f32 = 96.0;
const MAX_COL_WIDTH: f32 = 420.0;
const MAX_MANUAL_COL_WIDTH: f32 = 1_200.0;
const MAX_PHYSICAL_EXTENT: f64 = 12_000_000.0;
const MAX_VISIBLE_ROWS: u32 = 180;
const MAX_VISIBLE_COLS: u32 = 80;
const MIN_PREFETCH_ROWS: u32 = 24;
const APP_BG: Color32 = Color32::from_rgb(13, 17, 18);
const PANEL_BG: Color32 = Color32::from_rgb(20, 25, 27);
const SURFACE_BG: Color32 = Color32::from_rgb(25, 31, 33);
const RAISED_BG: Color32 = Color32::from_rgb(31, 38, 40);
const TABLE_BG: Color32 = Color32::from_rgb(15, 20, 21);
const GRID_LINE: Color32 = Color32::from_rgb(38, 47, 49);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(232, 238, 235);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(164, 177, 172);
const TEXT_MUTED: Color32 = Color32::from_rgb(112, 126, 121);
const ACCENT: Color32 = Color32::from_rgb(70, 196, 137);
const WARNING: Color32 = Color32::from_rgb(232, 175, 93);
const DANGER: Color32 = Color32::from_rgb(235, 116, 116);
const SETTINGS_LANGUAGE_KEY: &str = "language";
const SETTINGS_ACCENT_KEY: &str = "accent_color";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    English,
    Chinese,
}

impl Language {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "en" => Some(Self::English),
            "zh-CN" => Some(Self::Chinese),
            _ => None,
        }
    }

    fn storage_value(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh-CN",
        }
    }

    fn text(self, english: &'static str, chinese: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Chinese => chinese,
        }
    }

    fn is_chinese(self) -> bool {
        self == Self::Chinese
    }
}

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CSV Reader")
            .with_decorations(false)
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CSV Reader",
        options,
        Box::new(|cc| Ok(Box::new(CsvApp::new(cc)))),
    )
}

#[derive(Debug)]
enum WorkerMessage {
    Open(Result<OpenResult, String>),
    SearchProgress(SearchProgress),
    Search(Result<Vec<SearchResult>, String>),
    Cell(Result<(u32, u32, String), String>),
    Edit(Result<(u32, u32), String>),
    Export(Result<String, String>),
}

#[derive(Default)]
struct RowSelection {
    anchor: Option<u32>,
    range: Option<(u32, u32)>,
    toggled: BTreeSet<u32>,
}

impl RowSelection {
    fn clear(&mut self) {
        self.anchor = None;
        self.range = None;
        self.toggled.clear();
    }

    fn click(&mut self, row: u32, shift: bool, toggle: bool) {
        if shift {
            let anchor = self.anchor.unwrap_or(row);
            self.range = Some((anchor.min(row), anchor.max(row)));
            self.toggled.clear();
        } else if toggle {
            if self.range.is_some() {
                self.range = None;
            }
            if !self.toggled.insert(row) {
                self.toggled.remove(&row);
            }
            self.anchor = Some(row);
        } else {
            self.range = Some((row, row));
            self.toggled.clear();
            self.anchor = Some(row);
        }
    }

    fn contains(&self, row: u32) -> bool {
        self.range
            .map(|(start, end)| row >= start && row <= end)
            .unwrap_or(false)
            || self.toggled.contains(&row)
    }

    fn bounds(&self, total: u32) -> Option<(u32, u32)> {
        let mut bounds = self.range;
        for row in &self.toggled {
            bounds = Some(match bounds {
                Some((start, end)) => (start.min(*row), end.max(*row)),
                None => (*row, *row),
            });
        }
        bounds.map(|(start, end)| {
            (
                start.min(total.saturating_sub(1)),
                end.min(total.saturating_sub(1)),
            )
        })
    }
}

#[derive(Default)]
struct ColumnSelection {
    anchor: Option<u32>,
    range: Option<(u32, u32)>,
    toggled: BTreeSet<u32>,
}

impl ColumnSelection {
    fn clear(&mut self) {
        self.anchor = None;
        self.range = None;
        self.toggled.clear();
    }

    fn click(&mut self, col: u32, shift: bool, toggle: bool) {
        if shift {
            let anchor = self.anchor.unwrap_or(col);
            self.range = Some((anchor.min(col), anchor.max(col)));
            self.toggled.clear();
        } else if toggle {
            if self.range.is_some() {
                self.range = None;
            }
            if !self.toggled.insert(col) {
                self.toggled.remove(&col);
            }
            self.anchor = Some(col);
        } else {
            self.range = Some((col, col));
            self.toggled.clear();
            self.anchor = Some(col);
        }
    }

    fn contains(&self, col: u32) -> bool {
        self.range
            .map(|(start, end)| col >= start && col <= end)
            .unwrap_or(false)
            || self.toggled.contains(&col)
    }

    fn count(&self) -> u32 {
        self.range
            .map(|(start, end)| end.saturating_sub(start) + 1)
            .unwrap_or(self.toggled.len() as u32)
    }
}

struct ExportState {
    columns: Vec<bool>,
    from: String,
    to: String,
    error: String,
}

struct CsvApp {
    language: Language,
    accent_color: Color32,
    engine: Arc<Mutex<CsvEngine>>,
    tx: mpsc::Sender<WorkerMessage>,
    rx: mpsc::Receiver<WorkerMessage>,
    info: Option<Arc<OpenResult>>,
    rows: BTreeMap<u32, RowData>,
    row_height: f32,
    column_widths: Vec<f32>,
    display_column_widths: Arc<[f32]>,
    display_column_offsets: Arc<[f64]>,
    layout_available_width: f32,
    column_layout_dirty: bool,
    row_selection: RowSelection,
    col_selection: ColumnSelection,
    selected_cell: Option<(u32, u32)>,
    detail_text: String,
    editing: bool,
    busy: Option<String>,
    error: Option<String>,
    search_query: String,
    search_column: Option<u32>,
    search_case_sensitive: bool,
    search_results: Vec<SearchResult>,
    search_status: String,
    show_search: bool,
    show_detail: bool,
    export: Option<ExportState>,
    jump_to_row: Option<u32>,
    last_view: Option<(u32, u32, u32, u32)>,
}

impl CsvApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_fonts(&cc.egui_ctx);
        let language = cc
            .storage
            .and_then(|storage| storage.get_string(SETTINGS_LANGUAGE_KEY))
            .as_deref()
            .and_then(Language::parse)
            .unwrap_or(Language::English);
        let accent_color = cc
            .storage
            .and_then(|storage| storage.get_string(SETTINGS_ACCENT_KEY))
            .as_deref()
            .and_then(parse_color)
            .unwrap_or(ACCENT);
        configure_style(&cc.egui_ctx, accent_color);
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            language,
            accent_color,
            engine: Arc::new(Mutex::new(CsvEngine::new())),
            tx,
            rx,
            info: None,
            rows: BTreeMap::new(),
            row_height: 32.0,
            column_widths: Vec::new(),
            display_column_widths: Arc::from(Vec::<f32>::new()),
            display_column_offsets: Arc::from(Vec::<f64>::new()),
            layout_available_width: 0.0,
            column_layout_dirty: true,
            row_selection: RowSelection::default(),
            col_selection: ColumnSelection::default(),
            selected_cell: None,
            detail_text: String::new(),
            editing: false,
            busy: None,
            error: None,
            search_query: String::new(),
            search_column: None,
            search_case_sensitive: false,
            search_results: Vec::new(),
            search_status: String::new(),
            show_search: false,
            show_detail: false,
            export: None,
            jump_to_row: None,
            last_view: None,
        };
        if let Some(path) = std::env::args().nth(1) {
            app.spawn_open(path, &cc.egui_ctx);
        }
        app
    }

    fn accent_hover(&self) -> Color32 {
        mix_color(self.accent_color, Color32::WHITE, 0.14)
    }

    fn accent_dark(&self) -> Color32 {
        mix_color(APP_BG, self.accent_color, 0.34)
    }

    fn apply_accent(&mut self, color: Color32, ctx: &Context) {
        self.accent_color = Color32::from_rgb(color.r(), color.g(), color.b());
        configure_style(ctx, self.accent_color);
        ctx.request_repaint();
    }

    fn spawn_open(&mut self, path: String, ctx: &Context) {
        self.busy = Some(if self.language.is_chinese() {
            format!("正在打开 {}...", Path::new(&path).display())
        } else {
            format!("Opening {}...", Path::new(&path).display())
        });
        self.error = None;
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = engine
                .lock()
                .map_err(|_| "Engine lock poisoned".to_string())
                .and_then(|mut e| e.open(&path));
            let _ = tx.send(WorkerMessage::Open(result));
            repaint.request_repaint();
        });
    }

    fn choose_open(&mut self, ctx: &Context) {
        if self.busy.is_some() {
            return;
        }
        if let Some(path) = FileDialog::new()
            .add_filter(
                self.language.text("CSV files", "CSV 文件"),
                &["csv", "tsv", "txt"],
            )
            .pick_file()
        {
            self.spawn_open(path.to_string_lossy().into_owned(), ctx);
        }
    }

    fn handle_messages(&mut self, ctx: &Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::Open(result) => match result {
                    Ok(info) => {
                        self.column_widths = info
                            .headers
                            .iter()
                            .map(|header| estimated_column_width(header, false))
                            .collect();
                        self.column_widths
                            .resize(info.column_count as usize, MIN_COL_WIDTH);
                        self.column_layout_dirty = true;
                        self.info = Some(Arc::new(info));
                        self.rows.clear();
                        self.row_selection.clear();
                        self.col_selection.clear();
                        self.selected_cell = None;
                        self.detail_text.clear();
                        self.busy = None;
                        self.show_detail = false;
                    }
                    Err(error) => {
                        self.busy = None;
                        self.error = Some(error);
                    }
                },
                WorkerMessage::SearchProgress(progress) => {
                    self.search_status = if self.language.is_chinese() {
                        format!("正在搜索... {} / {}", progress.done, progress.total)
                    } else {
                        format!("Searching... {} / {}", progress.done, progress.total)
                    };
                    ctx.request_repaint();
                }
                WorkerMessage::Search(result) => {
                    self.busy = None;
                    match result {
                        Ok(results) => {
                            self.search_status = if self.language.is_chinese() {
                                format!("{} 条结果", results.len())
                            } else {
                                format!("{} result(s)", results.len())
                            };
                            self.search_results = results;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                WorkerMessage::Cell(result) => match result {
                    Ok((row, col, content)) => {
                        if self.selected_cell == Some((row, col)) {
                            self.detail_text = content;
                            self.show_detail = true;
                        }
                    }
                    Err(error) => self.error = Some(error),
                },
                WorkerMessage::Edit(result) => {
                    self.busy = None;
                    match result {
                        Ok((row, col)) => {
                            self.editing = false;
                            self.rows.clear();
                            self.selected_cell = Some((row, col));
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                WorkerMessage::Export(result) => {
                    self.busy = None;
                    match result {
                        Ok(_path) => self.export = None,
                        Err(error) => {
                            if let Some(export) = &mut self.export {
                                export.error = error;
                            }
                        }
                    }
                }
            }
        }
    }

    fn start_search(&mut self, ctx: &Context) {
        let Some(info) = &self.info else { return };
        let query = self.search_query.trim().to_string();
        if query.is_empty() || self.busy.is_some() {
            return;
        }
        self.busy = Some(
            self.language
                .text("Searching...", "正在搜索...")
                .to_string(),
        );
        self.search_results.clear();
        self.search_status = self
            .language
            .text("Searching...", "正在搜索...")
            .to_string();
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        let col = self.search_column;
        let case_sensitive = self.search_case_sensitive;
        let _row_count = info.row_count;
        thread::spawn(move || {
            let result = engine
                .lock()
                .map_err(|_| "Engine lock poisoned".to_string())
                .and_then(|engine| {
                    engine.search_with_progress(&query, col, case_sensitive, 500, {
                        let tx = tx.clone();
                        let repaint = repaint.clone();
                        move |done, total| {
                            let _ = tx.send(WorkerMessage::SearchProgress(SearchProgress {
                                done,
                                total,
                            }));
                            repaint.request_repaint();
                        }
                    })
                });
            let _ = tx.send(WorkerMessage::Search(result));
            repaint.request_repaint();
        });
    }

    fn load_cell(&mut self, row: u32, col: u32, ctx: &Context) {
        self.selected_cell = Some((row, col));
        self.detail_text.clear();
        self.show_detail = true;
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = engine
                .lock()
                .map_err(|_| "Engine lock poisoned".to_string())
                .and_then(|mut e| e.get_cell_content(row, col).map(|text| (row, col, text)));
            let _ = tx.send(WorkerMessage::Cell(result));
            repaint.request_repaint();
        });
    }

    fn save_edit(&mut self, ctx: &Context) {
        let Some((row, col)) = self.selected_cell else {
            return;
        };
        if !self.editing || self.busy.is_some() {
            return;
        }
        let value = self.detail_text.clone();
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        self.busy = Some(self.language.text("Saving...", "正在保存...").to_string());
        thread::spawn(move || {
            let result = engine
                .lock()
                .map_err(|_| "Engine lock poisoned".to_string())
                .and_then(|mut e| e.update_cell(row, col, &value).map(|_| (row, col)));
            let _ = tx.send(WorkerMessage::Edit(result));
            repaint.request_repaint();
        });
    }

    fn open_export(&mut self) {
        let Some(info) = &self.info else { return };
        let has_column_selection =
            self.col_selection.range.is_some() || !self.col_selection.toggled.is_empty();
        self.export = Some(ExportState {
            columns: (0..info.column_count)
                .map(|col| !has_column_selection || self.col_selection.contains(col))
                .collect(),
            from: self
                .row_selection
                .bounds(info.row_count)
                .map(|(start, _)| (start + 1).to_string())
                .unwrap_or_else(|| "1".to_string()),
            to: self
                .row_selection
                .bounds(info.row_count)
                .map(|(_, end)| (end + 1).to_string())
                .unwrap_or_else(|| info.row_count.to_string()),
            error: String::new(),
        });
    }

    fn export(&mut self, ctx: &Context) {
        let Some(info) = &self.info else { return };
        let Some(state) = &self.export else { return };
        let columns: Vec<u32> = state
            .columns
            .iter()
            .enumerate()
            .filter_map(|(i, checked)| checked.then_some(i as u32))
            .collect();
        let from = state
            .from
            .parse::<u32>()
            .ok()
            .filter(|v| *v >= 1)
            .map(|v| v - 1);
        let to = state
            .to
            .parse::<u32>()
            .ok()
            .filter(|v| *v >= 1)
            .map(|v| v - 1);
        if columns.is_empty() {
            if let Some(state) = &mut self.export {
                state.error = self
                    .language
                    .text("Select at least one column.", "请至少选择一列。")
                    .to_string();
            }
            return;
        }
        let (Some(from), Some(to)) = (from, to) else {
            if let Some(state) = &mut self.export {
                state.error = self
                    .language
                    .text("Enter a valid row range.", "请输入有效的行范围。")
                    .to_string();
            }
            return;
        };
        if from > to || to >= info.row_count {
            if let Some(state) = &mut self.export {
                state.error = self
                    .language
                    .text("Row range is outside the file.", "行范围超出文件内容。")
                    .to_string();
            }
            return;
        }
        let default_name = Path::new(&info.file_path)
            .file_stem()
            .map(|n| format!("{}_export.csv", n.to_string_lossy()))
            .unwrap_or_else(|| "export.csv".to_string());
        let Some(path) = FileDialog::new()
            .set_file_name(default_name)
            .add_filter(self.language.text("CSV files", "CSV 文件"), &["csv"])
            .save_file()
        else {
            return;
        };
        self.busy = Some(
            self.language
                .text("Exporting...", "正在导出...")
                .to_string(),
        );
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        let path_string = path.to_string_lossy().into_owned();
        thread::spawn(move || {
            let result = engine
                .lock()
                .map_err(|_| "Engine lock poisoned".to_string())
                .and_then(|e| {
                    e.export_csv(&path_string, &columns, from, to)
                        .map(|_| path_string.clone())
                });
            let _ = tx.send(WorkerMessage::Export(result));
            repaint.request_repaint();
        });
    }

    fn table(&mut self, ui: &mut Ui, ctx: &Context) {
        let Some(info) = self.info.clone() else {
            return;
        };
        let available = ui.available_size();
        let data_height = (available.y - HEADER_HEIGHT).max(80.0);
        let (column_widths, column_offsets) = self.column_layout(
            info.column_count,
            (available.x - ROW_HEADER_WIDTH).max(MIN_COL_WIDTH),
        );
        let data_width = column_offsets.last().copied().unwrap_or(0.0);
        let physical_width =
            (f64::from(ROW_HEADER_WIDTH) + data_width).min(MAX_PHYSICAL_EXTENT) as f32;
        let physical_height = (HEADER_HEIGHT as f64
            + info.row_count as f64 * self.row_height as f64)
            .min(MAX_PHYSICAL_EXTENT) as f32;
        let viewport_width = available.x.max(100.0);
        let viewport_height = available.y.max(100.0);
        let logical_width = f64::from(ROW_HEADER_WIDTH) + data_width;
        let logical_height = HEADER_HEIGHT as f64 + info.row_count as f64 * self.row_height as f64;
        let max_logical_x = (logical_width - viewport_width as f64).max(0.0);
        let max_physical_x = (physical_width - viewport_width).max(0.0) as f64;
        let max_logical_y = (logical_height - data_height as f64).max(0.0);
        let max_physical_y = (physical_height - viewport_height).max(0.0) as f64;
        let jump = self.jump_to_row.take();

        let mut scroll_area = egui::ScrollArea::both()
            .id_salt("csv-grid")
            .wheel_scroll_multiplier(Vec2::new(
                wheel_scroll_scale(max_logical_x, max_physical_x),
                wheel_scroll_scale(max_logical_y, max_physical_y),
            ))
            .auto_shrink([false, false]);
        if let Some(row) = jump {
            let target = (HEADER_HEIGHT as f64 + row as f64 * self.row_height as f64
                - data_height as f64 * 0.35)
                .clamp(0.0, max_logical_y);
            let y = if max_logical_y > 0.0 {
                target / max_logical_y * max_physical_y
            } else {
                0.0
            };
            scroll_area = scroll_area.scroll_offset(Vec2::new(0.0, y as f32));
        }
        scroll_area.show_viewport(ui, |ui, viewport| {
            let (content_rect, _) =
                ui.allocate_exact_size(Vec2::new(physical_width, physical_height), Sense::hover());
            let logical_scroll_x = if max_physical_x > 0.0 {
                viewport.min.x as f64 / max_physical_x * max_logical_x
            } else {
                0.0
            };
            let logical_scroll_y = if max_physical_y > 0.0 {
                viewport.min.y as f64 / max_physical_y * max_logical_y
            } else {
                0.0
            };
            let first_row = ((logical_scroll_y - HEADER_HEIGHT as f64).max(0.0)
                / self.row_height as f64)
                .floor() as u32;
            let last_row =
                ((logical_scroll_y + data_height as f64) / self.row_height as f64).ceil() as u32;
            // The row-number column stays pinned, so the scroll offset maps directly
            // to the data columns rather than consuming the pinned column's width.
            let data_scroll = logical_scroll_x.max(0.0);
            let first_col = column_offsets
                .partition_point(|offset| *offset <= data_scroll)
                .saturating_sub(1)
                .min(info.column_count.saturating_sub(1) as usize)
                as u32;
            let data_view_width = (f64::from(viewport_width) - f64::from(ROW_HEADER_WIDTH))
                .max(f64::from(MIN_COL_WIDTH));
            let last_col = column_offsets
                .partition_point(|offset| *offset < data_scroll + data_view_width)
                .min(info.column_count as usize) as u32;
            let (row_start, row_end) = prefetched_row_range(first_row, last_row, info.row_count);
            let col_start = first_col.saturating_sub(2).min(info.column_count);
            let col_end = last_col.saturating_add(2).min(info.column_count);
            let widths_changed = self.load_rows(
                row_start,
                row_end.saturating_sub(row_start),
                col_start,
                col_end.saturating_sub(col_start),
            );
            if widths_changed {
                ctx.request_repaint();
            }
            self.last_view = Some((row_start, row_end, col_start, col_end));
            let painter = ui.painter_at(content_rect);
            let clip = ui.clip_rect();
            let header_y = clip.min.y;
            let row_x = clip.min.x;
            let first_offset = column_offsets
                .get(first_col as usize)
                .copied()
                .unwrap_or(0.0);
            let data_x = clip.min.x + ROW_HEADER_WIDTH - (data_scroll - first_offset) as f32;
            self.paint_header(
                ui,
                &painter,
                &info,
                &column_widths,
                header_y,
                row_x,
                data_x,
                first_col,
                last_col,
            );
            let y_origin = clip.min.y + HEADER_HEIGHT
                - (logical_scroll_y - HEADER_HEIGHT as f64).max(0.0) as f32 % self.row_height;
            for row_index in first_row..last_row.min(info.row_count) {
                let y = y_origin + (row_index.saturating_sub(first_row)) as f32 * self.row_height;
                self.paint_row(
                    ui,
                    &painter,
                    &info,
                    &column_widths,
                    row_index,
                    y,
                    row_x,
                    data_x,
                    col_start,
                    first_col,
                    last_col,
                    ctx,
                );
            }
        });
    }

    fn column_layout(
        &mut self,
        column_count: u32,
        available_width: f32,
    ) -> (Arc<[f32]>, Arc<[f64]>) {
        let size_changed = (self.layout_available_width - available_width).abs() > 0.5;
        if self.column_layout_dirty
            || size_changed
            || self.display_column_widths.len() != column_count as usize
        {
            let mut widths = self.column_widths.clone();
            widths.resize(column_count as usize, MIN_COL_WIDTH);
            widths.truncate(column_count as usize);
            let content_width: f32 = widths.iter().sum();
            if !widths.is_empty() && content_width < available_width {
                let extra = (available_width - content_width) / widths.len() as f32;
                for width in &mut widths {
                    *width += extra;
                }
            }

            let mut offsets = Vec::with_capacity(widths.len() + 1);
            offsets.push(0.0_f64);
            for width in &widths {
                let next = offsets.last().copied().unwrap_or(0.0) + f64::from(*width);
                offsets.push(next);
            }
            self.display_column_widths = Arc::from(widths);
            self.display_column_offsets = Arc::from(offsets);
            self.layout_available_width = available_width;
            self.column_layout_dirty = false;
        }
        (
            Arc::clone(&self.display_column_widths),
            Arc::clone(&self.display_column_offsets),
        )
    }

    fn reset_column_widths(&mut self) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        self.column_widths = info
            .headers
            .iter()
            .map(|header| estimated_column_width(header, false))
            .collect();
        self.column_widths
            .resize(info.column_count as usize, MIN_COL_WIDTH);
        self.column_layout_dirty = true;
        self.last_view = None;
    }

    fn load_rows(&mut self, start: u32, count: u32, col_start: u32, col_count: u32) -> bool {
        if count == 0 || col_count == 0 || self.busy.as_deref() == Some("Opening") {
            return false;
        }
        let end = start.saturating_add(count);
        let col_end = col_start.saturating_add(col_count);
        let columns_changed = self
            .last_view
            .is_none_or(|(_, _, previous_start, previous_end)| {
                previous_start != col_start || previous_end != col_end
            });
        if columns_changed {
            self.rows.clear();
        }

        let mut missing_ranges = Vec::new();
        let mut missing_start = None;
        for row in start..end {
            if self.rows.contains_key(&row) {
                if let Some(range_start) = missing_start.take() {
                    missing_ranges.push((range_start, row));
                }
            } else if missing_start.is_none() {
                missing_start = Some(row);
            }
        }
        if let Some(range_start) = missing_start {
            missing_ranges.push((range_start, end));
        }
        if missing_ranges.is_empty() {
            return false;
        }

        let Ok(mut engine) = self.engine.try_lock() else {
            return false;
        };
        let mut widths_changed = false;
        for (range_start, range_end) in missing_ranges {
            let range_count = range_end.saturating_sub(range_start);
            let Ok(data) = engine.get_rows(
                range_start,
                range_count.min(MAX_VISIBLE_ROWS),
                col_start,
                col_count.min(MAX_VISIBLE_COLS),
            ) else {
                continue;
            };
            for (offset, row) in data.into_iter().enumerate() {
                for (cell_offset, cell) in row.cells.iter().enumerate() {
                    let col = col_start as usize + cell_offset;
                    if let Some(width) = self.column_widths.get_mut(col) {
                        let measured = estimated_column_width(cell, true);
                        if measured > *width + 0.5 {
                            *width = measured;
                            widths_changed = true;
                        }
                    }
                }
                self.rows.insert(range_start + offset as u32, row);
            }
        }
        while self.rows.len() > 256 {
            let center = start.saturating_add(count / 2);
            if let Some(key) = self
                .rows
                .keys()
                .max_by_key(|row| row.abs_diff(center))
                .copied()
            {
                self.rows.remove(&key);
            } else {
                break;
            }
        }
        self.column_layout_dirty |= widths_changed;
        widths_changed
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_header(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        info: &OpenResult,
        column_widths: &[f32],
        y: f32,
        row_x: f32,
        data_x: f32,
        first_col: u32,
        last_col: u32,
    ) {
        let header_rect = Rect::from_min_size(
            Pos2::new(row_x, y),
            Vec2::new(ROW_HEADER_WIDTH, HEADER_HEIGHT),
        );
        painter.rect_filled(header_rect, 0.0, RAISED_BG);
        painter.rect_stroke(
            header_rect,
            0.0,
            Stroke::new(1.0, GRID_LINE),
            egui::StrokeKind::Inside,
        );
        painter.text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            "#",
            FontId::proportional(12.0),
            TEXT_MUTED,
        );
        let mut x = data_x;
        for col in first_col..last_col.min(info.column_count) {
            let width = column_widths
                .get(col as usize)
                .copied()
                .unwrap_or(MIN_COL_WIDTH);
            let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, HEADER_HEIGHT));
            let selected = self.col_selection.contains(col);
            let response = ui
                .interact(rect, egui::Id::new(("column", col)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            let resize_rect = Rect::from_center_size(
                Pos2::new(rect.right(), rect.center().y),
                Vec2::new(8.0, rect.height()),
            );
            let resize_response = ui
                .interact(
                    resize_rect,
                    egui::Id::new(("column-resize", col)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            painter.rect_filled(
                rect,
                0.0,
                if selected {
                    self.accent_dark()
                } else if response.hovered() {
                    SURFACE_BG
                } else {
                    RAISED_BG
                },
            );
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, GRID_LINE),
                egui::StrokeKind::Inside,
            );
            if selected {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(rect.left(), rect.bottom() - 2.0),
                        rect.right_bottom(),
                    ),
                    0.0,
                    self.accent_color,
                );
            }
            if resize_response.hovered() || resize_response.dragged() {
                painter.line_segment(
                    [rect.right_top(), rect.right_bottom()],
                    Stroke::new(2.0, self.accent_hover()),
                );
            }
            painter.with_clip_rect(rect.shrink(8.0)).text(
                rect.left_center() + egui::vec2(9.0, 0.0),
                egui::Align2::LEFT_CENTER,
                info.headers
                    .get(col as usize)
                    .map(String::as_str)
                    .unwrap_or(""),
                FontId::proportional(12.0),
                TEXT_PRIMARY,
            );
            if resize_response.double_clicked() {
                if let Some(base_width) = self.column_widths.get_mut(col as usize) {
                    let header = info
                        .headers
                        .get(col as usize)
                        .map(String::as_str)
                        .unwrap_or("");
                    *base_width = estimated_column_width(header, false);
                    self.column_layout_dirty = true;
                    self.last_view = None;
                }
            } else if resize_response.dragged() {
                if resize_response.drag_started() {
                    self.column_widths = column_widths.to_vec();
                }
                if let Some(base_width) = self.column_widths.get_mut(col as usize) {
                    let delta = ui.input(|input| input.pointer.delta().x);
                    *base_width = (*base_width + delta).clamp(MIN_COL_WIDTH, MAX_MANUAL_COL_WIDTH);
                    self.column_layout_dirty = true;
                }
            } else if response.clicked() && !resize_response.hovered() {
                let modifiers = ui.input(|i| i.modifiers);
                self.col_selection
                    .click(col, modifiers.shift, modifiers.command || modifiers.ctrl);
            }
            x += width;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        info: &OpenResult,
        column_widths: &[f32],
        row: u32,
        y: f32,
        row_x: f32,
        data_x: f32,
        col_start: u32,
        first_col: u32,
        last_col: u32,
        ctx: &Context,
    ) {
        let row_rect = Rect::from_min_size(
            Pos2::new(row_x, y),
            Vec2::new(ROW_HEADER_WIDTH, self.row_height),
        );
        let selected = self.row_selection.contains(row);
        let row_response = ui
            .interact(row_rect, egui::Id::new(("row", row)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        painter.rect_filled(
            row_rect,
            0.0,
            if selected {
                self.accent_dark()
            } else if row_response.hovered() {
                RAISED_BG
            } else if row.is_multiple_of(2) {
                PANEL_BG
            } else {
                SURFACE_BG
            },
        );
        painter.rect_stroke(
            row_rect,
            0.0,
            Stroke::new(1.0, GRID_LINE),
            egui::StrokeKind::Inside,
        );
        if selected {
            painter.rect_filled(
                Rect::from_min_max(
                    row_rect.left_top(),
                    Pos2::new(row_rect.left() + 3.0, row_rect.bottom()),
                ),
                0.0,
                self.accent_color,
            );
        }
        painter.text(
            row_rect.right_center() - egui::vec2(9.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            (row + 1).to_string(),
            FontId::monospace(11.0),
            TEXT_MUTED,
        );
        if row_response.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            self.row_selection
                .click(row, modifiers.shift, modifiers.command || modifiers.ctrl);
        }
        let cells = self.rows.get(&row).map(|data| data.cells.as_slice());
        let mut cell_action = None;
        let mut x = data_x;
        for col in first_col..last_col.min(info.column_count) {
            let width = column_widths
                .get(col as usize)
                .copied()
                .unwrap_or(MIN_COL_WIDTH);
            let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, self.row_height));
            let is_selected = self.selected_cell == Some((row, col));
            let column_selected = self.col_selection.contains(col);
            let response =
                ui.interact(cell_rect, egui::Id::new(("cell", row, col)), Sense::click());
            let bg = if is_selected {
                self.accent_dark()
            } else if selected {
                mix_color(TABLE_BG, self.accent_color, 0.22)
            } else if column_selected {
                mix_color(TABLE_BG, self.accent_color, 0.13)
            } else if response.hovered() {
                RAISED_BG
            } else if row.is_multiple_of(2) {
                TABLE_BG
            } else {
                Color32::from_rgb(18, 24, 25)
            };
            painter.rect_filled(cell_rect, 0.0, bg);
            painter.rect_stroke(
                cell_rect,
                0.0,
                Stroke::new(
                    if is_selected { 2.0 } else { 1.0 },
                    if is_selected {
                        self.accent_color
                    } else {
                        GRID_LINE
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let text = cells
                .and_then(|cells| cells.get(col.saturating_sub(col_start) as usize))
                .map(String::as_str)
                .unwrap_or("...");
            painter.with_clip_rect(cell_rect.shrink(7.0)).text(
                cell_rect.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::monospace(12.0),
                if is_selected {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                },
            );
            if response.double_clicked() {
                cell_action = Some((col, true));
            } else if response.clicked() {
                cell_action = Some((col, false));
            }
            x += width;
        }
        if let Some((col, editing)) = cell_action {
            self.editing = editing;
            self.load_cell(row, col, ctx);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, ctx: &Context) {
        let language = self.language;
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            if primary_button(ui, language.text("Open", "打开"), self.accent_color).clicked() {
                self.choose_open(ctx);
            }
            if let Some(info) = self.info.clone() {
                if ui.button(language.text("Export", "导出")).clicked() {
                    self.open_export();
                }
                ui.separator();
                egui::ComboBox::from_id_salt("density")
                    .selected_text(if language.is_chinese() {
                        format!("行高 {} px", self.row_height as u32)
                    } else {
                        format!("{} px rows", self.row_height as u32)
                    })
                    .show_ui(ui, |ui| {
                        for (english, chinese, height) in [
                            ("Compact", "紧凑", 24.0),
                            ("Comfortable", "舒适", 32.0),
                            ("Relaxed", "宽松", 44.0),
                            ("Spacious", "大间距", 60.0),
                        ] {
                            if ui
                                .selectable_label(
                                    self.row_height == height,
                                    format!(
                                        "{} ({} px)",
                                        language.text(english, chinese),
                                        height as u32
                                    ),
                                )
                                .clicked()
                            {
                                self.row_height = height;
                            }
                        }
                    });
                if ui.button(language.text("Auto width", "自动列宽")).clicked() {
                    self.reset_column_widths();
                }
                ui.separator();
                ui.label(
                    RichText::new(if language.is_chinese() {
                        format!(
                            "{} 行  |  {} 列  |  {}  |  {}",
                            info.row_count,
                            info.column_count,
                            format_bytes(info.file_size),
                            info.encoding
                        )
                    } else {
                        format!(
                            "{} rows  |  {} columns  |  {}  |  {}",
                            info.row_count,
                            info.column_count,
                            format_bytes(info.file_size),
                            info.encoding
                        )
                    })
                    .color(TEXT_MUTED),
                );
                ui.separator();
                ui.add(egui::Label::new(RichText::new(&info.file_name).strong()).truncate())
                    .on_hover_text(&info.file_path);
            } else {
                ui.label(
                    RichText::new(language.text("No file open", "未打开文件")).color(TEXT_MUTED),
                );
            }
        });

        if let Some(info) = self.info.clone() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let search = ui.add(
                    TextEdit::singleline(&mut self.search_query)
                        .desired_width(280.0)
                        .hint_text(language.text("Search values", "搜索内容")),
                );
                let enter_pressed = search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if primary_button(ui, language.text("Find", "查找"), self.accent_color).clicked()
                    || enter_pressed
                {
                    self.start_search(ctx);
                    self.show_search = true;
                }
                egui::ComboBox::from_id_salt("search-column")
                    .width(190.0)
                    .selected_text(
                        self.search_column
                            .map(|col| info.headers.get(col as usize).cloned().unwrap_or_default())
                            .unwrap_or_else(|| language.text("All columns", "所有列").to_string()),
                    )
                    .truncate()
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                self.search_column.is_none(),
                                language.text("All columns", "所有列"),
                            )
                            .clicked()
                        {
                            self.search_column = None;
                        }
                        for (col, header) in info.headers.iter().enumerate() {
                            if ui
                                .selectable_label(self.search_column == Some(col as u32), header)
                                .clicked()
                            {
                                self.search_column = Some(col as u32);
                            }
                        }
                    });
                ui.checkbox(
                    &mut self.search_case_sensitive,
                    language.text("Case sensitive", "区分大小写"),
                );
                if !self.search_status.is_empty() {
                    ui.label(RichText::new(&self.search_status).color(TEXT_MUTED));
                }
            });
        }
        ui.add_space(3.0);
    }

    fn title_bar(&mut self, ui: &mut Ui, ctx: &Context) {
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.horizontal(|ui| {
            let title = ui.add(
                egui::Label::new(
                    RichText::new("CSV Reader")
                        .strong()
                        .color(self.accent_color),
                )
                .sense(Sense::click_and_drag()),
            );
            let drag_width = (ui.available_width() - 220.0).max(12.0);
            let drag = ui.allocate_response(Vec2::new(drag_width, 26.0), Sense::click_and_drag());
            let drag_response = title.union(drag);
            if drag_response.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            } else if drag_response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if title_bar_control(ui, "×", Color32::from_rgb(190, 55, 55)).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let maximize_symbol = if maximized { "❐" } else { "□" };
                if title_bar_control(ui, maximize_symbol, RAISED_BG).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if title_bar_control(ui, "—", RAISED_BG).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                ui.separator();
                self.settings_menu(ui, ctx);
            });
        });
    }

    fn window_resize_handles(&self, ui: &mut Ui, ctx: &Context) {
        if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let rect = ui.max_rect();
        let edge = 5.0;
        let corner = 10.0;
        let handles = [
            (
                Rect::from_min_max(
                    rect.min,
                    Pos2::new(rect.min.x + corner, rect.min.y + corner),
                ),
                egui::ResizeDirection::NorthWest,
                CursorIcon::ResizeNwSe,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.max.x - corner, rect.min.y),
                    Pos2::new(rect.max.x, rect.min.y + corner),
                ),
                egui::ResizeDirection::NorthEast,
                CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.min.x, rect.max.y - corner),
                    Pos2::new(rect.min.x + corner, rect.max.y),
                ),
                egui::ResizeDirection::SouthWest,
                CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.max.x - corner, rect.max.y - corner),
                    rect.max,
                ),
                egui::ResizeDirection::SouthEast,
                CursorIcon::ResizeNwSe,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.min.x + corner, rect.min.y),
                    Pos2::new(rect.max.x - corner, rect.min.y + edge),
                ),
                egui::ResizeDirection::North,
                CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.min.x + corner, rect.max.y - edge),
                    Pos2::new(rect.max.x - corner, rect.max.y),
                ),
                egui::ResizeDirection::South,
                CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.min.x, rect.min.y + corner),
                    Pos2::new(rect.min.x + edge, rect.max.y - corner),
                ),
                egui::ResizeDirection::West,
                CursorIcon::ResizeHorizontal,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(rect.max.x - edge, rect.min.y + corner),
                    Pos2::new(rect.max.x, rect.max.y - corner),
                ),
                egui::ResizeDirection::East,
                CursorIcon::ResizeHorizontal,
            ),
        ];
        for (index, (handle, direction, cursor)) in handles.into_iter().enumerate() {
            let response = ui
                .interact(
                    handle,
                    egui::Id::new(("window-resize", index)),
                    Sense::drag(),
                )
                .on_hover_cursor(cursor);
            if response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
            }
        }
    }

    fn settings_menu(&mut self, ui: &mut Ui, ctx: &Context) {
        let language = self.language;
        ui.menu_button(language.text("Settings", "设置"), |ui| {
            ui.set_min_width(220.0);
            ui.label(RichText::new(language.text("Language", "语言")).strong());
            ui.selectable_value(&mut self.language, Language::English, "English");
            ui.selectable_value(&mut self.language, Language::Chinese, "简体中文");
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(language.text("Accent color", "主题色")).strong());

            let mut rgb = [
                self.accent_color.r(),
                self.accent_color.g(),
                self.accent_color.b(),
            ];
            ui.horizontal(|ui| {
                ui.label(language.text("Custom", "自定义"));
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    self.apply_accent(Color32::from_rgb(rgb[0], rgb[1], rgb[2]), ctx);
                }
                ui.monospace(color_to_hex(self.accent_color));
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (english, chinese, color) in [
                    ("Green", "绿色", ACCENT),
                    ("Blue", "蓝色", Color32::from_rgb(66, 153, 225)),
                    ("Purple", "紫色", Color32::from_rgb(167, 139, 250)),
                    ("Orange", "橙色", Color32::from_rgb(237, 137, 54)),
                    ("Rose", "玫红", Color32::from_rgb(244, 114, 182)),
                ] {
                    let selected = self.accent_color == color;
                    let response = ui.add(
                        egui::Button::new(language.text(english, chinese))
                            .fill(mix_color(SURFACE_BG, color, 0.32))
                            .stroke(Stroke::new(
                                if selected { 2.0 } else { 1.0 },
                                if selected { color } else { GRID_LINE },
                            )),
                    );
                    if response.clicked() {
                        self.apply_accent(color, ctx);
                    }
                }
            });
            if ui
                .button(language.text("Reset color", "恢复默认颜色"))
                .clicked()
            {
                self.apply_accent(ACCENT, ctx);
            }
        });
    }

    fn detail_panel(&mut self, ui: &mut Ui, ctx: &Context) {
        if !self.show_detail {
            return;
        }
        let language = self.language;
        let accent_color = self.accent_color;
        egui::Panel::right("detail-panel")
            .default_size(360.0)
            .min_size(280.0)
            .resizable(true)
            .frame(side_panel_frame())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        RichText::new(language.text("Cell content", "单元格内容")).size(16.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("x")
                            .on_hover_text(language.text("Close details", "关闭详情"))
                            .clicked()
                        {
                            self.show_detail = false;
                            self.editing = false;
                        }
                    });
                });
                if let Some((row, col)) = self.selected_cell {
                    let position = if language.is_chinese() {
                        format!("第 {} 行  |  第 {} 列", row + 1, col + 1)
                    } else {
                        format!("Row {}  |  Column {}", row + 1, col + 1)
                    };
                    ui.label(RichText::new(position).color(TEXT_MUTED));
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                let content_height = (ui.available_height() - 44.0).max(120.0);
                if self.editing {
                    ui.add_sized(
                        [ui.available_width(), content_height],
                        TextEdit::multiline(&mut self.detail_text)
                            .font(TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                } else {
                    egui::Frame::new()
                        .fill(TABLE_BG)
                        .stroke(Stroke::new(1.0, GRID_LINE))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            egui::ScrollArea::both()
                                .max_height(content_height - 20.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&self.detail_text)
                                                .monospace()
                                                .color(TEXT_SECONDARY),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                }
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.editing {
                        if primary_button(ui, language.text("Save", "保存"), accent_color).clicked()
                        {
                            self.save_edit(ctx);
                        }
                        if ui.button(language.text("Cancel", "取消")).clicked() {
                            self.editing = false;
                        }
                    } else if primary_button(ui, language.text("Edit", "编辑"), accent_color)
                        .clicked()
                    {
                        self.editing = true;
                    }
                    if ui.button(language.text("Copy", "复制")).clicked() {
                        ui.ctx().copy_text(self.detail_text.clone());
                    }
                });
            });
    }

    fn search_panel(&mut self, ui: &mut Ui, ctx: &Context) {
        if !self.show_search {
            return;
        }
        let language = self.language;
        let mut selected_match = None;
        egui::Panel::right("search-panel")
            .default_size(390.0)
            .min_size(300.0)
            .resizable(true)
            .frame(side_panel_frame())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        RichText::new(language.text("Search results", "搜索结果")).size(16.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("x")
                            .on_hover_text(language.text("Close search", "关闭搜索"))
                            .clicked()
                        {
                            self.show_search = false;
                        }
                    });
                });
                ui.label(RichText::new(&self.search_status).color(TEXT_MUTED));
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for result in &self.search_results {
                            let row = result.row_index;
                            let label = if language.is_chinese() {
                                format!("第 {} 行  |  {} 处匹配", row + 1, result.matches.len())
                            } else {
                                format!("Row {}  |  {} matches", row + 1, result.matches.len())
                            };
                            if ui
                                .add_sized(
                                    [ui.available_width(), 28.0],
                                    egui::Button::new(RichText::new(label).strong())
                                        .fill(SURFACE_BG)
                                        .stroke(Stroke::new(1.0, GRID_LINE)),
                                )
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked()
                            {
                                if let Some(first) = result.matches.first() {
                                    selected_match = Some((row, first.col_index));
                                }
                            }
                            for matched in &result.matches {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "{}: {}",
                                            matched.col_name, matched.cell_text
                                        ))
                                        .monospace()
                                        .color(TEXT_SECONDARY),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&matched.cell_text);
                            }
                            ui.add_space(8.0);
                        }
                    });
            });
        if let Some((row, col)) = selected_match {
            self.jump_to_row = Some(row);
            self.load_cell(row, col, ctx);
        }
    }

    fn export_window(&mut self, ctx: &Context) {
        let language = self.language;
        let accent_color = self.accent_color;
        let Some(info) = &self.info else { return };
        let Some(state) = &mut self.export else {
            return;
        };
        let mut do_export = false;
        let mut cancel = false;
        egui::Window::new(language.text("Export CSV", "导出 CSV"))
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(language.text("Columns", "列")).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(language.text("None", "全不选")).clicked() {
                            state.columns.fill(false);
                        }
                        if ui.small_button(language.text("All", "全选")).clicked() {
                            state.columns.fill(true);
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, header) in info.headers.iter().enumerate() {
                            ui.checkbox(
                                &mut state.columns[index],
                                format!("{}  {}", index + 1, header),
                            );
                        }
                    });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(language.text("Row range", "行范围")).strong());
                    ui.add_space(8.0);
                    ui.add(TextEdit::singleline(&mut state.from).desired_width(80.0));
                    ui.label(language.text("to", "至"));
                    ui.add(TextEdit::singleline(&mut state.to).desired_width(80.0));
                });
                if !state.error.is_empty() {
                    ui.colored_label(DANGER, &state.error);
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, language.text("Export", "导出"), accent_color).clicked()
                    {
                        do_export = true;
                    }
                    if ui.button(language.text("Cancel", "取消")).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.export = None;
        }
        if do_export {
            self.export(ctx);
        }
    }
}

impl eframe::App for CsvApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages(&ctx);
        let language = self.language;
        let accent_color = self.accent_color;
        if ctx.input(|i| i.key_pressed(Key::O) && (i.modifiers.command || i.modifiers.ctrl)) {
            self.choose_open(&ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::F) && (i.modifiers.command || i.modifiers.ctrl)) {
            self.show_search = true;
        }
        if ctx.input(|i| i.key_pressed(Key::C) && (i.modifiers.command || i.modifiers.ctrl))
            && self.selected_cell.is_some()
        {
            ctx.copy_text(self.detail_text.clone());
        }
        egui::Panel::top("window-title-bar")
            .exact_size(36.0)
            .frame(
                egui::Frame::new()
                    .fill(SURFACE_BG)
                    .inner_margin(Margin::symmetric(12, 5))
                    .stroke(Stroke::new(1.0, GRID_LINE)),
            )
            .show(ui, |ui| self.title_bar(ui, &ctx));
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL_BG)
                    .inner_margin(Margin::symmetric(12, 7))
                    .stroke(Stroke::new(1.0, GRID_LINE)),
            )
            .show(ui, |ui| self.toolbar(ui, &ctx));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL_BG)
                    .inner_margin(Margin::symmetric(12, 6))
                    .stroke(Stroke::new(1.0, GRID_LINE)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(busy) = &self.busy {
                        ui.spinner();
                        ui.label(RichText::new(busy).color(WARNING));
                    } else if let Some(error) = self.error.take() {
                        ui.colored_label(DANGER, error);
                    } else if let Some(info) = &self.info {
                        ui.colored_label(accent_color, language.text("Ready", "就绪"));
                        ui.label(
                            RichText::new(if language.is_chinese() {
                                format!("{} 行  |  {} 列", info.row_count, info.column_count)
                            } else {
                                format!("{} rows  |  {} columns", info.row_count, info.column_count)
                            })
                            .color(TEXT_MUTED),
                        );
                    } else {
                        ui.label(
                            RichText::new(language.text("No file open", "未打开文件"))
                                .color(TEXT_MUTED),
                        );
                    }
                    if let Some((start, end)) = self
                        .row_selection
                        .bounds(self.info.as_ref().map(|i| i.row_count).unwrap_or(0))
                    {
                        ui.separator();
                        ui.label(
                            RichText::new(if language.is_chinese() {
                                format!("已选择 {} 行", end - start + 1)
                            } else {
                                format!("{} rows selected", end - start + 1)
                            })
                            .color(TEXT_SECONDARY),
                        );
                    }
                    let selected_columns = self.col_selection.count();
                    if selected_columns > 0 {
                        ui.separator();
                        ui.label(
                            RichText::new(if language.is_chinese() {
                                format!("已选择 {} 列", selected_columns)
                            } else {
                                format!("{} columns selected", selected_columns)
                            })
                            .color(TEXT_SECONDARY),
                        );
                    }
                    if let Some((row, col)) = self.selected_cell {
                        ui.separator();
                        ui.label(
                            RichText::new(if language.is_chinese() {
                                format!("单元格 第{}行 第{}列", row + 1, col + 1)
                            } else {
                                format!("Cell R{} C{}", row + 1, col + 1)
                            })
                            .color(TEXT_MUTED),
                        );
                    }
                });
            });
        if self.info.is_none() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(APP_BG))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(((ui.available_height() - 110.0) * 0.42).max(24.0));
                        ui.heading(RichText::new("CSV Reader").size(24.0));
                        ui.label(
                            RichText::new(language.text("No file selected", "尚未选择文件"))
                                .color(TEXT_MUTED),
                        );
                        ui.add_space(14.0);
                        if primary_button(
                            ui,
                            language.text("Open CSV file", "打开 CSV 文件"),
                            accent_color,
                        )
                        .clicked()
                        {
                            self.choose_open(&ctx);
                        }
                        if let Some(busy) = &self.busy {
                            ui.add_space(10.0);
                            ui.spinner();
                            ui.label(RichText::new(busy).color(WARNING));
                        }
                    });
                });
        } else {
            self.search_panel(ui, &ctx);
            self.detail_panel(ui, &ctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(TABLE_BG).inner_margin(Margin::ZERO))
                .show(ui, |ui| self.table(ui, &ctx));
        }
        self.export_window(&ctx);
        if self.busy.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.window_resize_handles(ui, &ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(
            SETTINGS_LANGUAGE_KEY,
            self.language.storage_value().to_string(),
        );
        storage.set_string(SETTINGS_ACCENT_KEY, color_to_hex(self.accent_color));
    }
}

fn configure_style(ctx: &Context, accent_color: Color32) {
    let accent_hover = mix_color(accent_color, Color32::WHITE, 0.14);
    let accent_dark = mix_color(APP_BG, accent_color, 0.34);
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size = Vec2::new(36.0, 26.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.combo_width = 160.0;
    style.spacing.text_edit_width = 220.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.handle_min_length = 32.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(20.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(12.0));

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.weak_text_color = Some(TEXT_MUTED);
    visuals.panel_fill = APP_BG;
    visuals.window_fill = PANEL_BG;
    visuals.window_stroke = Stroke::new(1.0, GRID_LINE);
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(4);
    visuals.faint_bg_color = SURFACE_BG;
    visuals.extreme_bg_color = TABLE_BG;
    visuals.text_edit_bg_color = Some(TABLE_BG);
    visuals.code_bg_color = TABLE_BG;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.hyperlink_color = accent_hover;
    visuals.selection.bg_fill = accent_dark;
    visuals.selection.stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.text_cursor.stroke = Stroke::new(1.5, accent_color);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, GRID_LINE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    visuals.widgets.inactive.bg_fill = SURFACE_BG;
    visuals.widgets.inactive.weak_bg_fill = SURFACE_BG;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, GRID_LINE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
    visuals.widgets.hovered.bg_fill = RAISED_BG;
    visuals.widgets.hovered.weak_bg_fill = RAISED_BG;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_color);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
    visuals.widgets.active.bg_fill = accent_dark;
    visuals.widgets.active.weak_bg_fill = accent_dark;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, accent_color);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.active.corner_radius = CornerRadius::same(4);
    visuals.widgets.open = visuals.widgets.active;
    style.visuals = visuals;
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn primary_button(ui: &mut Ui, label: &str, accent_color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .strong()
                .color(contrasting_text(accent_color)),
        )
        .fill(accent_color)
        .stroke(Stroke::new(
            1.0,
            mix_color(accent_color, Color32::WHITE, 0.14),
        ))
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(58.0, 27.0)),
    )
}

fn title_bar_control(ui: &mut Ui, label: &str, hover_fill: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(38.0, 26.0), Sense::click());
    let fill = if response.hovered() {
        hover_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(15.0),
        TEXT_PRIMARY,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn configure_fonts(ctx: &Context) {
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    let name = "system-cjk".to_string();
    fonts
        .font_data
        .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.push(name.clone());
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
        family.push(name);
    }
    ctx.set_fonts(fonts);
}

fn mix_color(base: Color32, overlay: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| ((a as f32 * (1.0 - amount) + b as f32 * amount).round()) as u8;
    Color32::from_rgb(
        channel(base.r(), overlay.r()),
        channel(base.g(), overlay.g()),
        channel(base.b(), overlay.b()),
    )
}

fn contrasting_text(color: Color32) -> Color32 {
    let luminance = 0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
    if luminance >= 150.0 {
        Color32::from_rgb(8, 18, 15)
    } else {
        Color32::WHITE
    }
}

fn parse_color(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color32::from_rgb(red, green, blue))
}

fn color_to_hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

fn side_panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL_BG)
        .inner_margin(Margin::symmetric(14, 12))
        .stroke(Stroke::new(1.0, GRID_LINE))
}

fn estimated_column_width(text: &str, monospace: bool) -> f32 {
    let mut current_units = 0.0_f32;
    let mut max_units = 0.0_f32;
    for character in text.chars().take(80) {
        match character {
            '\n' | '\r' => {
                max_units = max_units.max(current_units);
                current_units = 0.0;
            }
            '\t' => current_units += 4.0,
            _ if character.is_ascii() => current_units += 1.0,
            _ => current_units += 2.0,
        }
    }
    max_units = max_units.max(current_units);
    let glyph_width = if monospace { 7.25 } else { 7.0 };
    (max_units * glyph_width + 24.0).clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
}

fn wheel_scroll_scale(logical_max: f64, physical_max: f64) -> f32 {
    if logical_max > physical_max && physical_max > 0.0 {
        (physical_max / logical_max).clamp(0.01, 1.0) as f32
    } else {
        1.0
    }
}

fn prefetched_row_range(first_row: u32, last_row: u32, row_count: u32) -> (u32, u32) {
    if first_row >= row_count {
        return (row_count, row_count);
    }
    let visible_rows = last_row
        .saturating_sub(first_row)
        .clamp(1, MAX_VISIBLE_ROWS / 3);
    let page_rows = visible_rows.max(MIN_PREFETCH_ROWS);
    let page_start = first_row / page_rows * page_rows;
    let start = page_start.saturating_sub(page_rows);
    let end = page_start
        .saturating_add(page_rows.saturating_mul(2))
        .max(last_row)
        .min(row_count);
    (start, end)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_storage_values_round_trip() {
        for language in [Language::English, Language::Chinese] {
            assert_eq!(Language::parse(language.storage_value()), Some(language));
        }
        assert_eq!(Language::parse("unsupported"), None);
    }

    #[test]
    fn accent_color_hex_round_trips() {
        let color = Color32::from_rgb(12, 128, 254);
        assert_eq!(color_to_hex(color), "#0C80FE");
        assert_eq!(parse_color(&color_to_hex(color)), Some(color));
        assert_eq!(parse_color("#XYZ123"), None);
    }

    #[test]
    fn color_mix_clamps_amount() {
        let base = Color32::from_rgb(10, 20, 30);
        let overlay = Color32::from_rgb(110, 120, 130);
        assert_eq!(mix_color(base, overlay, -1.0), base);
        assert_eq!(mix_color(base, overlay, 2.0), overlay);
        assert_eq!(mix_color(base, overlay, 0.5), Color32::from_rgb(60, 70, 80));
    }

    #[test]
    fn compressed_scroll_range_preserves_wheel_distance() {
        assert_eq!(wheel_scroll_scale(1_000.0, 1_000.0), 1.0);
        assert_eq!(wheel_scroll_scale(320_000_000.0, 12_000_000.0), 0.0375);
        assert_eq!(wheel_scroll_scale(10_000_000_000.0, 12_000_000.0), 0.01);
    }

    #[test]
    fn row_prefetch_range_changes_by_pages() {
        assert_eq!(prefetched_row_range(0, 20, 1_000), (0, 48));
        assert_eq!(prefetched_row_range(10, 30, 1_000), (0, 48));
        assert_eq!(prefetched_row_range(24, 44, 1_000), (0, 72));
        assert_eq!(prefetched_row_range(990, 1_010, 1_000), (960, 1_000));
    }
}
