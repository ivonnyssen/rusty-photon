use rusty_photon_ui::App;
use dioxus_web;

fn main() {
    console_error_panic_hook::set_once();
    wasm_logger::init(wasm_logger::Config::default());
    dioxus_web::launch::launch(App, vec![], vec![]);
}
