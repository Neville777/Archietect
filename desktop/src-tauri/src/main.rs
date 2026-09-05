// Thin entry point, deliberately — all real logic lives in the library
// target (src/lib.rs) so `cargo test` can exercise it without needing a
// display/webview, the same split every Tauri app uses.
fn main() {
    archietect_desktop_lib::run();
}
