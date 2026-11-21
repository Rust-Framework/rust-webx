//! Run the IM reference application.
//!
//! ```text
//! IMBIT_ADDR=127.0.0.1:8090 cargo run -p imbit
//! ```

#[tokio::main]
async fn main() {
    let address = std::env::var("IMBIT_ADDR").unwrap_or_else(|_| "127.0.0.1:8090".to_string());
    if let Err(error) = imbit::run(&address).await {
        eprintln!("imbit stopped: {error}");
        std::process::exit(1);
    }
}
