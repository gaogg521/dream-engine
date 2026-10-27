use super::*;

#[tokio::test]
async fn dropping_transport_aborts_listener_instead_of_detaching() {
    let listener = tokio::spawn(std::future::pending::<()>());
    let abort = listener.abort_handle();
    let transport = SseTransport {
        client: reqwest::Client::new(),
        post_url: "http://127.0.0.1/unused".into(),
        headers: HeaderMap::new(),
        pending: Arc::new(Mutex::new(HashMap::new())),
        next_id: AtomicU64::new(1),
        _listener: listener,
    };
    drop(transport);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !abort.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping SSE must cancel its listener");
}
