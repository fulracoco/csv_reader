mod commands;
pub mod csv_engine;

use commands::AppLocale;
use csv_engine::CsvEngine;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

#[cfg(windows)]
mod win_webview {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    pub fn show_webview2_missing_dialog() {
        let title: Vec<u16> = OsStr::new("CSV Reader")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let text: Vec<u16> = OsStr::new(
            "WebView2 runtime is required but not installed.\n\n\
             Please download and install WebView2 from:\n\
             https://go.microsoft.com/fwlink/p/?LinkId=2124703",
        )
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                0x00000010,
            );
        }
    }

    extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            lp_text: *const u16,
            lp_caption: *const u16,
            u_type: u32,
        ) -> i32;
    }
}

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
                    "menu_fullscreen" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let is_fs = window.is_fullscreen().unwrap_or(false);
                            let _ = window.set_fullscreen(!is_fs);
                        }
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
            commands::search_csv,
            commands::set_language,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            #[cfg(windows)]
            win_webview::show_webview2_missing_dialog();
            panic!("error while running tauri application: {e}");
        });
}

fn set_language_inner(app: &tauri::AppHandle, locale: &str) -> Result<(), String> {
    let state = app.state::<AppLocale>();
    *state.0.lock().unwrap() = locale.to_string();
    let menu = commands::build_menu(app, locale).map_err(|e| e.to_string())?;
    app.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}
