//! A tiny HTTP server for tests: `std::net::TcpListener` on `127.0.0.1:0`,
//! plain `http`, one thread. It records every request as it arrived and
//! answers with whatever the test's handler says. No test talks to a real API.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One request as the stub read it off the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    /// Path and query, exactly as sent (`/api/x?a=1`).
    pub target: String,
    /// Names lowercase, in the order sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    /// The first header with this name (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What the stub does with a request.
pub enum Reply {
    /// Answer with this status, extra headers and body.
    Respond {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Read the request and never answer (the connection stays open until
    /// the stub is dropped).
    Silent,
}

impl Reply {
    /// A JSON answer.
    pub fn json(status: u16, body: &str) -> Reply {
        Reply::Respond {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.as_bytes().to_vec(),
        }
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

/// A running stub. Dropping it stops the accept loop and closes silent
/// connections.
pub struct Stub {
    pub base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    held: Arc<Mutex<Vec<TcpStream>>>,
    port: u16,
}

impl Stub {
    pub fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let port = listener.local_addr().expect("stub address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let held = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        {
            let requests = Arc::clone(&requests);
            let connections = Arc::clone(&connections);
            let stop = Arc::clone(&stop);
            let held = Arc::clone(&held);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    connections.fetch_add(1, Ordering::SeqCst);
                    let requests = Arc::clone(&requests);
                    let held = Arc::clone(&held);
                    let handler = Arc::clone(&handler);
                    std::thread::spawn(move || serve(stream, &*handler, &requests, &held));
                }
            });
        }
        Stub {
            base_url: format!("http://127.0.0.1:{port}"),
            requests,
            connections,
            stop,
            held,
            port,
        }
    }

    /// Every request read so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("stub lock").clone()
    }

    /// How many connections were accepted.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        self.held.lock().expect("stub lock").clear();
    }
}

fn serve(
    mut stream: TcpStream,
    handler: &Handler,
    requests: &Mutex<Vec<Recorded>>,
    held: &Mutex<Vec<TcpStream>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(recorded) = read_request(&mut stream) else {
        return;
    };
    requests.lock().expect("stub lock").push(recorded.clone());
    match handler(&recorded) {
        Reply::Silent => held.lock().expect("stub lock").push(stream),
        Reply::Respond {
            status,
            headers,
            body,
        } => {
            let mut head = format!("HTTP/1.1 {status} Stub\r\n");
            for (name, value) in &headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str(&format!(
                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ));
            // The client may hang up early (a body over its limit).
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
            // Wait for the client to hang up, as a real server's keep-alive
            // would: closing first races the client's socket setup, and
            // macOS then fails `setsockopt` with EINVAL.
            let mut rest = [0u8; 1024];
            while matches!(stream.read(&mut rest), Ok(read) if read > 0) {}
        }
    }
}

/// Read one request: the head up to the blank line, then `Content-Length`
/// bytes of body.
fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(end) = find(&data, b"\r\n\r\n") {
            break end;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Some(Recorded {
        method,
        target,
        headers,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
