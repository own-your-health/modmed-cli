//! A stand-in for ModMed's endpoint directory on 127.0.0.1, serving the
//! synthetic pages in `tests/fixtures/directory` with their links pointed
//! at itself. For the tests and `make mock`; every value is synthetic.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const PAGE1: &str = include_str!("../fixtures/directory/ema-page1.json");
const PAGE2: &str = include_str!("../fixtures/directory/ema-page2.json");

/// The origin the fixture pages name, replaced by the server's own.
const MODMED_ORIGIN: &str = "https://public-api.mmi.prod.fhir.ema-api.com";

pub struct DirectoryServer {
    origin: String,
    requests: Arc<AtomicUsize>,
    failing: Arc<AtomicBool>,
}

impl DirectoryServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let failing = Arc::new(AtomicBool::new(false));
        let (o, r, f) = (origin.clone(), requests.clone(), failing.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                r.fetch_add(1, Ordering::SeqCst);
                let (status, body) = if f.load(Ordering::SeqCst) {
                    ("503 Service Unavailable", String::new())
                } else if !target.starts_with("/fhir/r4/Endpoint") {
                    ("404 Not Found", String::new())
                } else if target.contains("_offset=2") {
                    ("200 OK", PAGE2.replace(MODMED_ORIGIN, &o))
                } else {
                    ("200 OK", PAGE1.replace(MODMED_ORIGIN, &o))
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/fhir+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        DirectoryServer {
            origin,
            requests,
            failing,
        }
    }

    /// The directory's first page, as `config set directory_url` takes it.
    pub fn url(&self) -> String {
        format!(
            "{}/fhir/r4/Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest",
            self.origin
        )
    }

    /// Requests answered so far.
    #[allow(dead_code)]
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Answer every request with a 503 from now on (`true`), or the pages
    /// again (`false`).
    #[allow(dead_code)]
    pub fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }
}
