#[cfg(not(target_arch = "wasm32"))]
mod backend;
#[cfg(target_arch = "wasm32")]
mod frontend;

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    if let Err(error) = backend::run().await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    frontend::run();
}
