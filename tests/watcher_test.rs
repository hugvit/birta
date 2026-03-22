use std::time::Duration;

use tempfile::NamedTempFile;
use tokio::sync::broadcast;
use tokio::time::timeout;

#[tokio::test]
async fn watcher_broadcasts_on_file_change() {
    let tmpfile = NamedTempFile::new().unwrap();
    let path = tmpfile.path().to_path_buf();
    std::fs::write(&path, "# Hello").unwrap();

    let (tx, mut rx) = broadcast::channel::<String>(16);
    let _debouncer = sheen::watcher::watch(path.clone(), tx).unwrap();

    // Give the watcher time to settle and drain any initial events
    tokio::time::sleep(Duration::from_millis(500)).await;
    while rx.try_recv().is_ok() {}

    // Overwrite the file (triggers modify event reliably)
    std::fs::write(&path, "# Updated content").unwrap();

    let result = timeout(Duration::from_secs(5), rx.recv()).await;
    let html = result
        .expect("timed out waiting for watcher event")
        .expect("broadcast recv failed");

    assert!(
        html.contains("Updated content"),
        "broadcast should contain rendered HTML from updated file, got: {html}"
    );
}
