//! Archietect Desktop — a native window wrapper around the SAME REST server
//! and the SAME `ui/index.html` the CLI's `archietect gui` uses. No new
//! business logic lives here; this crate exists only to answer "download
//! one thing, double-click it, no terminal at all," which `archietect gui`
//! cannot do since it has to be typed into a terminal in the first place.
//!
//! ## Why the window navigates to a real `http://` URL instead of Tauri's
//! ## own bundled-asset protocol
//!
//! `ui/index.html`'s JS calls `fetch('/doctor?root=...')` — a RELATIVE
//! path, correct only if the page is served BY the archietect REST server
//! itself (true for `archietect serve`/`archietect gui`, both of which
//! serve the page from the same origin the API lives on). If Tauri served
//! that same file through its own internal asset protocol instead, those
//! relative fetches would resolve against Tauri's asset server, not
//! archietect's REST server, and every tab would 404. So this crate does
//! NOT rely on `tauri.conf.json`'s bundled frontend for the live window —
//! it starts `archietect::rest::serve` on a real local port and points the
//! window straight at it, exactly the way a browser does for
//! `archietect gui`. `frontendDist` in the config still points at `ui/`
//! only because Tauri's schema requires SOME valid path there; it is
//! otherwise unused at runtime.

use tauri_plugin_dialog::DialogExt;

/// Starting point for an ephemeral local port search — high enough to
/// avoid the CLI's own default (`archietect serve`/`gui` use 7373), so a
/// user running both at once doesn't collide.
const PORT_SEARCH_START: u16 = 47373;

/// Binds and immediately drops a listener on each candidate port until one
/// succeeds — the standard "find a free port" trick, since `rest::serve`
/// itself takes a fixed port rather than an ephemeral one. A real TOCTOU
/// gap exists between this check and `rest::serve`'s own bind a moment
/// later, but the window is milliseconds and the cost of losing the race
/// is a clear bind error, not silent corruption.
fn find_free_port(start: u16) -> u16 {
    for port in start..start.saturating_add(200) {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    start
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    // `pick_folder`'s own callback runs on Tauri's event loop; blocking
    // this async command's task (not the main thread) on the channel is
    // the documented way to turn tauri-plugin-dialog's callback API into
    // something `await`-able from the frontend's single `invoke()` call.
    let folder = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .ok()
        .flatten();
    folder.map(|f| f.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port = find_free_port(PORT_SEARCH_START);
    // No default root — a first launch has no project chosen yet; the page
    // itself already handles an empty root gracefully (shows the "set a
    // repository root above" placeholder), same as a browser hitting
    // `archietect gui` with nothing typed in yet.
    std::thread::spawn(move || {
        let _ = archietect::rest::serve(None, port);
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![pick_folder])
        .setup(move |app| {
            // Give tiny_http a moment to actually bind before the webview
            // tries to load from it — same reasoning, and same small
            // fixed delay, as the CLI's `archietect gui` uses before
            // opening an external browser to the same kind of URL.
            std::thread::sleep(std::time::Duration::from_millis(200));
            let url = format!("http://127.0.0.1:{port}/");
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url.parse()?))
                .title("Archietect")
                .inner_size(1150.0, 760.0)
                .min_inner_size(700.0, 480.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running archietect desktop");
}
