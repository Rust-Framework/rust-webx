//! Keyset pagination end to end: `KeysetRequest` binding, opaque cursor and
//! `CursorPage` envelope.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct ListItemsRequest {
    #[fromquery]
    cursor: Option<String>,
    #[fromquery]
    page_size: Option<u32>,
}

#[get("/api/items")]
impl IRequest<CursorPage<Item>> for ListItemsRequest {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Item {
    id: u32,
}

#[derive(Default)]
struct ListItemsHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ListItemsRequest, CursorPage<Item>> for ListItemsHandler {
    async fn handle(&mut self, req: ListItemsRequest) -> ActionResult<CursorPage<Item>> {
        let paging = KeysetRequest::from_parts(req.cursor, req.page_size);
        let start = match paging.decoded()?.first() {
            Some(value) => value
                .parse::<u32>()
                .map_err(|_| Error::validation("invalid cursor".to_string()))?,
            None => 0,
        };
        let rows: Vec<Item> = (start + 1..=5)
            .take(paging.page_size() as usize + 1)
            .map(|id| Item { id })
            .collect();
        Ok(CursorPage::from_rows(rows, paging.page_size(), |row| {
            vec![row.id.to_string()]
        }))
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn spawn_host(port: u16) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

async fn page(port: u16, cursor: Option<&str>) -> serde_json::Value {
    let mut request = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/items"))
        .query(&[("page_size", "2")]);
    if let Some(cursor) = cursor {
        request = request.query(&[("cursor", cursor)]);
    }
    request
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_pages_walk_the_dataset_exactly_once() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let first = page(port, None).await;
    assert_eq!(
        first["items"],
        serde_json::json!([{ "id": 1 }, { "id": 2 }])
    );
    assert_eq!(first["has_more"], true);
    let cursor = first["cursor"].as_str().expect("cursor").to_string();

    let second = page(port, Some(&cursor)).await;
    assert_eq!(
        second["items"],
        serde_json::json!([{ "id": 3 }, { "id": 4 }])
    );
    assert_eq!(second["has_more"], true);
    let cursor = second["cursor"].as_str().expect("cursor").to_string();

    let last = page(port, Some(&cursor)).await;
    assert_eq!(last["items"], serde_json::json!([{ "id": 5 }]));
    assert_eq!(last["has_more"], false);
    assert!(last.get("cursor").is_none(), "an ended page has no cursor");

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_cursor_is_a_400() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/items"))
        .query(&[("cursor", "not-a-cursor!!"), ("page_size", "2")])
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers().get("content-type").expect("ct"),
        "application/problem+json"
    );

    handle.shutdown();
}
