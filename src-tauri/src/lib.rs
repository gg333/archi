pub mod archive;
mod browser;
mod commands;
mod jobs;
pub mod safe_paths;
mod settings;
mod shell_requests;

/// Keeps Quit and window close from silently abandoning a running archive job.
mod quit_guard {
    use crate::jobs::JobManager;
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
        time::Duration,
    };
    use tauri::{AppHandle, Manager};
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    static PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

    /// Returns true when the app may exit now. Otherwise the request must be refused;
    /// the user is asked once whether to stop the job, and the app exits after it has ended.
    pub(super) fn allow_exit(app: &AppHandle) -> bool {
        let jobs = app.state::<JobManager>().inner().clone();
        if !jobs.is_active() {
            return true;
        }
        if PROMPT_OPEN.swap(true, Ordering::SeqCst) {
            return false;
        }
        let handle = app.clone();
        let mut dialog = app
            .dialog()
            .message(
                "An archive operation is still running. Stopping it discards its progress. \
                 If it is already saving its result, Archi quits as soon as that finishes.",
            )
            .title("Quit Archi?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Stop and Quit".to_string(),
                "Keep Working".to_string(),
            ));
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.parent(&window);
        }
        dialog.show(move |stop| {
            if !stop {
                PROMPT_OPEN.store(false, Ordering::SeqCst);
                return;
            }
            thread::spawn(move || {
                // Cancelling is refused once results are being installed; that step runs to the end.
                let _ = jobs.cancel();
                while jobs.is_active() {
                    thread::sleep(Duration::from_millis(50));
                }
                handle.exit(0);
            });
        });
        false
    }
}

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
        fn archive_app_install_quit_guard();
        fn archive_app_quick_look(path: *const c_char);
        fn archive_app_icon_data_url(key: *const c_char) -> *mut c_char;
        fn archive_app_free_string(value: *mut c_char);
    }

    pub(super) fn register(app: AppHandle, requests: ShellRequestStore) {
        let _ = APP_HANDLE.set(app.clone());
        requests.set_provider_registered(true);
        let _ = REQUESTS.set(requests.clone());
        unsafe { archive_app_register_services() };
        unsafe { archive_app_install_quit_guard() };
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
            Err(error) => {
                eprintln!("macOS Service request rejected: {error}");
                if let Some(requests) = REQUESTS.get() {
                    requests.reject(&error);
                }
            }
        }
    }

    /// Called by AppKit for Cmd+Q, the Dock's Quit, and logout, none of which reach Tauri's
    /// exit-requested event.
    #[unsafe(no_mangle)]
    pub extern "C" fn archive_app_should_terminate() -> bool {
        APP_HANDLE.get().is_none_or(crate::quit_guard::allow_exit)
    }

    fn dispatch(app: AppHandle, requests: ShellRequestStore, action: String, paths: Vec<String>) {
        thread::spawn(move || {
            // A refused request is still shown to the user, so the window comes forward either way.
            if let Err(error) = requests.submit(&action, paths) {
                eprintln!("macOS Service request rejected: {error}");
                requests.reject(&error.message);
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

/// The menu bar. Items carry the id the window is told about when they are chosen; the
/// window reports which of them currently apply, and which archives are recent.
pub(crate) mod app_menu {
    use std::{collections::HashMap, path::Path, sync::Mutex};
    use tauri::{
        menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID},
        AppHandle, Emitter, Manager, Wry,
    };

    const WEBSITE: &str = "https://gg333.github.io/archi/";
    const WEBSITE_ID: &str = "website";
    const CLEAR_RECENTS_ID: &str = "clear-recents";
    const RECENT_PREFIX: &str = "recent:";

    pub(crate) struct AppMenu {
        items: HashMap<&'static str, MenuItem<Wry>>,
        recent: Submenu<Wry>,
        recents: Mutex<Vec<String>>,
    }

    pub(super) fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
        let menu = Menu::default(app)?;
        let mut items = HashMap::new();
        let mut item = |id: &'static str, text: &str, shortcut: Option<&str>| {
            let item = MenuItem::with_id(app, id, text, true, shortcut)?;
            items.insert(id, item.clone());
            tauri::Result::Ok(item)
        };
        let about = item("about", "About Archi", None)?;
        let settings = item("settings", "Settings…", Some("CmdOrCtrl+,"))?;
        let new_archive = item("new-archive", "New Archive…", Some("CmdOrCtrl+N"))?;
        let open_archive = item("open-archive", "Open Archive…", Some("CmdOrCtrl+O"))?;
        let close_archive = item("close-archive", "Close Archive", Some("CmdOrCtrl+Shift+W"))?;
        let extract = item("extract", "Extract…", Some("CmdOrCtrl+E"))?;
        let test = item("test", "Test Archive", Some("CmdOrCtrl+T"))?;
        let add_files = item("add-files", "Add Files…", None)?;
        let add_folder = item("add-folder", "Add Folder…", None)?;
        let rename = item("rename", "Rename…", None)?;
        // No shortcut here: it would also fire while typing in the filter field.
        let delete = item("delete", "Delete", None)?;
        let comment = item("comment", "Edit Comment…", None)?;
        let properties = item("properties", "Properties", Some("CmdOrCtrl+I"))?;
        let sidebar = item(
            "toggle-sidebar",
            "Show or Hide Sidebar",
            Some("Ctrl+CmdOrCtrl+S"),
        )?;
        let recent = Submenu::with_id(app, "open-recent", "Open Recent", true)?;
        let separator = || PredefinedMenuItem::separator(app);

        // The application menu holds About and Settings, as on every Mac app.
        #[cfg(target_os = "macos")]
        if let Some(MenuItemKind::Submenu(application)) = menu.items()?.into_iter().next() {
            application.remove_at(0)?;
            application.insert(&about, 0)?;
            application.insert_items(&[&settings, &separator()?], 2)?;
        }
        if let Some(MenuItemKind::Submenu(help)) = menu.get(HELP_SUBMENU_ID) {
            #[cfg(not(target_os = "macos"))]
            {
                help.remove_at(0)?;
                help.append(&about)?;
            }
            help.append(&MenuItem::with_id(
                app,
                WEBSITE_ID,
                "Archi Website",
                true,
                None::<&str>,
            )?)?;
        }

        let mut window_position = None;
        for (position, entry) in menu.items()?.into_iter().enumerate() {
            let MenuItemKind::Submenu(submenu) = entry else {
                continue;
            };
            match submenu.text()?.as_str() {
                "File" => {
                    submenu.insert_items(
                        &[
                            &new_archive,
                            &open_archive,
                            &recent,
                            &separator()?,
                            &close_archive,
                            &separator()?,
                        ],
                        0,
                    )?;
                    #[cfg(not(target_os = "macos"))]
                    submenu.insert_items(&[&settings, &separator()?], 6)?;
                }
                "View" => submenu.insert_items(&[&sidebar, &separator()?], 0)?,
                "Window" => window_position = Some(position),
                _ => {}
            }
        }
        let archive = Submenu::with_items(
            app,
            "Archive",
            true,
            &[
                &extract,
                &test,
                &separator()?,
                &add_files,
                &add_folder,
                &separator()?,
                &rename,
                &delete,
                &comment,
                &separator()?,
                &properties,
            ],
        )?;
        match window_position {
            Some(position) => menu.insert(&archive, position)?,
            None => menu.append(&archive)?,
        }

        let state = AppMenu {
            items,
            recent,
            recents: Mutex::new(Vec::new()),
        };
        state.show_recents(app, Vec::new())?;
        app.manage(state);
        Ok(menu)
    }

    pub(super) fn handle_event(app: &AppHandle, id: &str) {
        let Some(state) = app.try_state::<AppMenu>() else {
            return;
        };
        if id == WEBSITE_ID {
            let _ = crate::commands::platform_open(Path::new(WEBSITE));
        } else if let Some(index) = id.strip_prefix(RECENT_PREFIX) {
            let path = index.parse::<usize>().ok().and_then(|index| {
                state
                    .recents
                    .lock()
                    .ok()
                    .and_then(|recents| recents.get(index).cloned())
            });
            if let Some(path) = path {
                let _ = app.emit("menu-open-recent", path);
            }
        } else if id == CLEAR_RECENTS_ID || state.items.contains_key(id) {
            let _ = app.emit("menu-action", id);
        }
    }

    impl AppMenu {
        /// Enables or disables items by id; ids that are not menu items are ignored.
        pub(crate) fn set_enabled(&self, enabled: &HashMap<String, bool>) -> tauri::Result<()> {
            for (id, enabled) in enabled {
                if id == "open-recent" {
                    self.recent.set_enabled(*enabled)?;
                } else if let Some(item) = self.items.get(id.as_str()) {
                    item.set_enabled(*enabled)?;
                }
            }
            Ok(())
        }

        /// Rebuilds File > Open Recent from the given archive paths.
        pub(crate) fn show_recents(
            &self,
            app: &AppHandle,
            paths: Vec<String>,
        ) -> tauri::Result<()> {
            for _ in self.recent.items()? {
                self.recent.remove_at(0)?;
            }
            let name = |path: &str| {
                Path::new(path).file_name().map_or_else(
                    || path.to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            };
            for (index, path) in paths.iter().enumerate() {
                // Two archives with one name are told apart by their folder.
                let shared = paths
                    .iter()
                    .filter(|other| name(other) == name(path))
                    .count()
                    > 1;
                let folder = Path::new(path)
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .map(|folder| folder.to_string_lossy().into_owned())
                    .filter(|_| shared);
                let label = match folder {
                    Some(folder) => format!("{} — {folder}", name(path)),
                    None => name(path),
                };
                self.recent.append(&MenuItem::with_id(
                    app,
                    format!("{RECENT_PREFIX}{index}"),
                    label,
                    true,
                    None::<&str>,
                )?)?;
            }
            if !paths.is_empty() {
                self.recent.append(&PredefinedMenuItem::separator(app)?)?;
            }
            self.recent.append(&MenuItem::with_id(
                app,
                CLEAR_RECENTS_ID,
                "Clear Menu",
                !paths.is_empty(),
                None::<&str>,
            )?)?;
            if let Ok(mut recents) = self.recents.lock() {
                *recents = paths;
            }
            Ok(())
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(jobs::JobManager::default())
        .manage(browser::ArchiveStore::default())
        .menu(app_menu::build)
        .on_menu_event(|app, event| app_menu::handle_event(app, event.id().as_ref()))
        .setup(|app| {
            app.manage(settings::LocalData::initialize(app.handle())?);
            let shell_requests = shell_requests::ShellRequestStore::initialize(app.handle())?;
            app.manage(shell_requests.clone());
            if let Err(error) = safe_paths::cleanup_stale_staging() {
                eprintln!("Stale extraction cleanup failed: {error}");
            }
            match app.path().app_cache_dir() {
                Ok(cache) => {
                    // Copies made for Open, Quick Look, and drag-out do not outlive the session.
                    if let Err(error) = safe_paths::cleanup_stale_previews(
                        &cache.join("Previews"),
                        std::time::Duration::ZERO,
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
            commands::prepare_archive_entry,
            commands::create_archive,
            commands::add_to_archive,
            commands::delete_archive_entries,
            commands::rename_archive_entry,
            commands::set_archive_comment,
            commands::test_archive,
            commands::entry_page,
            commands::view_entry_paths,
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
            commands::set_menu_enabled,
            commands::set_recent_menu,
            commands::take_shell_requests,
            commands::take_shell_rejections,
            commands::shell_integration_status,
            commands::default_zip_output,
            commands::entry_icons
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Opened { urls } => {
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
        tauri::RunEvent::Exit => {
            if let Ok(cache) = handle.path().app_cache_dir() {
                let _ = safe_paths::cleanup_stale_previews(
                    &cache.join("Previews"),
                    std::time::Duration::ZERO,
                );
            }
        }
        tauri::RunEvent::ExitRequested { api, .. } if !quit_guard::allow_exit(handle) => {
            api.prevent_exit();
        }
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if !quit_guard::allow_exit(handle) => {
            api.prevent_close();
        }
        _ => {}
    });
}
