pub mod archive;
mod browser;
mod commands;
mod jobs;
pub mod safe_paths;
mod settings;
mod shell_requests;

#[cfg(target_os = "macos")]
mod macos_services {
    use crate::shell_requests::ShellRequestStore;
    use std::{
        collections::HashMap,
        ffi::{CStr, CString},
        os::raw::c_char,
        sync::{Mutex, OnceLock},
        thread,
    };
    use tauri::{AppHandle, Manager};

    static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
    static REQUESTS: OnceLock<ShellRequestStore> = OnceLock::new();
    static PENDING_DOCUMENTS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());
    static ICONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

    unsafe extern "C" {
        fn archive_app_register_services();
        fn archive_app_quick_look(path: *const c_char);
        fn archive_app_icon_data_url(key: *const c_char) -> *mut c_char;
        fn archive_app_free_string(value: *mut c_char);
    }

    pub(super) fn register(app: AppHandle, requests: ShellRequestStore) {
        let _ = APP_HANDLE.set(app.clone());
        requests.set_provider_registered(true);
        let _ = REQUESTS.set(requests.clone());
        unsafe { archive_app_register_services() };
        for paths in take_pending_documents() {
            dispatch(app.clone(), requests.clone(), "open".to_string(), paths);
        }
    }

    pub(super) fn open_documents(paths: Vec<String>) {
        if let (Some(app), Some(requests)) = (APP_HANDLE.get(), REQUESTS.get()) {
            dispatch(app.clone(), requests.clone(), "open".to_string(), paths);
        } else if let Ok(mut pending) = PENDING_DOCUMENTS.lock() {
            pending.push(paths);
        }
    }

    pub(crate) fn quick_look(path: &std::path::Path) -> Result<(), crate::archive::ArchiveError> {
        let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
            crate::archive::ArchiveError::new(
                "preview_unavailable",
                "The temporary preview path is not valid",
            )
        })?;
        unsafe { archive_app_quick_look(path.as_ptr()) };
        Ok(())
    }

    pub(super) fn icons(keys: Vec<String>) -> HashMap<String, String> {
        let cache = ICONS.get_or_init(|| Mutex::new(HashMap::new()));
        let Ok(mut cache) = cache.lock() else {
            return HashMap::new();
        };
        for key in keys.into_iter().take(100) {
            if cache.contains_key(&key)
                || !(key == "__folder__"
                    || (1..=16).contains(&key.len())
                        && key.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            {
                continue;
            }
            let Ok(key_string) = CString::new(key.as_str()) else {
                continue;
            };
            let pointer = unsafe { archive_app_icon_data_url(key_string.as_ptr()) };
            if pointer.is_null() {
                continue;
            }
            let value = unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned();
            unsafe { archive_app_free_string(pointer) };
            cache.insert(key, value);
        }
        cache.clone()
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn archive_app_receive_service_paths(
        action: *const c_char,
        paths_json: *const c_char,
    ) {
        match unsafe { decode_request(action, paths_json) } {
            Ok((action, paths)) => {
                if let (Some(app), Some(requests)) = (APP_HANDLE.get(), REQUESTS.get()) {
                    dispatch(app.clone(), requests.clone(), action, paths);
                }
            }
            Err(error) => eprintln!("macOS Service request rejected: {error}"),
        }
    }

    fn dispatch(app: AppHandle, requests: ShellRequestStore, action: String, paths: Vec<String>) {
        thread::spawn(move || {
            if let Err(error) = requests.submit(&action, paths) {
                eprintln!("macOS Service request rejected: {error}");
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        });
    }

    fn take_pending_documents() -> Vec<Vec<String>> {
        PENDING_DOCUMENTS
            .lock()
            .map(|mut pending| pending.drain(..).collect())
            .unwrap_or_default()
    }

    unsafe fn decode_request(
        action: *const c_char,
        paths_json: *const c_char,
    ) -> Result<(String, Vec<String>), String> {
        if action.is_null() || paths_json.is_null() {
            return Err("macOS Service returned a null request".to_string());
        }

        let action = unsafe { CStr::from_ptr(action) }
            .to_str()
            .map_err(|_| "macOS Service action was not UTF-8")?
            .to_string();
        let paths_json = unsafe { CStr::from_ptr(paths_json) }
            .to_str()
            .map_err(|_| "macOS Service paths were not UTF-8")?;
        let paths: Vec<String> = serde_json::from_str(paths_json)
            .map_err(|error| format!("invalid macOS Service paths: {error}"))?;

        if paths.is_empty() {
            return Err("macOS Service returned no paths".to_string());
        }

        Ok((action, paths))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::ffi::CString;

        #[test]
        fn decodes_multiple_finder_urls() {
            let action = CString::new("inspect").unwrap();
            let paths = CString::new("[\"/tmp/one.zip\",\"/tmp/two.7z\"]").unwrap();
            let (action, paths) =
                unsafe { decode_request(action.as_ptr(), paths.as_ptr()) }.unwrap();
            assert_eq!(action, "inspect");
            assert_eq!(paths, ["/tmp/one.zip", "/tmp/two.7z"]);
        }

        #[test]
        fn buffers_document_opens_until_setup() {
            open_documents(vec!["/tmp/cold-start.zip".to_string()]);
            assert_eq!(
                take_pending_documents(),
                [vec!["/tmp/cold-start.zip".to_string()]]
            );
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{
        menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, HELP_SUBMENU_ID},
        Emitter, Manager,
    };

    const ABOUT_MENU_ID: &str = "about-archi";
    const NEW_ARCHIVE_MENU_ID: &str = "new-archive";
    const OPEN_ARCHIVE_MENU_ID: &str = "open-archive";
    const SETTINGS_MENU_ID: &str = "settings";

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(jobs::JobManager::default())
        .manage(browser::ArchiveStore::default())
        .menu(|app| {
            let menu = Menu::default(app)?;

            #[cfg(target_os = "macos")]
            if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
                app_menu.remove_at(0)?;
                app_menu.remove_at(0)?;
            }

            if let Some(MenuItemKind::Submenu(help_menu)) = menu.get(HELP_SUBMENU_ID) {
                #[cfg(not(target_os = "macos"))]
                help_menu.remove_at(0)?;
                help_menu.append(&MenuItem::with_id(
                    app,
                    ABOUT_MENU_ID,
                    "About Archi",
                    true,
                    None::<&str>,
                )?)?;
            }

            for item in menu.items()? {
                if let MenuItemKind::Submenu(file_menu) = item {
                    if file_menu.text()? == "File" {
                        let new_archive = MenuItem::with_id(
                            app,
                            NEW_ARCHIVE_MENU_ID,
                            "New Archive…",
                            true,
                            Some("CmdOrCtrl+N"),
                        )?;
                        let open_archive = MenuItem::with_id(
                            app,
                            OPEN_ARCHIVE_MENU_ID,
                            "Open Archive…",
                            true,
                            Some("CmdOrCtrl+O"),
                        )?;
                        let settings = MenuItem::with_id(
                            app,
                            SETTINGS_MENU_ID,
                            "Settings…",
                            true,
                            Some("CmdOrCtrl+,"),
                        )?;
                        let first_separator = PredefinedMenuItem::separator(app)?;
                        let second_separator = PredefinedMenuItem::separator(app)?;
                        file_menu.insert_items(
                            &[
                                &new_archive,
                                &open_archive,
                                &first_separator,
                                &settings,
                                &second_separator,
                            ],
                            0,
                        )?;
                        break;
                    }
                }
            }

            Ok(menu)
        })
        .on_menu_event(|app, event| {
            let action = match event.id().as_ref() {
                ABOUT_MENU_ID => Some("about"),
                NEW_ARCHIVE_MENU_ID => Some("new-archive"),
                OPEN_ARCHIVE_MENU_ID => Some("open-archive"),
                SETTINGS_MENU_ID => Some("settings"),
                _ => None,
            };
            if let Some(action) = action {
                let _ = app.emit("menu-action", action);
            }
        })
        .setup(|app| {
            app.manage(settings::LocalData::initialize(app.handle())?);
            let shell_requests = shell_requests::ShellRequestStore::initialize(app.handle())?;
            app.manage(shell_requests.clone());
            if let Err(error) = safe_paths::cleanup_stale_staging() {
                eprintln!("Stale extraction cleanup failed: {error}");
            }
            match app.path().app_cache_dir() {
                Ok(cache) => {
                    if let Err(error) = safe_paths::cleanup_stale_previews(
                        &cache.join("Previews"),
                        std::time::Duration::from_secs(24 * 60 * 60),
                    ) {
                        eprintln!("Stale preview cleanup failed: {error}");
                    }
                }
                Err(error) => eprintln!("Preview cache is unavailable: {error}"),
            }
            #[cfg(target_os = "macos")]
            macos_services::register(app.handle().clone(), shell_requests);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_archive,
            commands::start_extract,
            commands::open_archive_entry,
            commands::create_archive,
            commands::add_to_archive,
            commands::delete_archive_entries,
            commands::rename_archive_entry,
            commands::set_archive_comment,
            commands::test_archive,
            commands::entry_page,
            commands::archive_folders,
            commands::archive_changed,
            commands::job_status,
            commands::open_destination,
            commands::cancel_job,
            commands::get_settings,
            commands::save_settings,
            commands::reset_settings,
            commands::recent_archives,
            commands::clear_recent_archives,
            commands::record_diagnostic,
            commands::clear_diagnostics,
            commands::export_diagnostics,
            commands::take_shell_requests,
            commands::shell_integration_status,
            commands::default_zip_output,
            commands::entry_icons
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = event {
            let paths = urls
                .into_iter()
                .filter_map(|url| url.to_file_path().ok())
                .filter(|path| path.is_file())
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                macos_services::open_documents(paths);
            }
        }
    });
}
