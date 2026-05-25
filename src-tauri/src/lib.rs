mod commands;
pub mod csv_engine;

use commands::AppLocale;
use csv_engine::CsvEngine;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(Mutex::new(CsvEngine::new()))
        .manage(AppLocale(Mutex::new(String::from("en"))))
        .setup(|app| {
            let menu = commands::build_menu(app.handle(), "en")?;
            app.set_menu(menu)?;

            let _handle = app.handle().clone();
            app.on_menu_event(move |app_handle, event| {
                let id = event.id().0.as_str();
                match id {
                    "menu_open" => {
                        let _ = app_handle.emit("menu-open-file", ());
                    }
                    "menu_issues" => {
                        #[allow(deprecated)]
                        let _ = tauri_plugin_shell::ShellExt::shell(app_handle)
                            .open("https://github.com/fulracoco/csv_reader/issues", None);
                    }
                    "lang_en" => {
                        let _ = set_language_inner(app_handle, "en");
                    }
                    "lang_zh" => {
                        let _ = set_language_inner(app_handle, "zh");
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::open_file_path,
            commands::get_rows,
            commands::get_rows_by_index,
            commands::get_cell_content,
            commands::update_cell,
            commands::export_csv,
            commands::set_language,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn set_language_inner(app: &tauri::AppHandle, locale: &str) -> Result<(), String> {
    let state = app.state::<AppLocale>();
    *state.0.lock().unwrap() = locale.to_string();
    let menu = commands::build_menu(app, locale).map_err(|e| e.to_string())?;
    app.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}
