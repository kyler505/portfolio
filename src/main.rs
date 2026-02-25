#[cfg(target_arch = "wasm32")]
mod frontend;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This project is frontend-only. Run `trunk serve` or `trunk build --release`.");
}

#[cfg(target_arch = "wasm32")]
fn main() {
    frontend::run();
}
