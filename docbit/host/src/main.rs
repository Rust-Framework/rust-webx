//! Docbit host binary entry point.

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--doctor") {
        print!("{}", rust_webx::format_route_diagnostics());
        return;
    }

    docbit_host::build_host()
        .run()
        .await
        .expect("Server failed");
}
