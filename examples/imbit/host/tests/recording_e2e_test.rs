//! 2.4-2 end-to-end: recording metadata + resumable segment upload + playback.
//!
//! Control plane stores segment references through the `IObjectStore` port;
//! uploading the same part again is an idempotent retry, and segments are
//! replayed as raw bytes through the self-controlled `IRequest<Response>`
//! endpoint.

use std::time::{Duration, Instant};

use tokio::net::TcpStream;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("imbit did not become ready");
}

async fn login(http: &reqwest::Client, base: &str, username: &str) -> (String, String) {
    let response = http
        .post(format!("{base}/api/login"))
        .json(&serde_json::json!({ "username": username, "password": "secret-123" }))
        .send()
        .await
        .expect("login request");
    assert!(response.status().is_success(), "{:?}", response.status());
    let body: serde_json::Value = response.json().await.expect("login body");
    (
        body["token"].as_str().expect("token").to_string(),
        body["user_id"].as_str().expect("user id").to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recording_upload_resume_and_playback_work_end_to_end() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let address = format!("127.0.0.1:{port}");
    let host = imbit_host::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let http = reqwest::Client::new();
    let (alice_token, _alice_id) = login(&http, &base, "alice").await;
    let (charlie_token, _charlie_id) = login(&http, &base, "charlie").await;

    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "recorded" }))
        .send()
        .await
        .expect("create room")
        .json()
        .await
        .expect("room body");
    let room_id = room["id"].as_str().expect("room id").to_string();

    // Start recording (alice is a member).
    let recording: serde_json::Value = http
        .post(format!("{base}/api/recordings"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "room_id": room_id, "content_type": "audio/ogg" }))
        .send()
        .await
        .expect("start recording")
        .json()
        .await
        .expect("recording body");
    let recording_id = recording["id"].as_str().expect("recording id").to_string();
    assert_eq!(recording["status"], "recording");
    assert_eq!(recording["content_type"], "audio/ogg");

    // Upload segment 1 and 2 (base64 JSON transport in the reference app).
    let upload = |part: u32, bytes: &[u8], duration_ms: u64| {
        let body = serde_json::json!({
            "data_base64": data_encoding::BASE64.encode(bytes),
            "duration_ms": duration_ms,
            "started_at_unix": 1_700_000_000 + part,
        });
        http.post(format!("{base}/api/recordings/{recording_id}/parts/{part}"))
            .bearer_auth(&alice_token)
            .json(&body)
            .send()
    };
    let after_first: serde_json::Value = upload(1, b"hello-", 1_000)
        .await
        .expect("upload part 1")
        .json()
        .await
        .expect("part body");
    assert_eq!(
        after_first["segments"].as_array().expect("segments").len(),
        1
    );
    assert_eq!(after_first["total_size"], 6);
    let after_second: serde_json::Value = upload(2, b"world", 1_500)
        .await
        .expect("upload part 2")
        .json()
        .await
        .expect("part body");
    assert_eq!(
        after_second["segments"].as_array().expect("segments").len(),
        2
    );
    assert_eq!(after_second["total_size"], 11);
    assert_eq!(after_second["total_duration_ms"], 2_500);

    // Retrying the same part is a resumable retry: no duplicate segment entry.
    let retried: serde_json::Value = upload(2, b"world", 1_500)
        .await
        .expect("retry part 2")
        .json()
        .await
        .expect("retry body");
    assert_eq!(retried["segments"].as_array().expect("segments").len(), 2);
    assert_eq!(retried["total_size"], 11);

    // Complete: playable recording with an ordered playback index.
    let completed: serde_json::Value = http
        .post(format!("{base}/api/recordings/{recording_id}/complete"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("complete recording")
        .json()
        .await
        .expect("complete body");
    assert_eq!(completed["status"], "completed");
    assert!(completed["completed_at_unix"].as_u64().is_some());
    let segments = completed["segments"].as_array().expect("segments");
    assert_eq!(segments[0]["part"], 1);
    assert_eq!(segments[1]["part"], 2);
    assert_eq!(segments[1]["size"], 5);

    // Playback: raw bytes with the recording's media type.
    let part_one = http
        .get(format!("{base}/api/recordings/{recording_id}/parts/1"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("play part 1");
    assert_eq!(part_one.status(), reqwest::StatusCode::OK);
    assert_eq!(
        part_one
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/ogg")
    );
    assert_eq!(
        part_one.bytes().await.expect("part bytes").as_ref(),
        b"hello-"
    );
    let part_two = http
        .get(format!("{base}/api/recordings/{recording_id}/parts/2"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("play part 2");
    assert_eq!(
        part_two.bytes().await.expect("part bytes").as_ref(),
        b"world"
    );

    // Metadata lookup.
    let fetched: serde_json::Value = http
        .get(format!("{base}/api/recordings/{recording_id}"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("get recording")
        .json()
        .await
        .expect("recording body");
    assert_eq!(fetched["status"], "completed");
    assert_eq!(fetched["total_size"], 11);

    // Non-members cannot read records, start recordings, or upload segments.
    let forbidden = http
        .get(format!("{base}/api/recordings/{recording_id}"))
        .bearer_auth(&charlie_token)
        .send()
        .await
        .expect("charlie get");
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);
    let forbidden = http
        .post(format!("{base}/api/recordings"))
        .bearer_auth(&charlie_token)
        .json(&serde_json::json!({ "room_id": room_id }))
        .send()
        .await
        .expect("charlie start");
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);

    // Missing segment -> 404.
    let missing = http
        .get(format!("{base}/api/recordings/{recording_id}/parts/9"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("missing part");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    // Completing with no segments is a validation error.
    let empty: serde_json::Value = http
        .post(format!("{base}/api/recordings"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "room_id": room_id }))
        .send()
        .await
        .expect("start empty recording")
        .json()
        .await
        .expect("empty recording body");
    let empty_id = empty["id"].as_str().expect("id");
    let invalid = http
        .post(format!("{base}/api/recordings/{empty_id}/complete"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("complete empty");
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);

    handle.shutdown();
}
