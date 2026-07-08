//! Docbit host binary entry point.

#[tokio::main]
async fn main() {
    docbit_host::build_host()
        .run()
        .await
        .expect("Server failed");
}
