use crate::csv_engine::{CsvEngine, OpenResult, RowData, SearchProgress, SearchResult};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{Emitter, State};
use tauri::menu::{MenuBuilder, SubmenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri_plugin_dialog::DialogExt;

// ─── App State ───────────────────────────────────────────────────────────────

pub struct AppLocale(pub Mutex<String>);

// ─── Progress event payloads ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct IndexProgress {
    pub percent: u32,
    pub current: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ExportProgress {
    pub current: u64,
    pub total: u64,
}

// ─── Tauri Commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn open_file(
    app: tauri::AppHandle,
    engine: State<'_, Mutex<CsvEngine>>,
) -> Result<Option<OpenResult>, String> {
    let path = app
        .dialog()
        .file()
        .add_filter("CSV & TSV Files", &["csv", "tsv", "txt"])
        .add_filter("All Files", &["*"])
        .blocking_pick_file();

    match path {
        Some(file_path) => {
            let path_str = file_path.as_path().unwrap().to_string_lossy().to_string();
            match engine.lock().unwrap().open(&path_str) {
                Ok(info) => Ok(Some(info)),
                Err(e) => Err(e),
            }
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub fn open_file_path(
    engine: State<'_, Mutex<CsvEngine>>,
    path: String,
) -> Result<OpenResult, String> {
    engine.lock().unwrap().open(&path)
}

#[tauri::command]
pub fn get_rows(
    engine: State<'_, Mutex<CsvEngine>>,
    start: u32,
    count: u32,
) -> Result<Vec<RowData>, String> {
    engine.lock().unwrap().get_rows(start, count)
}

#[tauri::command]
pub fn get_rows_by_index(
    engine: State<'_, Mutex<CsvEngine>>,
    indices: Vec<u32>,
) -> Result<Vec<Vec<String>>, String> {
    engine.lock().unwrap().get_rows_by_index(&indices)
}

#[tauri::command]
pub fn get_cell_content(
    engine: State<'_, Mutex<CsvEngine>>,
    row: u32,
    col: u32,
) -> Result<String, String> {
    engine.lock().unwrap().get_cell_content(row, col)
}

#[tauri::command]
pub fn update_cell(
    engine: State<'_, Mutex<CsvEngine>>,
    row: u32,
    col: u32,
    content: String,
) -> Result<(), String> {
    engine.lock().unwrap().update_cell(row, col, &content)
}

#[tauri::command]
pub fn export_csv(
    app: tauri::AppHandle,
    engine: State<'_, Mutex<CsvEngine>>,
    col_indices: Vec<u32>,
    start_row: u32,
    end_row: u32,
) -> Result<serde_json::Value, String> {
    let default_name = {
        let eng = engine.lock().unwrap();
        eng.file_path()
            .rsplit_once('.')
            .map(|(base, _)| format!("{}_export.csv", base))
            .unwrap_or_else(|| "export.csv".to_string())
    };

    let path = app
        .dialog()
        .file()
        .add_filter("CSV Files", &["csv"])
        .set_file_name(&default_name)
        .blocking_save_file();

    match path {
        Some(file_path) => {
            let path_str = file_path.as_path().unwrap().to_string_lossy().to_string();
            engine
                .lock()
                .unwrap()
                .export_csv(&path_str, &col_indices, start_row, end_row)?;
            Ok(serde_json::json!({"ok": true, "path": path_str}))
        }
        None => Ok(serde_json::json!({"canceled": true})),
    }
}

#[tauri::command]
pub fn search_csv(
    app: tauri::AppHandle,
    engine: State<'_, Mutex<CsvEngine>>,
    query: String,
    col_filter: Option<u32>,
    case_sensitive: Option<bool>,
    max_results: Option<u32>,
) -> Result<Vec<SearchResult>, String> {
    let case_sensitive = case_sensitive.unwrap_or(false);
    let max_results = max_results.unwrap_or(500);
    let app_handle = app.clone();
    engine.lock().unwrap().search_with_progress(
        &query,
        col_filter,
        case_sensitive,
        max_results,
        move |done, total| {
            let _ = app_handle.emit("search-progress", SearchProgress { done, total });
        },
    )
}

#[tauri::command]
pub fn set_language(
    app: tauri::AppHandle,
    locale_state: State<'_, AppLocale>,
    locale: String,
) -> Result<(), String> {
    *locale_state.0.lock().unwrap() = locale.clone();
    let menu = build_menu(&app, &locale).map_err(|e| e.to_string())?;
    app.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Menu ────────────────────────────────────────────────────────────────────

pub fn build_menu(
    app: &tauri::AppHandle,
    locale: &str,
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    let t = |en: &'static str, zh: &'static str| -> &'static str {
        if locale == "zh" { zh } else { en }
    };

    let file_submenu = SubmenuBuilder::new(app, t("File", "文件"))
        .item(
            &MenuItemBuilder::with_id("menu_open", t("Open File...", "打开文件..."))
                .accelerator("CmdOrCtrl+O")
                .build(app)?,
        )
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some(t("Quit", "退出")))?)
        .build()?;

    let edit_submenu = SubmenuBuilder::new(app, t("Edit", "编辑"))
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, t("View", "视图"))
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    let help_submenu = SubmenuBuilder::new(app, t("Help", "帮助"))
        .item(
            &MenuItemBuilder::with_id("menu_issues", t("Issues", "问题反馈"))
                .build(app)?,
        )
        .separator()
        .item(
            &SubmenuBuilder::new(app, t("Language", "语言"))
                .item(
                    &MenuItemBuilder::with_id("lang_en", t("English", "English"))
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("lang_zh", t("中文", "中文"))
                        .build(app)?,
                )
                .build()?,
        )
        .build()?;

    let menu = MenuBuilder::new(app)
        .item(&file_submenu)
        .item(&edit_submenu)
        .item(&view_submenu)
        .item(&help_submenu)
        .build()?;

    Ok(menu)
}
