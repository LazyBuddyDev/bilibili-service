use crate::api::{self, Media, Result};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
pub struct Resource {
    pub source: Media,
    pub token: String,
    pub origin: String,
    pub id: u64,
    pub active: AtomicBool,
    pub closed: AtomicBool,
    pub requests: AtomicU64,
    pub bytes: AtomicU64,
}
pub struct Server {
    port: u16,
    pub current: Arc<Mutex<Option<Arc<Resource>>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
pub fn random() -> Result<String> {
    let mut b = [0u8; 32];
    let s = unsafe {
        windows_sys::Win32::Security::Cryptography::BCryptGenRandom(
            std::ptr::null_mut(),
            b.as_mut_ptr(),
            32,
            windows_sys::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if s != 0 {
        return Err("random_failed");
    }
    Ok(b.iter().map(|v| format!("{v:02x}")).collect())
}
pub fn origin(s: &str) -> bool {
    s == "http://tauri.localhost"
        || s == "tauri://localhost"
        || s.parse::<ureq::http::Uri>().is_ok_and(|u| {
            u.scheme_str() == Some("http")
                && u.host() == Some("127.0.0.1")
                && u.authority()
                    .is_some_and(|a| a.port_u16().is_some() && !a.as_str().contains('@'))
                && u.path() == "/"
                && u.query().is_none()
        })
}
impl Server {
    pub fn new() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "resource_server_failed")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "resource_server_failed")?;
        let port = listener
            .local_addr()
            .map_err(|_| "resource_server_failed")?
            .port();
        let current: Arc<Mutex<Option<Arc<Resource>>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let state = current.clone();
        let done = stop.clone();
        let inflight = Arc::new(AtomicU64::new(0));
        let worker = std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        if inflight.fetch_add(1, Ordering::AcqRel) >= 4 {
                            inflight.fetch_sub(1, Ordering::AcqRel);
                            drop(socket);
                            continue;
                        }
                        let state = state.clone();
                        let count = inflight.clone();
                        std::thread::spawn(move || {
                            let _ = serve(socket, port, &state);
                            count.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            port,
            current,
            stop,
            worker: Some(worker),
        })
    }
    pub fn open(&self, source: Media, origin: String, id: u64) -> Result<Value> {
        self.close();
        let r = Arc::new(Resource {
            source,
            origin,
            id,
            token: random()?,
            active: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            requests: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        });
        let result = self.describe(&r);
        *self.current.lock().unwrap() = Some(r);
        Ok(result)
    }
    fn describe(&self, r: &Resource) -> Value {
        json!({"session":r.id,"resource":{"url":format!("http://127.0.0.1:{}/media/{}",self.port,r.token),"mime":"video/mp4"},"title":r.source.title,"bvid":r.source.bvid,"cid":r.source.cid,"duration":r.source.duration,"quality":r.source.quality,"entries":r.source.entries,"suspended":!r.active.load(Ordering::Acquire)})
    }
    pub fn control(&self, id: u64, active: bool) -> Result<Value> {
        let lock = self.current.lock().unwrap();
        let r = lock
            .as_ref()
            .filter(|r| r.id == id && !r.closed.load(Ordering::Acquire))
            .ok_or("session_not_found")?;
        r.active.store(active, Ordering::Release);
        Ok(self.describe(r))
    }
    pub fn close(&self) {
        if let Some(r) = self.current.lock().unwrap().take() {
            r.active.store(false, Ordering::Release);
            r.closed.store(true, Ordering::Release);
        }
    }
    pub fn suspend(&self) {
        if let Some(r) = self.current.lock().unwrap().as_ref() {
            r.active.store(false, Ordering::Release);
        }
    }
    pub fn counters(&self) -> Value {
        let lock = self.current.lock().unwrap();
        lock.as_ref().map(|r|json!({"requests":r.requests.load(Ordering::Acquire),"bytes":r.bytes.load(Ordering::Acquire),"entries":r.source.entries,"suspended":!r.active.load(Ordering::Acquire)})).unwrap_or(json!({"requests":0,"bytes":0,"suspended":true}))
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.close();
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
fn range(s: &str, size: u64) -> Result<(u64, u64)> {
    let r = s.strip_prefix("bytes=").ok_or("invalid_range")?;
    let (a, b) = r.split_once('-').ok_or("invalid_range")?;
    if r.contains(',') || a.is_empty() {
        return Err("invalid_range");
    }
    let a = a.parse::<u64>().map_err(|_| "invalid_range")?;
    let b = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>().map_err(|_| "invalid_range")?
    };
    if a >= size || b < a || b >= size {
        return Err("invalid_range");
    }
    Ok((a, b.min(a.saturating_add(1024 * 1024 - 1))))
}
fn reject(s: &mut TcpStream, code: u16) -> Result<()> {
    write!(
        s,
        "HTTP/1.1 {code} Rejected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(|_| "resource_io_failed")
}
fn serve(mut socket: TcpStream, port: u16, state: &Mutex<Option<Arc<Resource>>>) -> Result<()> {
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "resource_io_failed")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "resource_io_failed")?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|_| "resource_io_failed")?);
    let mut first = String::new();
    reader
        .by_ref()
        .take(4097)
        .read_line(&mut first)
        .map_err(|_| "resource_io_failed")?;
    if first.len() > 4096 {
        return reject(&mut socket, 400);
    }
    let parts = first.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 {
        return reject(&mut socket, 400);
    }
    let mut headers = std::collections::BTreeMap::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        reader
            .by_ref()
            .take(8193)
            .read_line(&mut line)
            .map_err(|_| "resource_io_failed")?;
        total += line.len();
        if total > 16384 {
            return reject(&mut socket, 400);
        }
        if line == "\r\n" {
            break;
        }
        let (k, v) = line.split_once(':').ok_or("invalid_headers")?;
        if headers
            .insert(k.trim().to_ascii_lowercase(), v.trim().to_string())
            .is_some()
        {
            return reject(&mut socket, 400);
        }
    }
    let Some(r) = state.lock().unwrap().clone() else {
        return reject(&mut socket, 410);
    };
    if headers.get("host") != Some(&format!("127.0.0.1:{port}"))
        || headers.get("origin") != Some(&r.origin)
        || parts[1] != format!("/media/{}", r.token)
        || r.closed.load(Ordering::Acquire)
    {
        return reject(&mut socket, 403);
    }
    if parts[0] == "OPTIONS" {
        return write!(socket,"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: {}\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: Range\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",r.origin).map_err(|_|"resource_io_failed");
    }
    if !matches!(parts[0], "GET" | "HEAD") {
        return reject(&mut socket, 405);
    }
    if !r.active.load(Ordering::Acquire) {
        return reject(&mut socket, 423);
    }
    let (a, b) = match range(
        headers
            .get("range")
            .map(String::as_str)
            .unwrap_or("bytes=0-"),
        r.source.size,
    ) {
        Ok(r) => r,
        Err(_) => return reject(&mut socket, 416),
    };
    if parts[0] == "HEAD" {
        return write!(socket,"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: {}\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",r.origin,r.source.size).map_err(|_|"resource_io_failed");
    }
    r.requests.fetch_add(1, Ordering::AcqRel);
    let response = api::agent()
        .get(&r.source.url)
        .header("User-Agent", api::UA)
        .header("Referer", api::REFERER)
        .header("Range", format!("bytes={a}-{b}"))
        .call();
    let mut response = match response {
        Ok(v) => v,
        Err(e) => {
            if matches!(e, ureq::Error::StatusCode(401 | 403)) {
                r.active.store(false, Ordering::Release);
                r.closed.store(true, Ordering::Release);
            }
            return reject(&mut socket, 502);
        }
    };
    let expected = format!("bytes {a}-{b}/{}", r.source.size);
    if response.status() != 206
        || response
            .headers()
            .get("content-range")
            .and_then(|h| h.to_str().ok())
            != Some(expected.as_str())
    {
        r.active.store(false, Ordering::Release);
        return reject(&mut socket, 502);
    }
    if !r.active.load(Ordering::Acquire) || r.closed.load(Ordering::Acquire) {
        return reject(&mut socket, 423);
    }
    write!(socket,"HTTP/1.1 206 Partial Content\r\nAccess-Control-Allow-Origin: {}\r\nAccess-Control-Expose-Headers: Content-Range\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Range: {expected}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",r.origin,b-a+1).map_err(|_|"resource_io_failed")?;
    let mut stream = response.body_mut().as_reader().take(b - a + 1);
    let mut buf = [0u8; 32768];
    loop {
        if !r.active.load(Ordering::Acquire) || r.closed.load(Ordering::Acquire) {
            break;
        }
        let n = stream.read(&mut buf).map_err(|_| "resource_io_failed")?;
        if n == 0 {
            break;
        }
        if !r.active.load(Ordering::Acquire) || r.closed.load(Ordering::Acquire) {
            break;
        }
        socket
            .write_all(&buf[..n])
            .map_err(|_| "resource_io_failed")?;
        r.bytes.fetch_add(n as u64, Ordering::AcqRel);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_sessions_cannot_control_replacements() {
        let server = Server::new().unwrap();
        let media = || Media {
            url: "https://upos.bilivideo.com/fixture".into(),
            size: 100,
            title: "fixture".into(),
            duration: 1.,
            quality: 32,
            bvid: "BV1oJ41137oE".into(),
            cid: "1".into(),
            entries: json!({"items":[],"complete":true}),
        };
        server
            .open(media(), "http://tauri.localhost".into(), 1)
            .unwrap();
        server
            .open(media(), "http://tauri.localhost".into(), 2)
            .unwrap();
        assert!(server.control(1, false).is_err());
        assert_eq!(server.control(2, true).unwrap()["suspended"], false);
        server.suspend();
        assert_eq!(server.counters()["suspended"], true);
        server.close();
        assert!(server.control(2, true).is_err());
    }
    #[test]
    fn bounded_ranges_and_origins() {
        assert_eq!(range("bytes=0-", 10_000_000).unwrap(), (0, 1048575));
        assert_eq!(range("bytes=3-5", 10).unwrap(), (3, 5));
        for s in ["bytes=-100", "bytes=1-5,8-9", "bytes=10-11", "bytes=3-2"] {
            assert!(range(s, 10).is_err());
        }
        assert!(origin("http://127.0.0.1:5139"));
        assert!(origin("http://tauri.localhost"));
        assert!(!origin("https://evil.test"));
        assert!(!origin("http://127.0.0.1:5139/path"));
    }
}
