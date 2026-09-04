//! Dmbit host binary.

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--doctor") {
        print!("{}", rust_webx::format_route_diagnostics());
        return;
    }

    dmbit_host::build_host().run().await.expect("Server failed");
}
