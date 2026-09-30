//! One render on several machines. `geneva farm` plans the parts that
//! `geneva plan` describes, serves the timeline and the files it reads
//! over HTTP, hands the parts out to `geneva worker` processes as they
//! ask for them, and joins what comes back. Workers pull: a fast machine
//! asks more often and so renders more parts, a worker that stops
//! answering has its part given to another, and a worker can join at any
//! time. The coordinator never needs to know where its workers run.
//!
//! The protocol is plain HTTP/1.1 with JSON bodies, one request per
//! connection, and a shared token in an `Authorization: Bearer` header:
//!
//! - `POST /hello`: a worker names its geneva version and encoder; the
//!   answer is the job (the timeline, the files, the render settings) or
//!   409 with why the worker cannot take part.
//! - `GET /file/<n>`: the `n`th file the timeline reads.
//! - `POST /claim`: the next part to render (200), nothing yet (202, with
//!   how long to wait), or nothing more (204).
//! - `PUT /part/<n>`: a finished part's bytes.
//! - `POST /failed/<n>`, `POST /refuse`: a part that could not be
//!   rendered, or a worker that cannot render this job at all.
//! - `GET /status`, `GET /metrics`: progress as JSON, and as Prometheus
//!   text for an autoscaler. These two need no token.
//!
//! Nothing is encrypted: on a network that is not trusted, put the
//! coordinator behind a tunnel or a proxy that adds TLS.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use geneva_render::RenderError;
use geneva_timeline::{Composition, Diagnostic, Severity};

use crate::media::{self, Part, RenderMode, RenderOverrides, RenderStats};

/// How many times a part may fail before the whole render does.
const MAX_ATTEMPTS: u32 = 3;

/// How long a worker waits before asking again when every part is taken.
const WAIT_MS: u64 = 1000;

/// A part still running this many times longer than the median finished
/// part is given to a second worker too, and the first copy back wins.
const STRAGGLER_FACTOR: f64 = 2.0;

/// The geneva build and encoder a part's bytes depend on. Parts from two
/// builds, or from a machine with x264 and one without, do not join.
pub fn fingerprint() -> String {
    #[cfg(feature = "media")]
    let x264 =
        geneva_media::system_x264().map_or_else(|| "none".to_owned(), |l| l.build.to_string());
    #[cfg(not(feature = "media"))]
    let x264 = "no media support".to_owned();
    format!("geneva {}; x264 {x264}", env!("CARGO_PKG_VERSION"))
}

/// What `geneva farm` was asked to do beyond the render itself.
pub struct FarmOptions {
    /// How many picture parts.
    pub parts: u32,
    /// The address to listen on.
    pub listen: String,
    /// The shared secret, or `None` for a random one.
    pub token: Option<String>,
    /// Workers started on this machine.
    pub local: u32,
    /// A shell command that starts one worker, run `launch_count` times.
    pub launch: Option<String>,
    /// How many times to run `launch`; the number of parts by default.
    pub launch_count: Option<u32>,
    /// How long a part may run before it is given to another worker.
    pub part_timeout: Duration,
    /// Files the timeline reads, relative to the root.
    pub files: Vec<String>,
    /// The timeline's text, as the workers will load it.
    pub timeline: String,
    /// Warnings and errors the coordinator's own load reported, as
    /// `code path`, so that a worker whose load differs can say so.
    pub diagnostics: Vec<String>,
}

/// `code path` for each warning and error: what a worker's load must not
/// add to.
pub fn diagnostic_keys(diagnostics: &[Diagnostic]) -> Vec<String> {
    let mut keys: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.severity >= Severity::Warning)
        .map(|d| format!("{} {}", d.code, d.path))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Whether `path` is a relative path that stays under the root.
fn safe_relative(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && p.is_relative()
        && p.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

// ---------------------------------------------------------------------
// HTTP, as much of it as the two ends need.

/// A request as the server reads it: the head parsed, the body left on
/// the stream.
struct Request {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    token: Option<String>,
    length: u64,
    /// A `Range: bytes=a-b` header, as the byte range `a..b + 1`.
    range: Option<std::ops::Range<u64>>,
    body: BufReader<TcpStream>,
}

impl Request {
    fn read(stream: TcpStream) -> std::io::Result<Self> {
        let mut body = BufReader::new(stream);
        let mut line = String::new();
        body.read_line(&mut line)?;
        let mut words = line.split_whitespace();
        let method = words.next().unwrap_or_default().to_owned();
        let target = words.next().unwrap_or_default().to_owned();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let query = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let mut length = 0;
        let mut token = None;
        let mut range = None;
        loop {
            line.clear();
            if body.read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let value = value.trim();
                match name.trim().to_ascii_lowercase().as_str() {
                    "content-length" => length = value.parse().unwrap_or(0),
                    "authorization" => {
                        token = value.strip_prefix("Bearer ").map(str::to_owned);
                    }
                    "range" => {
                        range = value
                            .strip_prefix("bytes=")
                            .and_then(|r| r.split_once('-'))
                            .and_then(|(a, b)| {
                                Some(a.parse::<u64>().ok()?..b.parse::<u64>().ok()? + 1)
                            });
                    }
                    _ => {}
                }
            }
        }
        Ok(Self {
            method,
            path: path.to_owned(),
            query,
            token,
            length,
            range,
            body,
        })
    }

    /// The body, when it is small enough to hold: a JSON message.
    fn json(&mut self) -> serde_json::Value {
        if self.length > 1 << 20 {
            return serde_json::Value::Null;
        }
        let mut bytes = vec![0; self.length as usize];
        if self.body.read_exact(&mut bytes).is_err() {
            return serde_json::Value::Null;
        }
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    fn respond(&mut self, status: u16, content_type: &str, body: &[u8]) {
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            409 => "Conflict",
            416 => "Range Not Satisfiable",
            _ => "Error",
        };
        let stream = self.body.get_mut();
        let _ = write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    fn respond_json(&mut self, status: u16, body: &serde_json::Value) {
        self.respond(status, "application/json", body.to_string().as_bytes());
    }

    /// Sends the file, or the part of it the request's range names.
    /// Returns the bytes sent.
    fn respond_file(&mut self, path: &Path) -> u64 {
        use std::io::Seek;
        let Ok(mut file) = std::fs::File::open(path) else {
            self.respond(404, "text/plain", b"not found");
            return 0;
        };
        let size = file.metadata().map_or(0, |m| m.len());
        let (status, range) = match self.range.clone() {
            Some(r) if r.start < r.end && r.end <= size => ("206 Partial Content", r),
            Some(_) => {
                self.respond(416, "text/plain", b"the range is outside the file");
                return 0;
            }
            None => ("200 OK", 0..size),
        };
        if file.seek(std::io::SeekFrom::Start(range.start)).is_err() {
            self.respond(404, "text/plain", b"not readable");
            return 0;
        }
        let length = range.end - range.start;
        let stream = self.body.get_mut();
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        );
        let sent = std::io::copy(&mut file.take(length), stream).unwrap_or(0);
        let _ = stream.flush();
        sent
    }
}

/// The coordinator as a worker sees it: `http://host:port`.
#[derive(Clone)]
pub struct Client {
    address: String,
    token: String,
}

/// What a request carries.
enum Body<'a> {
    Empty,
    Json(&'a serde_json::Value),
    File(&'a Path),
}

impl Client {
    pub fn new(url: &str, token: &str) -> Result<Self, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("{url:?} is not an http:// address"))?;
        let address = rest.trim_end_matches('/').to_owned();
        if address.is_empty() || address.contains('/') {
            return Err(format!("{url:?} is not http://host:port"));
        }
        Ok(Self {
            address,
            token: token.to_owned(),
        })
    }

    /// Sends one request and returns the status and the connection with
    /// the body still to read, and the body's length.
    fn send(
        &self,
        method: &str,
        path: &str,
        body: &Body<'_>,
    ) -> std::io::Result<(u16, u64, BufReader<TcpStream>)> {
        self.send_range(method, path, body, None)
    }

    fn send_range(
        &self,
        method: &str,
        path: &str,
        body: &Body<'_>,
        range: Option<&std::ops::Range<u64>>,
    ) -> std::io::Result<(u16, u64, BufReader<TcpStream>)> {
        let mut stream = TcpStream::connect(&self.address)?;
        let (length, json) = match body {
            Body::Empty => (0, None),
            Body::Json(v) => {
                let text = v.to_string();
                (text.len() as u64, Some(text))
            }
            Body::File(p) => (std::fs::metadata(p)?.len(), None),
        };
        let range = range.map_or_else(String::new, |r| {
            format!("Range: bytes={}-{}\r\n", r.start, r.end - 1)
        });
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n{range}Content-Length: {length}\r\nConnection: close\r\n\r\n",
            self.address, self.token
        )?;
        match body {
            Body::Empty => {}
            Body::Json(_) => stream.write_all(json.unwrap_or_default().as_bytes())?,
            Body::File(p) => {
                std::io::copy(&mut std::fs::File::open(p)?, &mut stream)?;
            }
        }
        stream.flush()?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let status = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| std::io::Error::other(format!("not an HTTP answer: {line:?}")))?;
        let mut length = 0;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        Ok((status, length, reader))
    }

    /// A request whose answer is a JSON message, or nothing.
    fn call(
        &self,
        method: &str,
        path: &str,
        body: &Body<'_>,
    ) -> std::io::Result<(u16, serde_json::Value)> {
        let (status, length, reader) = self.send(method, path, body)?;
        let mut bytes = Vec::new();
        reader.take(length).read_to_end(&mut bytes)?;
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        Ok((status, value))
    }

    /// Fetches `path` into the file `to`.
    fn download(&self, path: &str, to: &Path) -> std::io::Result<()> {
        let (status, length, reader) = self.send("GET", path, &Body::Empty)?;
        if status != 200 {
            return Err(std::io::Error::other(format!("{path}: status {status}")));
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::File::create(to)?;
        let copied = std::io::copy(&mut reader.take(length), &mut file)?;
        if copied != length {
            return Err(std::io::Error::other(format!(
                "{path}: {copied} of {length} bytes arrived"
            )));
        }
        Ok(())
    }
}

impl Client {
    /// Fetches bytes `range` of `path` into the same place of the file
    /// `to`, which already has its full size.
    fn download_range(
        &self,
        path: &str,
        to: &Path,
        range: &std::ops::Range<u64>,
    ) -> std::io::Result<()> {
        use std::io::Seek;
        let (status, length, reader) = self.send_range("GET", path, &Body::Empty, Some(range))?;
        if status != 206 || length != range.end - range.start {
            return Err(std::io::Error::other(format!(
                "{path}: status {status} for bytes {}..{}",
                range.start, range.end
            )));
        }
        let mut file = std::fs::OpenOptions::new().write(true).open(to)?;
        file.seek(std::io::SeekFrom::Start(range.start))?;
        let copied = std::io::copy(&mut reader.take(length), &mut file)?;
        if copied != length {
            return Err(std::io::Error::other(format!(
                "{path}: {copied} of {length} bytes arrived"
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// The coordinator.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Running,
    Done,
}

struct PartState {
    part: Part,
    /// The bytes of each file fetched in ranges that this part needs
    /// beyond each file's base, by file index.
    ranges: Vec<(usize, Vec<std::ops::Range<u64>>)>,
    file: PathBuf,
    status: Status,
    /// When it was last handed out.
    started: Option<Instant>,
    /// The workers rendering it now.
    holders: Vec<u64>,
    attempts: u32,
    /// How long the copy that arrived took.
    seconds: Option<f64>,
    /// The worker whose copy arrived.
    by: Option<u64>,
}

struct Worker {
    name: String,
}

#[derive(Default)]
struct State {
    parts: Vec<PartState>,
    workers: BTreeMap<u64, Worker>,
    next_worker: u64,
    /// Why the render cannot finish.
    failed: Option<String>,
    /// Workers that could not render this job, and why.
    refusals: Vec<String>,
    /// Set once the parts are joined, so that late workers stop.
    finished: bool,
}

struct Farm {
    token: String,
    fingerprint: String,
    /// The job as `/hello` answers it, less the worker's id.
    job: serde_json::Value,
    /// The files under the root, in `/file/<n>` order.
    files: Vec<PathBuf>,
    part_timeout: Duration,
    state: Mutex<State>,
    changed: Condvar,
    /// Bytes of files sent to workers.
    sent: std::sync::atomic::AtomicU64,
}

impl Farm {
    fn serve(self: &Arc<Self>, stream: TcpStream) {
        let Ok(mut req) = Request::read(stream) else {
            return;
        };
        let open = matches!(req.path.as_str(), "/status" | "/metrics");
        if !open && req.token.as_deref() != Some(self.token.as_str()) {
            req.respond(401, "text/plain", b"a wrong or missing token");
            return;
        }
        let segments: Vec<String> = req
            .path
            .trim_start_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect();
        let index = segments.get(1).and_then(|s| s.parse::<usize>().ok());
        match (req.method.as_str(), segments[0].as_str()) {
            ("POST", "hello") => self.hello(&mut req),
            ("GET", "file") => match index.and_then(|i| self.files.get(i)) {
                Some(path) => {
                    let n = req.respond_file(path);
                    self.sent.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                }
                None => req.respond(404, "text/plain", b"no such file"),
            },
            ("POST", "claim") => self.claim(&mut req),
            ("PUT", "part") => self.receive(&mut req, index),
            ("POST", "failed") => self.failed(&mut req, index),
            ("POST", "refuse") => {
                let msg = req.json();
                let mut state = self.state.lock().expect("state");
                let id = msg["worker"].as_u64().unwrap_or(0);
                let name = state
                    .workers
                    .remove(&id)
                    .map_or_else(|| format!("worker {id}"), |w| w.name);
                state.refusals.push(format!(
                    "{name}: {}",
                    msg["reason"].as_str().unwrap_or("no reason given")
                ));
                drop(state);
                self.changed.notify_all();
                req.respond_json(200, &serde_json::json!({ "ok": true }));
            }
            ("GET", "status") => {
                let body = self.status();
                req.respond_json(200, &body);
            }
            ("GET", "metrics") => {
                let s = self.status();
                let text = format!(
                    "# HELP geneva_farm_parts Parts of the render by state.\n# TYPE geneva_farm_parts gauge\ngeneva_farm_parts{{state=\"pending\"}} {}\ngeneva_farm_parts{{state=\"running\"}} {}\ngeneva_farm_parts{{state=\"done\"}} {}\n# HELP geneva_farm_workers Workers that have said hello.\n# TYPE geneva_farm_workers gauge\ngeneva_farm_workers {}\n",
                    s["pending"], s["running"], s["done"], s["workers"]
                );
                req.respond(200, "text/plain; version=0.0.4", text.as_bytes());
            }
            _ => req.respond(404, "text/plain", b"no such endpoint"),
        }
    }

    fn status(&self) -> serde_json::Value {
        let state = self.state.lock().expect("state");
        let count = |s: Status| state.parts.iter().filter(|p| p.status == s).count();
        serde_json::json!({
            "parts": state.parts.len(),
            "pending": count(Status::Pending),
            "running": count(Status::Running),
            "done": count(Status::Done),
            "workers": state.workers.len(),
            "finished": state.finished,
            "fingerprint": self.fingerprint,
        })
    }

    fn hello(&self, req: &mut Request) {
        let msg = req.json();
        let theirs = msg["fingerprint"].as_str().unwrap_or("unknown");
        if theirs != self.fingerprint {
            req.respond_json(
                409,
                &serde_json::json!({
                    "reason": format!(
                        "this farm renders with {}, the worker with {theirs}; parts from the two would not join",
                        self.fingerprint
                    )
                }),
            );
            return;
        }
        let mut state = self.state.lock().expect("state");
        if state.finished || state.failed.is_some() {
            drop(state);
            req.respond(204, "text/plain", b"");
            return;
        }
        state.next_worker += 1;
        let id = state.next_worker;
        let name = msg["name"].as_str().unwrap_or("worker").to_owned();
        state.workers.insert(
            id,
            Worker {
                name: format!("{name} ({id})"),
            },
        );
        drop(state);
        let mut job = self.job.clone();
        job["worker"] = id.into();
        req.respond_json(200, &job);
    }

    fn claim(&self, req: &mut Request) {
        let msg = req.json();
        let id = msg["worker"].as_u64().unwrap_or(0);
        let mut state = self.state.lock().expect("state");
        if state.finished
            || state.failed.is_some()
            || state.parts.iter().all(|p| p.status == Status::Done)
        {
            drop(state);
            req.respond(204, "text/plain", b"");
            return;
        }
        let now = Instant::now();
        let mut finished: Vec<f64> = state.parts.iter().filter_map(|p| p.seconds).collect();
        finished.sort_by(f64::total_cmp);
        let median = finished.get(finished.len() / 2).copied();
        let timeout = self.part_timeout;
        // A part nobody has, else one whose worker went quiet, else a
        // straggler another worker could race.
        let pick = state
            .parts
            .iter()
            .position(|p| p.status == Status::Pending)
            .or_else(|| {
                state.parts.iter().position(|p| {
                    p.status == Status::Running
                        && !p.holders.contains(&id)
                        && p.started.is_some_and(|s| now - s > timeout)
                })
            })
            .or_else(|| {
                let median = median?;
                state.parts.iter().position(|p| {
                    p.status == Status::Running
                        && p.holders.len() < 2
                        && !p.holders.contains(&id)
                        && p.started.is_some_and(|s| {
                            (now - s).as_secs_f64() > STRAGGLER_FACTOR * median.max(1.0)
                        })
                })
            });
        let Some(i) = pick else {
            drop(state);
            req.respond_json(202, &serde_json::json!({ "wait_ms": WAIT_MS }));
            return;
        };
        let p = &mut state.parts[i];
        if p.started.is_some_and(|s| now - s > timeout) {
            // The worker holding it is presumed gone.
            p.holders.clear();
        }
        p.status = Status::Running;
        p.started = Some(now);
        p.holders.push(id);
        let mut body = match &p.part {
            Part::Frames(r) => serde_json::json!({ "part": i, "frames": [r.start, r.end] }),
            Part::Audio => serde_json::json!({ "part": i, "audio": true }),
        };
        body["ranges"] = p
            .ranges
            .iter()
            .map(|(file, ranges)| {
                serde_json::json!({
                    "file": file,
                    "ranges": ranges.iter().map(|r| [r.start, r.end]).collect::<Vec<_>>(),
                })
            })
            .collect();
        drop(state);
        req.respond_json(200, &body);
    }

    fn receive(&self, req: &mut Request, index: Option<usize>) {
        let id = req
            .query
            .get("worker")
            .and_then(|w| w.parse::<u64>().ok())
            .unwrap_or(0);
        let (file, started) = {
            let state = self.state.lock().expect("state");
            match index.and_then(|i| state.parts.get(i)) {
                Some(p) if p.status == Status::Done => {
                    drop(state);
                    req.respond_json(200, &serde_json::json!({ "ok": true, "late": true }));
                    return;
                }
                Some(p) => (p.file.clone(), p.started),
                None => {
                    drop(state);
                    req.respond(404, "text/plain", b"no such part");
                    return;
                }
            }
        };
        let tmp = file.with_extension(format!("from-{id}"));
        let length = req.length;
        let written = std::fs::File::create(&tmp)
            .and_then(|mut f| std::io::copy(&mut (&mut req.body).take(length), &mut f));
        if !matches!(written, Ok(n) if n == length) {
            let _ = std::fs::remove_file(&tmp);
            req.respond(400, "text/plain", b"the part did not arrive whole");
            return;
        }
        let i = index.expect("checked above");
        let mut state = self.state.lock().expect("state");
        if state.parts[i].status == Status::Done {
            drop(state);
            let _ = std::fs::remove_file(&tmp);
            req.respond_json(200, &serde_json::json!({ "ok": true, "late": true }));
            return;
        }
        if std::fs::rename(&tmp, &file).is_err() {
            drop(state);
            req.respond(400, "text/plain", b"could not keep the part");
            return;
        }
        let p = &mut state.parts[i];
        p.status = Status::Done;
        p.holders.clear();
        p.seconds = started.map(|s| s.elapsed().as_secs_f64());
        p.by = Some(id);
        drop(state);
        self.changed.notify_all();
        req.respond_json(200, &serde_json::json!({ "ok": true }));
    }

    fn failed(&self, req: &mut Request, index: Option<usize>) {
        let msg = req.json();
        let id = msg["worker"].as_u64().unwrap_or(0);
        let reason = msg["reason"]
            .as_str()
            .unwrap_or("no reason given")
            .to_owned();
        let mut state = self.state.lock().expect("state");
        let name = state
            .workers
            .get(&id)
            .map_or_else(|| format!("worker {id}"), |w| w.name.clone());
        if let Some(p) = index.and_then(|i| state.parts.get_mut(i)) {
            if p.status != Status::Done {
                p.holders.retain(|h| *h != id);
                p.attempts += 1;
                if p.holders.is_empty() {
                    p.status = Status::Pending;
                    p.started = None;
                }
                if p.attempts >= MAX_ATTEMPTS {
                    state.failed = Some(format!(
                        "part {} failed {MAX_ATTEMPTS} times; the last, on {name}: {reason}",
                        index.unwrap_or(0)
                    ));
                }
            }
        }
        drop(state);
        self.changed.notify_all();
        req.respond_json(200, &serde_json::json!({ "ok": true }));
    }
}

/// A random token: 128 bits from the system where it has a source, else
/// from the process's hash seeds and the clock.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    let from_system = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !from_system {
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes.iter().fold(String::new(), |mut hex, b| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// The address other machines reach this one at: the listening address,
/// or when that is every interface, the one the route out leaves from.
/// Connecting a UDP socket sends nothing.
fn reachable_address(bound: std::net::SocketAddr) -> String {
    if !bound.ip().is_unspecified() {
        return bound.to_string();
    }
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|()| s))
        .and_then(|s| s.local_addr())
        .map_or_else(|_| "127.0.0.1".to_owned(), |a| a.ip().to_string());
    format!("{ip}:{}", bound.port())
}

/// Renders `comp` into `output` on whatever workers connect, and joins
/// the parts. `progress` reports parts, not frames.
pub fn run(
    comp: &Composition,
    root: &Path,
    output: &Path,
    overrides: &RenderOverrides,
    opts: &FarmOptions,
    human: bool,
) -> Result<RenderStats, RenderError> {
    let started = Instant::now();
    let refuse = |reason: String| RenderError::Asset {
        id: output.display().to_string(),
        reason,
    };
    let has_sound = media::can_split(comp, output)
        .map_err(|reason| refuse(format!("cannot render on a farm: {reason}")))?;
    let has_sound = has_sound && !overrides.no_audio;
    let mut files = Vec::new();
    for f in &opts.files {
        if !safe_relative(f) {
            return Err(refuse(format!(
                "{f:?} is not a path under the asset root, so it cannot be sent to a worker"
            )));
        }
        let path = root.join(f);
        if path.is_file() {
            files.push((f.clone(), path));
        }
    }
    // Video and sound files whose index says where every packet is are
    // sent in ranges: each worker fetches the bytes its parts read.
    let maps: Vec<Option<geneva_media::ranges::PacketMap>> = files
        .iter()
        .map(|(f, p)| packet_map_of(comp, f, p))
        .collect();
    let assets_by_file: Vec<Vec<String>> = files
        .iter()
        .map(|(f, _)| {
            comp.assets
                .iter()
                .filter(|(_, a)| &a.src == f)
                .map(|(id, _)| id.clone())
                .collect()
        })
        .collect();
    let keyframe_interval = comp
        .encode
        .as_ref()
        .and_then(|e| e.video.as_ref())
        .and_then(|v| v.keyframe_interval);
    let ranges = geneva_media::chunks::split_frames(comp, keyframe_interval, opts.parts.max(1));
    let name = output
        .file_name()
        .map_or_else(|| "output".to_owned(), |n| n.to_string_lossy().into_owned());
    let ext = output
        .extension()
        .map_or_else(|| "mkv".to_owned(), |e| e.to_string_lossy().into_owned());
    let dir = output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join(format!(".{name}.farm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| refuse(format!("{}: {e}", dir.display())))?;
    let io = |e: std::io::Error| refuse(e.to_string());
    let mut parts: Vec<PartState> = Vec::new();
    let new_part = |part: Part, file: PathBuf| PartState {
        ranges: part_ranges(comp, &part, &assets_by_file, &maps),
        part,
        file,
        status: Status::Pending,
        started: None,
        holders: Vec::new(),
        attempts: 0,
        seconds: None,
        by: None,
    };
    // The sound first: it is quick, and done early it is off the path.
    if has_sound {
        parts.push(new_part(Part::Audio, dir.join(format!("audio.{ext}"))));
    }
    for (i, r) in ranges.iter().enumerate() {
        parts.push(new_part(
            Part::Frames(r.clone()),
            dir.join(format!("part-{i:03}.{ext}")),
        ));
    }
    let token = opts.token.clone().unwrap_or_else(random_token);
    let job = serde_json::json!({
        "timeline": opts.timeline,
        "files": files.iter().zip(&maps).map(|((f, p), map)| {
            let mut entry = serde_json::json!({
                "path": f,
                "size": std::fs::metadata(p).map_or(0, |m| m.len()),
            });
            if let Some(map) = map {
                entry["ranges"] = map.base().iter().map(|r| [r.start, r.end]).collect();
            }
            entry
        }).collect::<Vec<_>>(),
        "ext": ext,
        "crf": overrides.crf,
        "preset": overrides.preset,
        "diagnostics": opts.diagnostics,
    });
    let farm = Arc::new(Farm {
        token: token.clone(),
        fingerprint: fingerprint(),
        job,
        files: files.iter().map(|(_, p)| p.clone()).collect(),
        part_timeout: opts.part_timeout,
        state: Mutex::new(State {
            parts,
            ..State::default()
        }),
        changed: Condvar::new(),
        sent: std::sync::atomic::AtomicU64::new(0),
    });
    let whole: u64 = files
        .iter()
        .map(|(_, p)| std::fs::metadata(p).map_or(0, |m| m.len()))
        .sum();
    let listener = TcpListener::bind(&opts.listen)
        .map_err(|e| refuse(format!("cannot listen on {}: {e}", opts.listen)))?;
    let bound = listener.local_addr().map_err(io)?;
    let url = format!("http://{}", reachable_address(bound));
    let local_url = format!("http://127.0.0.1:{}", bound.port());
    {
        let farm = Arc::clone(&farm);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let farm = Arc::clone(&farm);
                std::thread::spawn(move || farm.serve(stream));
            }
        });
    }
    let total_parts = farm.state.lock().expect("state").parts.len();
    eprintln!(
        "farm: {total_parts} parts at {url}; start workers with\n  geneva worker --connect {url} --token {token}"
    );
    // Workers this farm starts: on this machine, and through the launch
    // command.
    let exe = std::env::current_exe().map_err(io)?;
    let mut children: Vec<std::process::Child> = Vec::new();
    for _ in 0..opts.local {
        let child = std::process::Command::new(&exe)
            .args([
                "--format",
                "json",
                "worker",
                "--connect",
                &local_url,
                "--name",
                "local",
            ])
            .env("GENEVA_FARM_TOKEN", &token)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(io)?;
        children.push(child);
    }
    if let Some(cmd) = &opts.launch {
        let count = opts.launch_count.unwrap_or(total_parts as u32);
        for n in 0..count {
            let mut shell = if cfg!(windows) {
                let mut c = std::process::Command::new("cmd");
                c.arg("/C").arg(cmd);
                c
            } else {
                let mut c = std::process::Command::new("sh");
                c.arg("-c").arg(cmd);
                c
            };
            let child = shell
                .env("GENEVA_FARM_URL", &url)
                .env("GENEVA_FARM_TOKEN", &token)
                .env("GENEVA_FARM_WORKER", n.to_string())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .map_err(|e| refuse(format!("cannot run the launch command: {e}")))?;
            children.push(child);
        }
    }
    let started_children = !children.is_empty();
    // Wait for the parts, reporting as they arrive.
    let mut last_line: Option<Instant> = None;
    loop {
        let state = farm.state.lock().expect("state");
        let (state, _) = farm
            .changed
            .wait_timeout(state, Duration::from_millis(500))
            .expect("state");
        if let Some(reason) = &state.failed {
            let reason = reason.clone();
            drop(state);
            stop(&farm, &mut children);
            let _ = std::fs::remove_dir_all(&dir);
            return Err(refuse(reason));
        }
        let done = state
            .parts
            .iter()
            .filter(|p| p.status == Status::Done)
            .count();
        let running = state
            .parts
            .iter()
            .filter(|p| p.status == Status::Running)
            .count();
        let workers = state.workers.len();
        if done == state.parts.len() {
            break;
        }
        // Every worker this farm started has gone and nobody else is
        // rendering: nothing will arrive.
        let all_gone = started_children
            && running == 0
            && children
                .iter_mut()
                .all(|c| matches!(c.try_wait(), Ok(Some(_))));
        if all_gone {
            let why = if state.refusals.is_empty() {
                "the workers this farm started stopped before the parts were done".to_owned()
            } else {
                format!("no worker could render this: {}", state.refusals.join("; "))
            };
            drop(state);
            stop(&farm, &mut children);
            let _ = std::fs::remove_dir_all(&dir);
            return Err(refuse(why));
        }
        drop(state);
        let pace = if human {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        };
        if last_line.is_none_or(|l| l.elapsed() >= pace) {
            last_line = Some(Instant::now());
            if human {
                eprint!(
                    "\rparts {done}/{total_parts} done, {running} rendering, {workers} workers"
                );
            } else {
                eprintln!(
                    "{}",
                    serde_json::json!({ "parts": total_parts, "done": done, "running": running, "workers": workers })
                );
            }
        }
    }
    if human {
        eprintln!();
    }
    let render_secs = started.elapsed().as_secs_f64();
    let (picture, sound, summary) = {
        let state = farm.state.lock().expect("state");
        let picture: Vec<PathBuf> = state
            .parts
            .iter()
            .filter(|p| matches!(p.part, Part::Frames(_)))
            .map(|p| p.file.clone())
            .collect();
        let sound = state
            .parts
            .iter()
            .find(|p| matches!(p.part, Part::Audio))
            .map(|p| p.file.clone());
        let mut by_worker: BTreeMap<String, u32> = BTreeMap::new();
        for p in &state.parts {
            let name =
                p.by.and_then(|id| state.workers.get(&id))
                    .map_or_else(|| "a worker".to_owned(), |w| w.name.clone());
            *by_worker.entry(name).or_default() += 1;
        }
        let summary = by_worker
            .iter()
            .map(|(name, n)| format!("{name} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        (picture, sound, summary)
    };
    let joined = media::join_parts(comp, root, output, overrides, &picture, sound.as_deref());
    stop(&farm, &mut children);
    let _ = std::fs::remove_dir_all(&dir);
    let mut stats = joined?;
    let sent = farm.sent.load(std::sync::atomic::Ordering::Relaxed);
    let workers = farm.state.lock().expect("state").workers.len().max(1) as u64;
    stats.notes = vec![
        format!(
            "rendered in {total_parts} parts in {render_secs:.1}s (parts by worker: {summary}), joined without re-encoding"
        ),
        format!(
            "workers fetched {} of files; each whole copy is {}, so {workers} whole copies would have been {}",
            human_bytes(sent),
            human_bytes(whole),
            human_bytes(whole * workers)
        ),
    ];
    stats.frames = comp.frame_count();
    stats.mode = RenderMode::Render;
    stats.seconds = started.elapsed().as_secs_f64();
    Ok(stats)
}

fn human_bytes(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / f64::from(1u32 << 30))
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / f64::from(1u32 << 20))
    } else {
        format!("{:.0} kB", n as f64 / 1024.0)
    }
}

/// Where every packet of the file `f` is, when it is a video or sound
/// asset in a container that can be fetched in ranges.
fn packet_map_of(
    comp: &Composition,
    f: &str,
    path: &Path,
) -> Option<geneva_media::ranges::PacketMap> {
    use geneva_timeline::schema::AssetKind;
    let media = comp
        .assets
        .values()
        .any(|a| a.src == f && matches!(a.kind, AssetKind::Video | AssetKind::Audio));
    if !media {
        return None;
    }
    #[cfg(feature = "media")]
    return geneva_media::packet_map(path).ok().flatten();
    #[cfg(not(feature = "media"))]
    {
        let _ = path;
        None
    }
}

/// The seconds of each video asset that output frames `range` show, by
/// asset id: `None` for an asset used where the mapping is not a plain
/// offset (inside a nested composition, or as a mask), which a part
/// then fetches whole.
fn source_windows(
    comp: &Composition,
    range: &std::ops::Range<u64>,
) -> BTreeMap<String, Option<(f64, f64)>> {
    use geneva_timeline::ResolvedSource;
    fn videos_in(layers: &[geneva_timeline::ResolvedLayer], ids: &mut Vec<String>) {
        for clip in layers.iter().flat_map(|l| &l.clips) {
            match &clip.source {
                ResolvedSource::Video { asset, .. } => ids.push(asset.clone()),
                ResolvedSource::Composition(c) => videos_in(&c.layers, ids),
                _ => {}
            }
            if let Some(asset) = clip.mask.as_ref().and_then(|m| m.asset.as_ref()) {
                ids.push(asset.clone());
            }
        }
    }
    let t0 = comp.frame_time(range.start);
    let t1 = comp.frame_time(range.end);
    let mut out: BTreeMap<String, Option<(f64, f64)>> = BTreeMap::new();
    let mut widen = |id: &str, w: Option<(f64, f64)>| {
        let entry = out.entry(id.to_owned()).or_insert(w);
        *entry = match (*entry, w) {
            (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
            _ => None,
        };
    };
    for clip in comp.layers.iter().flat_map(|l| &l.clips) {
        if let Some(asset) = clip.mask.as_ref().and_then(|m| m.asset.as_ref()) {
            widen(asset, None);
        }
        let from = if clip.start > t0 { clip.start } else { t0 };
        let to = if clip.end < t1 { clip.end } else { t1 };
        if from >= to {
            continue;
        }
        match &clip.source {
            ResolvedSource::Video { asset, in_, .. } => {
                let a = (*in_ + (from - clip.start) * clip.speed).to_f64();
                let b = (*in_ + (to - clip.start) * clip.speed).to_f64();
                widen(asset, Some((a.min(b), a.max(b))));
            }
            ResolvedSource::Composition(c) => {
                let mut ids = Vec::new();
                videos_in(&c.layers, &mut ids);
                for id in ids {
                    widen(&id, None);
                }
            }
            _ => {}
        }
    }
    out
}

/// The ranges of each file a part fetches beyond the file's base.
fn part_ranges(
    comp: &Composition,
    part: &Part,
    assets_by_file: &[Vec<String>],
    maps: &[Option<geneva_media::ranges::PacketMap>],
) -> Vec<(usize, Vec<std::ops::Range<u64>>)> {
    // A little either side of the stretch, for timestamps that round.
    const MARGIN: f64 = 0.1;
    let windows = match part {
        Part::Frames(r) => Some(source_windows(comp, r)),
        Part::Audio => None,
    };
    let mut out = Vec::new();
    for (i, map) in maps.iter().enumerate() {
        let Some(map) = map else { continue };
        let ranges = match &windows {
            None => map.sound(),
            Some(w) => {
                let mut ranges = Vec::new();
                for id in &assets_by_file[i] {
                    match w.get(id) {
                        Some(Some((a, b))) => ranges.extend(map.picture(a - MARGIN, b + MARGIN)),
                        Some(None) => ranges.push(0..map.size),
                        None => {}
                    }
                }
                geneva_media::ranges::merge(ranges)
            }
        };
        if !ranges.is_empty() {
            out.push((i, ranges));
        }
    }
    out
}

/// Tells late workers there is nothing more, and gives this farm's own
/// workers a moment to leave before stopping them.
fn stop(farm: &Farm, children: &mut [std::process::Child]) {
    farm.state.lock().expect("state").finished = true;
    let deadline = Instant::now() + Duration::from_secs(5);
    for child in children.iter_mut() {
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------
// The worker.

/// What a worker did, for its report.
pub struct WorkerReport {
    /// Parts rendered and sent: index, what, seconds.
    pub parts: Vec<(usize, String, f64)>,
    /// Why it stopped early, if it did.
    pub refused: Option<String>,
}

/// How long a worker keeps trying to reach a coordinator that is not
/// answering yet, as one started a moment before it may not be.
const CONNECT_PATIENCE: Duration = Duration::from_secs(30);

/// Connects to the farm at `url`, fetches the job, renders parts until
/// there are none left, and cleans up after itself. `load` loads the
/// timeline as `render` would; `quiet` keeps the frame progress off.
pub fn work(
    url: &str,
    token: &str,
    name: &str,
    dir: Option<&Path>,
    load: &dyn Fn(&str, &Path) -> geneva_timeline::Loaded,
    progress_format: media::ProgressFormat,
    human: bool,
) -> Result<WorkerReport, String> {
    let client = Client::new(url, token)?;
    let hello = serde_json::json!({ "fingerprint": fingerprint(), "name": name });
    let first_try = Instant::now();
    let (status, job) = loop {
        match client.call("POST", "/hello", &Body::Json(&hello)) {
            Ok(answer) => break answer,
            Err(e) if first_try.elapsed() < CONNECT_PATIENCE => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(format!("cannot reach the farm at {url}: {e}")),
        }
    };
    match status {
        200 => {}
        204 => {
            return Ok(WorkerReport {
                parts: Vec::new(),
                refused: None,
            });
        }
        401 => return Err("the farm refused the token".to_owned()),
        409 => {
            return Err(job["reason"]
                .as_str()
                .unwrap_or("the farm refused this worker")
                .to_owned());
        }
        s => return Err(format!("the farm answered {s}")),
    }
    let id = job["worker"].as_u64().unwrap_or(0);
    let own_dir = dir.is_none();
    let dir = dir.map_or_else(
        || std::env::temp_dir().join(format!("geneva-worker-{}-{id}", std::process::id())),
        Path::to_path_buf,
    );
    let result = work_in(&client, &job, id, &dir, load, progress_format, human);
    if own_dir {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn work_in(
    client: &Client,
    job: &serde_json::Value,
    id: u64,
    dir: &Path,
    load: &dyn Fn(&str, &Path) -> geneva_timeline::Loaded,
    progress_format: media::ProgressFormat,
    human: bool,
) -> Result<WorkerReport, String> {
    let root = dir.join("root");
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    // The ranges of each ranged file already here, by file index.
    let mut fetched: BTreeMap<usize, Vec<std::ops::Range<u64>>> = BTreeMap::new();
    let paths: Vec<PathBuf> = job["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| root.join(f["path"].as_str().unwrap_or_default()))
        .collect();
    // The files the timeline reads, at the same paths under this root.
    for (n, f) in job["files"].as_array().into_iter().flatten().enumerate() {
        let path = f["path"].as_str().unwrap_or_default();
        if !safe_relative(path) {
            return Err(format!("the farm sent a path outside the root: {path:?}"));
        }
        let to = root.join(path);
        let size = f["size"].as_u64().unwrap_or(u64::MAX);
        if let Some(base) = f["ranges"].as_array() {
            // Fetched in ranges: a file of the full size with holes, then
            // the headers and the index, and each part's bytes later.
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let file = std::fs::File::create(&to).map_err(|e| format!("{}: {e}", to.display()))?;
            file.set_len(size)
                .map_err(|e| format!("{}: {e}", to.display()))?;
            let base = json_ranges(base);
            fetch_ranges(client, n, &to, &base, &mut fetched)?;
            continue;
        }
        if std::fs::metadata(&to).is_ok_and(|m| m.len() == size) {
            continue;
        }
        client
            .download(&format!("/file/{n}"), &to)
            .map_err(|e| format!("fetching {path}: {e}"))?;
    }
    let text = job["timeline"].as_str().unwrap_or_default();
    let loaded = load(text, &root);
    // Anything this machine's load finds that the farm's did not (a font
    // it lacks, a file it cannot decode) would make its parts differ.
    let theirs: Vec<&str> = job["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let extra: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity >= Severity::Warning)
        .filter(|d| !theirs.contains(&format!("{} {}", d.code, d.path).as_str()))
        .map(|d| format!("{}[{}]: {}", severity_word(d.severity), d.code, d.message))
        .collect();
    let (Some(comp), true) = (&loaded.composition, extra.is_empty()) else {
        let reason = if extra.is_empty() {
            "the timeline does not load here".to_owned()
        } else {
            format!(
                "this machine reads the timeline differently: {}",
                extra.join("; ")
            )
        };
        let _ = client.call(
            "POST",
            "/refuse",
            &Body::Json(&serde_json::json!({ "worker": id, "reason": reason })),
        );
        return Ok(WorkerReport {
            parts: Vec::new(),
            refused: Some(reason),
        });
    };
    let overrides = RenderOverrides {
        renderer: media::RendererChoice::Cpu,
        crf: job["crf"].as_u64().map(|c| c as u8),
        preset: job["preset"].as_str().map(str::to_owned),
        no_audio: false,
        exact: false,
        picture_as_is: false,
    };
    let ext = job["ext"].as_str().unwrap_or("mkv");
    let mut done = Vec::new();
    let mut quiet_since: Option<Instant> = None;
    loop {
        let answer = client.call(
            "POST",
            "/claim",
            &Body::Json(&serde_json::json!({ "worker": id })),
        );
        let (status, claim) = match answer {
            Ok(a) => {
                quiet_since = None;
                a
            }
            // The farm stopped answering: it finished, or it is gone.
            Err(_) => {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(5) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        match status {
            200 => {}
            202 => {
                let ms = claim["wait_ms"].as_u64().unwrap_or(WAIT_MS);
                std::thread::sleep(Duration::from_millis(ms));
                continue;
            }
            _ => break,
        }
        let index = claim["part"].as_u64().unwrap_or(0) as usize;
        let (part, what) = if claim["audio"].as_bool() == Some(true) {
            (Part::Audio, "sound".to_owned())
        } else {
            let a = claim["frames"][0].as_u64().unwrap_or(0);
            let b = claim["frames"][1].as_u64().unwrap_or(0);
            (Part::Frames(a..b), format!("frames {a}..{b}"))
        };
        let file = dir.join(format!("part-{index:03}.{ext}"));
        let started = Instant::now();
        let mut fetch_failed = None;
        for entry in claim["ranges"].as_array().into_iter().flatten() {
            let n = entry["file"].as_u64().unwrap_or(u64::MAX) as usize;
            let Some(to) = paths.get(n) else { continue };
            let want = json_ranges(entry["ranges"].as_array().map_or(&[][..], Vec::as_slice));
            if let Err(e) = fetch_ranges(client, n, to, &want, &mut fetched) {
                fetch_failed = Some(e);
                break;
            }
        }
        if let Some(reason) = fetch_failed {
            let _ = client.call(
                "POST",
                &format!("/failed/{index}"),
                &Body::Json(&serde_json::json!({ "worker": id, "reason": reason })),
            );
            continue;
        }
        let progress = media::Progress::new(progress_format);
        match media::render_part(comp, &root, &file, &overrides, &part, &progress) {
            Ok(_) => {
                let sent = client.call(
                    "PUT",
                    &format!("/part/{index}?worker={id}"),
                    &Body::File(&file),
                );
                let _ = std::fs::remove_file(&file);
                let secs = started.elapsed().as_secs_f64();
                match sent {
                    Ok((200, _)) => {
                        if human {
                            eprintln!("\rpart {index} ({what}) rendered and sent in {secs:.1}s");
                        }
                        done.push((index, what, secs));
                    }
                    Ok((s, _)) => {
                        if human {
                            eprintln!("\rpart {index} ({what}): the farm answered {s}");
                        }
                    }
                    Err(_) => break,
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                if human {
                    eprintln!("\rpart {index} ({what}) failed: {e}");
                }
                let _ = client.call(
                    "POST",
                    &format!("/failed/{index}"),
                    &Body::Json(&serde_json::json!({ "worker": id, "reason": e.to_string() })),
                );
            }
        }
    }
    Ok(WorkerReport {
        parts: done,
        refused: None,
    })
}

/// `[[a, b], ...]` as byte ranges.
fn json_ranges(values: &[serde_json::Value]) -> Vec<std::ops::Range<u64>> {
    values
        .iter()
        .filter_map(|r| Some(r[0].as_u64()?..r[1].as_u64()?))
        .collect()
}

/// Fetches the parts of `want` of file `n` not yet in `fetched`.
fn fetch_ranges(
    client: &Client,
    n: usize,
    to: &Path,
    want: &[std::ops::Range<u64>],
    fetched: &mut BTreeMap<usize, Vec<std::ops::Range<u64>>>,
) -> Result<(), String> {
    use geneva_media::ranges::{merge, missing, union};
    let have = fetched.entry(n).or_default();
    for r in missing(&merge(want.to_vec()), have) {
        client
            .download_range(&format!("/file/{n}"), to, &r)
            .map_err(|e| format!("fetching {}: {e}", to.display()))?;
        have.push(r);
    }
    *have = union(std::mem::take(have));
    Ok(())
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_relative_paths_under_the_root_are_sent() {
        assert!(safe_relative("clip.mp4"));
        assert!(safe_relative("media/clip.mp4"));
        assert!(safe_relative("./card.html"));
        assert!(!safe_relative("../secret"));
        assert!(!safe_relative("media/../../secret"));
        assert!(!safe_relative("/etc/passwd"));
        assert!(!safe_relative(""));
    }

    #[test]
    fn a_token_is_32_hex_digits_and_differs_each_time() {
        let a = random_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_token());
    }

    #[test]
    fn a_client_takes_only_http_host_port() {
        assert!(Client::new("http://10.0.0.5:7700", "t").is_ok());
        assert!(Client::new("http://10.0.0.5:7700/", "t").is_ok());
        assert!(Client::new("https://10.0.0.5:7700", "t").is_err());
        assert!(Client::new("http://10.0.0.5:7700/x", "t").is_err());
    }
}
