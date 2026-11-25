//! Run the IM reference application.
//!
//! ```text
//! IMBIT_ADDR=127.0.0.1:8090 cargo run -p imbit
//! ```

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let address =
        std::env::var("IMBIT_ADDR").unwrap_or_else(|_| imbit_host::DEFAULT_ADDRESS.to_string());

    match imbit_host::run(&address).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "imbit stopped");
            ExitCode::FAILURE
        }
    }
}
