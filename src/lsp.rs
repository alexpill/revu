//! Minimal language-server client: JSON-RPC over stdio, enough for
//! definition, references and hover on a read-only checkout.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// Request ids are unique across servers so responses can share one channel.
static NEXT_ID: AtomicI64 = AtomicI64::new(1);

/// Default server and LSP language id per file extension.
fn default_server(ext: &str) -> Option<(&'static [&'static str], &'static str)> {
    Some(match ext {
        "rs" => (&["rust-analyzer"], "rust"),
        "ts" | "mts" | "cts" => (&["typescript-language-server", "--stdio"], "typescript"),
        "tsx" => (&["typescript-language-server", "--stdio"], "typescriptreact"),
        "js" | "mjs" | "cjs" => (&["typescript-language-server", "--stdio"], "javascript"),
        "jsx" => (&["typescript-language-server", "--stdio"], "javascriptreact"),
        "py" => (&["pyright-langserver", "--stdio"], "python"),
        "go" => (&["gopls"], "go"),
        "nix" => (&["nil"], "nix"),
        "lua" => (&["lua-language-server"], "lua"),
        "c" | "h" => (&["clangd"], "c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => (&["clangd"], "cpp"),
        "zig" => (&["zls"], "zig"),
        "rb" => (&["ruby-lsp"], "ruby"),
        _ => return None,
    })
}

/// (server command, language id) for `path`, honoring `[lsp]` overrides.
pub fn server_for(path: &str, overrides: &HashMap<String, Vec<String>>) -> Option<(Vec<String>, String)> {
    let ext = path.rsplit_once('.')?.1;
    let default = default_server(ext);
    let lang = default.map_or(ext, |(_, l)| l).to_string();
    match overrides.get(ext) {
        Some(cmd) if !cmd.is_empty() => Some((cmd.clone(), lang)),
        _ => default.map(|(cmd, _)| (cmd.iter().map(|s| s.to_string()).collect(), lang)),
    }
}

struct Shared {
    stdin: ChildStdin,
    /// Until the initialize response arrives, outgoing messages wait here.
    ready: bool,
    queue: Vec<Value>,
}

fn write(stdin: &mut ChildStdin, msg: &Value) -> std::io::Result<()> {
    let body = msg.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    stdin.flush()
}

/// What a server reports, delivered on its reader thread.
#[derive(Debug)]
pub enum Event {
    Response(i64, Result<Value, String>),
    /// Work-done progress ("Indexing 40%") or a server error/warning message.
    Status(String),
    /// All progress has ended: answers should be complete from now on.
    Idle,
    /// The process is gone; carries the end of its stderr.
    Exited(String),
}

pub struct Server {
    /// Unique across servers, passed along with each event.
    pub id: i64,
    child: Child,
    shared: Arc<Mutex<Shared>>,
    opened: HashSet<PathBuf>,
}

impl Server {
    /// Spawns the server; `on_event(server_id, event)` runs on a reader thread.
    pub fn start(cmd: &[String], root: &Path, on_event: impl Fn(i64, Event) + Send + 'static) -> Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let on_event = move |e| on_event(id, e);
        let mut child = Command::new(&cmd[0])
            .args(&cmd[1..])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start `{}` (not installed? see [lsp] in the config)", cmd[0]))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        // Drain stderr (a full pipe would block the server) and keep its tail
        // to explain an early exit, e.g. a rustup proxy without the component.
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let stderr_tail = std::thread::spawn(move || {
            let mut tail = std::collections::VecDeque::new();
            for line in stderr.lines().map_while(Result::ok) {
                if tail.len() == 5 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            Vec::from(tail).join(" ")
        });
        let shared = Arc::new(Mutex::new(Shared {
            stdin,
            ready: false,
            queue: Vec::new(),
        }));

        let init_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root_uri = uri(root);
        let init = json!({
            "jsonrpc": "2.0", "id": init_id, "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "pr" }],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-16"] },
                    // Without this servers never report $/progress (indexing).
                    "window": { "workDoneProgress": true },
                    // rust-analyzer: tells when loading is really over (`quiescent`).
                    "experimental": { "serverStatusNotification": true },
                    "workspace": { "configuration": true, "workspaceFolders": true },
                    "textDocument": {
                        "synchronization": {},
                        "definition": { "linkSupport": true },
                        "references": {},
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                    },
                },
            },
        });
        write(&mut shared.lock().unwrap().stdin, &init)?;

        let reader_shared = shared.clone();
        std::thread::spawn(move || {
            let mut out = BufReader::new(stdout);
            let mut progress = Progress::default();
            // REVU_LSP_LOG=<file> appends every server message, for debugging.
            let mut log = std::env::var_os("REVU_LSP_LOG")
                .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
            while let Some(msg) = read_message(&mut out) {
                if let Some(f) = &mut log {
                    let _ = writeln!(f, "{msg}");
                }
                let id = msg.get("id").and_then(Value::as_i64);
                if let Some(event) = status_of(&msg, &mut progress) {
                    on_event(event);
                }
                if let Some(method) = msg.get("method").and_then(Value::as_str) {
                    // Requests from the server must be answered or some servers stall.
                    if let Some(id) = msg.get("id") {
                        let result = match method {
                            "workspace/configuration" => {
                                let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                                Value::Array(vec![Value::Null; n])
                            }
                            _ => Value::Null,
                        };
                        let mut s = reader_shared.lock().unwrap();
                        let _ = write(&mut s.stdin, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    }
                    continue;
                }
                match id {
                    Some(id) if id == init_id => {
                        let mut s = reader_shared.lock().unwrap();
                        let _ = write(
                            &mut s.stdin,
                            &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
                        );
                        for m in std::mem::take(&mut s.queue) {
                            let _ = write(&mut s.stdin, &m);
                        }
                        s.ready = true;
                    }
                    Some(id) => {
                        let res = match msg.get("error") {
                            Some(e) => Err(e["message"].as_str().unwrap_or("language server error").to_string()),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        on_event(Event::Response(id, res));
                    }
                    None => {}
                }
            }
            on_event(Event::Exited(stderr_tail.join().unwrap_or_default()));
        });
        Ok(Self {
            id,
            child,
            shared,
            opened: HashSet::new(),
        })
    }

    fn send(&self, msg: Value) {
        let mut s = self.shared.lock().unwrap();
        if s.ready {
            let _ = write(&mut s.stdin, &msg);
        } else {
            s.queue.push(msg);
        }
    }

    /// Tells the server about a file once (servers need it before answering).
    pub fn open(&mut self, path: &Path, lang: &str, text: &str) {
        if self.opened.insert(path.to_path_buf()) {
            self.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
                "textDocument": { "uri": uri(path), "languageId": lang, "version": 1, "text": text } } }));
        }
    }

    /// Sends a position request (definition/references/hover); returns its id.
    pub fn request(&self, method: &str, path: &Path, line: u32, character: u32) -> i64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut params =
            json!({ "textDocument": { "uri": uri(path) }, "position": { "line": line, "character": character } });
        if method == "textDocument/references" {
            params["context"] = json!({ "includeDeclaration": false });
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Progress {
    /// Running `$/progress` tokens and their titles.
    titles: HashMap<String, String>,
    /// The server sends `experimental/serverStatus`, which then decides idleness:
    /// its progress goes quiet several times while loading.
    has_server_status: bool,
}

/// Status from `$/progress`, `experimental/serverStatus` and
/// `window/showMessage` (errors and warnings only).
fn status_of(msg: &Value, progress: &mut Progress) -> Option<Event> {
    let params = &msg["params"];
    let titles = &mut progress.titles;
    match msg.get("method")?.as_str()? {
        "experimental/serverStatus" => {
            progress.has_server_status = true;
            match params["message"].as_str() {
                Some(m) if params["health"] != "ok" => Some(Event::Status(m.to_string())),
                _ => (params["quiescent"] == true).then_some(Event::Idle),
            }
        }
        "$/progress" => {
            let token = params["token"].to_string();
            let v = &params["value"];
            let title = match v["kind"].as_str()? {
                "begin" => {
                    let t = v["title"].as_str().unwrap_or_default().to_string();
                    titles.insert(token, t.clone());
                    t
                }
                "report" => titles.get(&token).cloned().unwrap_or_default(),
                _ => {
                    titles.remove(&token);
                    return (titles.is_empty() && !progress.has_server_status).then_some(Event::Idle);
                }
            };
            let mut s = title;
            if let Some(m) = v["message"].as_str() {
                s += &format!(" {m}");
            }
            if let Some(p) = v["percentage"].as_u64() {
                s += &format!(" {p}%");
            }
            Some(Event::Status(s))
        }
        // 1 = error, 2 = warning
        "window/showMessage" if params["type"].as_u64().is_some_and(|t| t <= 2) => {
            params["message"].as_str().map(|m| Event::Status(m.to_string()))
        }
        _ => None,
    }
}

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut header = String::new();
        if r.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(v) = header.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

pub fn uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s += &format!("%{b:02X}");
        }
    }
    s
}

fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(raw[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// A target position: absolute path, 0-based line, UTF-16 column.
pub type Location = (PathBuf, u32, u32);

/// Location | Location[] | LocationLink[] | null
pub fn locations(v: &Value) -> Vec<Location> {
    let items = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Null => Vec::new(),
        one => vec![one],
    };
    items
        .into_iter()
        .filter_map(|l| {
            let (uri, range) = match l.get("targetUri") {
                Some(u) => (u, &l["targetSelectionRange"]),
                None => (l.get("uri")?, &l["range"]),
            };
            let start = &range["start"];
            Some((
                path_from_uri(uri.as_str()?)?,
                start["line"].as_u64()? as u32,
                start["character"].as_u64()? as u32,
            ))
        })
        .collect()
}

/// Hover contents (MarkupContent, MarkedString or an array of them) as text.
pub fn hover_text(v: &Value) -> Option<String> {
    fn one(c: &Value) -> Option<String> {
        match c {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => {
                let value = o.get("value")?.as_str()?;
                Some(match o.get("language").and_then(Value::as_str) {
                    Some(lang) => format!("```{lang}\n{value}\n```"),
                    None => value.to_string(),
                })
            }
            _ => None,
        }
    }
    let contents = v.get("contents")?;
    let text = match contents {
        Value::Array(a) => a.iter().filter_map(one).collect::<Vec<_>>().join("\n\n"),
        c => one(c)?,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// Display column (tabs as 4 spaces) to the UTF-16 offset LSP expects.
pub fn utf16_col(line: &str, display_col: usize) -> u32 {
    let mut display = 0;
    let mut utf16 = 0;
    for c in line.chars() {
        let w = if c == '\t' { 4 } else { 1 };
        if display + w > display_col {
            break;
        }
        display += w;
        utf16 += c.len_utf16() as u32;
    }
    utf16
}

/// Inverse of `utf16_col`.
pub fn display_col(line: &str, utf16_col: u32) -> usize {
    let mut display = 0;
    let mut utf16 = 0;
    for c in line.chars() {
        if utf16 >= utf16_col {
            break;
        }
        utf16 += c.len_utf16() as u32;
        display += if c == '\t' { 4 } else { 1 };
    }
    display
}

pub fn read_line(path: &Path, line: u32) -> Result<String> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .nth(line as usize)
        .map(String::from)
        .ok_or_else(|| anyhow!("line {line} out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns() {
        assert_eq!(utf16_col("\tfoo(é😀x)", 4), 1);
        assert_eq!(utf16_col("\tfoo(é😀x)", 9), 6);
        assert_eq!(display_col("\tfoo(é😀x)", 6), 9);
        assert_eq!(utf16_col("ab😀x", 3), 4);
    }

    #[test]
    fn uris_and_locations() {
        let p = Path::new("/tmp/a b/c#.rs");
        assert_eq!(uri(p), "file:///tmp/a%20b/c%23.rs");
        assert_eq!(path_from_uri(&uri(p)).unwrap(), p);
        let v = json!([{ "targetUri": "file:///x.rs", "targetSelectionRange": { "start": { "line": 3, "character": 7 } } }]);
        assert_eq!(locations(&v), vec![(PathBuf::from("/x.rs"), 3, 7)]);
        let v = json!({ "uri": "file:///y.rs", "range": { "start": { "line": 1, "character": 0 } } });
        assert_eq!(locations(&v), vec![(PathBuf::from("/y.rs"), 1, 0)]);
        assert_eq!(
            hover_text(&json!({ "contents": { "kind": "markdown", "value": "fn x()" } })).as_deref(),
            Some("fn x()")
        );
    }

    #[test]
    fn statuses() {
        let mut progress = Progress::default();
        let mut status = |kind: &str, token: &str, extra: Value| {
            let mut v = json!({ "kind": kind });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let msg = json!({ "method": "$/progress", "params": { "token": token, "value": v } });
            match status_of(&msg, &mut progress) {
                Some(Event::Status(s)) => s,
                Some(Event::Idle) => "idle".into(),
                _ => "-".into(),
            }
        };
        assert_eq!(status("begin", "a", json!({ "title": "Indexing" })), "Indexing");
        assert_eq!(status("begin", "b", json!({ "title": "Fetching" })), "Fetching");
        assert_eq!(
            status("report", "a", json!({ "message": "3/9", "percentage": 33 })),
            "Indexing 3/9 33%"
        );
        assert_eq!(status("end", "a", json!({})), "-"); // "b" still running
        assert_eq!(status("end", "b", json!({})), "idle");
        let err = json!({ "method": "window/showMessage", "params": { "type": 1, "message": "failed to load" } });
        assert!(matches!(status_of(&err, &mut Progress::default()), Some(Event::Status(s)) if s == "failed to load"));
        let info = json!({ "method": "window/showMessage", "params": { "type": 3, "message": "hi" } });
        assert!(status_of(&info, &mut Progress::default()).is_none());

        // With serverStatus, only `quiescent` means idle.
        let mut p = Progress::default();
        let st =
            |q: bool| json!({ "method": "experimental/serverStatus", "params": { "health": "ok", "quiescent": q } });
        assert!(status_of(&st(false), &mut p).is_none());
        let begin =
            json!({ "method": "$/progress", "params": { "token": "x", "value": { "kind": "begin", "title": "T" } } });
        let end = json!({ "method": "$/progress", "params": { "token": "x", "value": { "kind": "end" } } });
        status_of(&begin, &mut p);
        assert!(status_of(&end, &mut p).is_none());
        assert!(matches!(status_of(&st(true), &mut p), Some(Event::Idle)));
    }

    #[test]
    fn framing() {
        let mut r = std::io::Cursor::new(b"Content-Length: 8\r\nContent-Type: x\r\n\r\n{\"a\":1}\n".to_vec());
        assert_eq!(read_message(&mut r), Some(json!({ "a": 1 })));
    }
}
