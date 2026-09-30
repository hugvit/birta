use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Serves `file` with default options on an ephemeral port.
pub async fn start_server(file: PathBuf) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let variant = || {
        Box::new(birta::theme::VariantData {
            css_vars: String::new(),
            syntax: None,
        })
    };
    let opts = birta::server::ServerOptions {
        port: 0,
        no_open: true,
        custom_css: None,
        font_css: None,
        theme: birta::theme::ResolvedTheme {
            name: "github".to_string(),
            variants: birta::theme::ThemeVariants::Both {
                light: variant(),
                dark: variant(),
            },
            active_variant: birta::theme::Variant::Dark,
        },
        enable_swap: false,
        enable_toggle: true,
        show_header: true,
        reading_mode: false,
        raw_mode: false,
        keybindings_json: "{}".to_string(),
        variant_explicit: false,
        drafted_by: None,
    };
    // The listener is already bound, so connects queue until the server accepts.
    tokio::spawn(birta::server::start(file, listener, opts));
    port
}

/// Sends one HTTP/1.1 request over raw TCP with exactly `headers`, so Host can be
/// omitted or forged, and returns the response status.
pub async fn http_request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> u16 {
    let response = http_exchange(port, method, path, headers).await;
    let status_line = response.lines().next().unwrap_or_default();
    status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("bad status line: {status_line:?}"))
}

/// Like [`http_request`], returning the whole response as text.
pub async fn http_exchange(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nconnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut response))
        .await
        .expect("timed out reading response")
        .unwrap();
    String::from_utf8_lossy(&response).into_owned()
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
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
}

pub async fn http_get(port: u16, path: &str, headers: &[(&str, &str)]) -> u16 {
    http_request(port, "GET", path, headers).await
}
