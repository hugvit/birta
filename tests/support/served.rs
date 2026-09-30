//! A real `birta` process serving a temp folder, driven over its socket and HTTP. The
//! env is cleared, so the gate, the hold and the drafts dir come only from what a test sets.
// Each test binary that includes the kit uses a different part of it
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// A socket, with the pushes an exchange read past kept for the next read.
pub struct Ws {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    backlog: VecDeque<Value>,
}

pub const GATE_ON: (&str, &str) = ("BIRTA_REVIEW", "1");
/// How long a reply may take before a test calls it missing.
pub const WAIT: Duration = Duration::from_secs(5);
// Spawns and first connects stall under a fully parallel `cargo test`
const CONNECT: Duration = Duration::from_secs(30);

/// `<tmp>/root` is served from; `home/` and `state/` sit beside it.
pub struct Site {
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub root: PathBuf,
}

impl Site {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("root");
        for dir in [&root, &base.join("home"), &base.join("state")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.path(rel)).ok()
    }

    /// `$XDG_STATE_HOME/birta/drafts` of every served process.
    pub fn drafts_dir(&self) -> PathBuf {
        self.base.join("state/birta/drafts")
    }

    /// Every file under the drafts dir.
    pub fn drafts(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.drafts_dir(), &mut out);
        out
    }

    /// Starts `birta args… --no-open --port 0` in the served folder.
    pub fn serve(&self, args: &[&str], envs: &[(&str, &str)]) -> Served {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin("birta"))
            .args(args)
            .args(["--no-open", "--port", "0"])
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.base.join("home"))
            .env("XDG_STATE_HOME", self.base.join("state"))
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            // Read to the end, so the server never writes to a closed pipe
            for line in BufReader::new(stderr).lines() {
                let _ = tx.send(line.unwrap_or_default());
            }
        });
        let mut served = Served {
            child,
            port: 0,
            stderr: Vec::new(),
        };
        loop {
            let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) else {
                panic!(
                    "no `serving` line from birta {args:?}: {:#?}",
                    served.stderr
                );
            };
            served.stderr.push(line.clone());
            if let Some((_, addr)) = line.split_once(" at http://") {
                served.port = addr.rsplit(':').next().unwrap().trim().parse().unwrap();
                return served;
            }
        }
    }
}

/// A served `birta` process, killed on drop so a failing test leaves nothing running.
pub struct Served {
    child: Child,
    pub port: u16,
    pub stderr: Vec<String>,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// A socket past its connect-time `content` frame.
    pub async fn connect(&self) -> Ws {
        let url = format!("ws://127.0.0.1:{}/ws", self.port);
        let (socket, _) = tokio::time::timeout(CONNECT, connect_async(&url))
            .await
            .expect("connect timed out")
            .unwrap();
        let mut ws = Ws {
            socket,
            backlog: VecDeque::new(),
        };
        let first = next_within(&mut ws, CONNECT)
            .await
            .expect("no content frame after connect");
        assert_eq!(first["type"], "content", "{first}");
        ws
    }

    /// A socket that has said hello for `path`, past the hello's own frames.
    pub async fn viewing(&self, path: &str) -> (Ws, Vec<Value>) {
        let mut ws = self.connect().await;
        let frames = hello(&mut ws, path).await;
        (ws, frames)
    }

    /// `GET path`, returning the status and the raw response.
    pub async fn get(&self, path: &str) -> (u16, String) {
        self.request("GET", path, &[], None).await
    }

    /// One HTTP/1.1 request with `Host` set to this server.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n",
            self.host()
        );
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(body) = body {
            head.push_str(&format!("content-length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        head.push_str(body.unwrap_or(""));
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
            .await
            .expect("timed out reading response")
            .unwrap();
        let response = String::from_utf8_lossy(&response).into_owned();
        let status = response
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("bad response: {response:?}"));
        (status, response)
    }

    /// The BOOT JSON of `/view/<rel>`.
    pub async fn boot(&self, rel: &str) -> Value {
        let (status, page) = self.get(&format!("/view/{rel}")).await;
        assert_eq!(status, 200, "{page}");
        boot_of(&page)
    }
}

pub fn boot_of(page: &str) -> Value {
    let start = page.find("id=\"birta-boot\"").expect("no birta-boot block");
    let body = &page[start..];
    let open = body.find('>').unwrap() + 1;
    let close = body.find("</script>").unwrap();
    serde_json::from_str(&body[open..close]).unwrap()
}

/// The body of a raw HTTP response.
pub fn body_of(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The value of header `name` in a raw response, matched case-insensitively.
pub fn header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response
        .split("\r\n\r\n")
        .next()?
        .lines()
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
}

pub async fn send(ws: &mut Ws, frame: Value) {
    ws.socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
}

/// The next JSON frame, or `None` when nothing arrives within `wait`.
pub async fn next_within(ws: &mut Ws, wait: Duration) -> Option<Value> {
    if let Some(frame) = ws.backlog.pop_front() {
        return Some(frame);
    }
    loop {
        let msg = tokio::time::timeout(wait, ws.socket.next())
            .await
            .ok()??
            .ok()?;
        if let Message::Text(text) = msg {
            return Some(serde_json::from_str(&text).unwrap());
        }
    }
}

pub async fn next(ws: &mut Ws) -> Value {
    next_within(ws, WAIT)
        .await
        .expect("no frame from the server in time")
}

/// Reads until a frame matches, returning the frames before it and the match.
pub async fn until(ws: &mut Ws, what: &str, pred: impl Fn(&Value) -> bool) -> (Vec<Value>, Value) {
    let mut seen = Vec::new();
    loop {
        let Some(frame) = next_within(ws, WAIT).await else {
            panic!("never got {what}; saw {seen:#?}");
        };
        if pred(&frame) {
            return (seen, frame);
        }
        seen.push(frame);
    }
}

/// Every frame that arrives within `wait`.
pub async fn drain(ws: &mut Ws, wait: Duration) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Some(frame) = next_within(ws, wait).await {
        frames.push(frame);
    }
    frames
}

/// A frame that gets a reply and changes nothing, so its reply marks every earlier
/// frame on the socket as handled.
pub fn probe(cid: &str) -> Value {
    json!({"type": "checkbox", "cid": cid})
}

/// Sends `frame`, then a probe, and returns the frames before the probe's reply.
pub async fn exchange(ws: &mut Ws, frame: Value) -> Vec<Value> {
    send(ws, frame).await;
    send(ws, probe("probe")).await;
    until(ws, "the probe's reply", |f| f["cid"] == "probe")
        .await
        .0
}

/// Sends `hello` for `path` and returns its frames.
pub async fn hello(ws: &mut Ws, path: &str) -> Vec<Value> {
    exchange(ws, json!({"type": "hello", "path": path, "tab": "t1"})).await
}

/// The reply to the frame with `cid`, skipping pushes. `None` when the frames after
/// it are handled first, meaning it got none.
pub async fn reply(ws: &mut Ws, frame: Value) -> Option<Value> {
    let cid = frame["cid"].clone();
    let is_reply = |f: &Value| {
        f["cid"] == cid && (f["type"] == "ack" || f["type"] == "nack" || f["type"] == "resolved")
    };
    let mut frames = exchange(ws, frame).await;
    let found = frames.iter().position(is_reply).map(|i| frames.remove(i));
    // Pushes that came with the reply stay readable, in order
    ws.backlog.extend(frames);
    found
}

/// The ack of `frame`, failing the test on anything else.
pub async fn acked(ws: &mut Ws, frame: Value) -> Value {
    let got = reply(ws, frame.clone()).await;
    match got {
        Some(r) if r["type"] == "ack" => r,
        other => panic!("want an ack of {frame}, got {other:?}"),
    }
}

/// The nack reason for `frame`, failing the test on anything else.
pub async fn nacked(ws: &mut Ws, frame: Value) -> String {
    let got = reply(ws, frame.clone()).await;
    match got {
        Some(r) if r["type"] == "nack" => r["reason"].as_str().unwrap().to_string(),
        other => panic!("want a nack of {frame}, got {other:?}"),
    }
}

/// Every parsed line of the review file at `path`; empty when there is none.
pub fn log_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The lines of the review file whose op is `op`.
pub fn ops_named(path: &Path, op: &str) -> Vec<Value> {
    log_lines(path)
        .into_iter()
        .filter(|l| l["op"] == op)
        .collect()
}

/// Polls `check` until it holds, failing with `what` after [`WAIT`].
pub async fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !check() {
        assert!(std::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
