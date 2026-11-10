// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Elevated TUN install and the Wintun bridge reuse this executable. They
    // must return before the window is created.
    if v2rayr_helper::tun::handle_process_entry() {
        return;
    }
    v2rayr_desktop::run()
}
