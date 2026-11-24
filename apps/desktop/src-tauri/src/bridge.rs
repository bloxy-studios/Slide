use crate::{catalog, manager::{Manager, Operation}, AppState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{hmac, rand::{SecureRandom, SystemRandom}};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager as _};
use uuid::Uuid;

const MAX_CONNECTIONS: usize = 8;

#[derive(Default)]
pub struct Bridge(Mutex<Option<Listener>>);

struct Listener {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Drop for Listener {
    fn drop(&mut self) { self.stop.store(true, Ordering::Release); }
}

#[derive(Serialize)]
pub struct Status {
    endpoint: Option<String>,
    token: Option<String>,
    audit: Vec<Value>,
}

fn key() -> hmac::Key { hmac::Key::new(hmac::HMAC_SHA256, b"Slide local bridge token v1") }

fn audit(manager: &Manager, operation: &str, workspace: Option<&str>, outcome: &str) -> Result<i64, String> {
    let db = manager.db()?;
    db.execute("INSERT INTO automation_audit (created,account_id,operation,workspace_id,outcome) VALUES (?1,?2,?3,?4,?5)",
        params![catalog::now(), catalog::account(db)?, operation, workspace, outcome]).map_err(|e| e.to_string())?;
    Ok(db.last_insert_rowid())
}

fn rotate(manager: &Manager) -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "Could not generate a bridge token.")?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    manager.db()?.execute("INSERT INTO automation_token VALUES (1,?1) ON CONFLICT(singleton) DO UPDATE SET digest=excluded.digest",
        [hmac::sign(&key(), token.as_bytes()).as_ref()]).map_err(|e| e.to_string())?;
    audit(manager, "token.rotate", None, "ok")?;
    Ok(token)
}

impl Bridge {
    pub fn control(&self, app: AppHandle, manager: &Manager, action: &str, port: u16) -> Result<Status, String> {
        let mut active = self.0.lock().map_err(|_| "Automation bridge unavailable.")?;
        let db = manager.db()?;
        let mut token = None;
        match action {
            "status" => (),
            "rotate" => token = Some(rotate(manager)?),
            "disable" => {
                if active.take().is_some() { audit(manager, "bridge.disable", None, "ok")?; }
            }
            "enable" if active.is_none() => {
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                    .map_err(|e| format!("Could not bind the loopback bridge: {e}"))?;
                listener.set_nonblocking(true).map_err(|e| e.to_string())?;
                let port = listener.local_addr().map_err(|e| e.to_string())?.port();
                if !db.query_row("SELECT EXISTS(SELECT 1 FROM automation_token)", [], |row| row.get::<_, bool>(0))
                    .map_err(|e| e.to_string())? {
                    token = Some(rotate(manager)?);
                }
                audit(manager, "bridge.enable", None, "ok")?;
                let stop = Arc::new(AtomicBool::new(false));
                let listener_stop = stop.clone();
                std::thread::Builder::new().name("slide-automation".into())
                    .spawn(move || serve(app, listener, port, listener_stop)).map_err(|e| e.to_string())?;
                *active = Some(Listener { port, stop });
            }
            "enable" => (),
            _ => return Err("Unknown automation bridge operation.".into()),
        }
        let mut statement = db.prepare("SELECT created,operation,workspace_id,outcome FROM automation_audit
            WHERE account_id=?1 ORDER BY id DESC LIMIT 30").map_err(|e| e.to_string())?;
        let rows = statement.query_map([catalog::account(db)?], |row| Ok(json!({
            "created": row.get::<_, i64>(0)?, "operation": row.get::<_, String>(1)?,
            "workspaceId": row.get::<_, Option<String>>(2)?, "outcome": row.get::<_, String>(3)?,
        }))).map_err(|e| e.to_string())?;
        Ok(Status {
            endpoint: active.as_ref().map(|listener| format!("http://127.0.0.1:{}", listener.port)),
            token,
            audit: rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?,
        })
    }
}

#[derive(Debug)]
enum Action {
    List,
    Status(String),
    Perform(String, Operation, &'static str),
}

type Failure = (u16, &'static str);

fn authorize<'a>(peer: SocketAddr, port: u16, request: &'a str) -> Result<(&'a str, Action), Failure> {
    if peer.ip() != Ipv4Addr::LOCALHOST { return Err((403, "Loopback clients only.")); }
    let (head, body) = request.split_once("\r\n\r\n").ok_or((400, "Invalid request."))?;
    if !body.is_empty() { return Err((400, "Request bodies are not accepted.")); }
    let mut lines = head.split("\r\n");
    let parts: Vec<_> = lines.next().ok_or((400, "Invalid request."))?.split(' ').collect();
    let [method, path, "HTTP/1.1"] = parts.as_slice() else { return Err((400, "Invalid request.")); };
    let (mut host, mut bearer) = (None, None);
    let mut has_length = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or((400, "Invalid header."))?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err((400, "Invalid header."));
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("host") && host.replace(value).is_some()
            || name.eq_ignore_ascii_case("authorization") && bearer.replace(value).is_some() {
            return Err((400, "Duplicate header."));
        }
        if name.eq_ignore_ascii_case("origin") { return Err((403, "Web origins are not accepted.")); }
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && (std::mem::replace(&mut has_length, true) || value != "0")) {
            return Err((400, "Request bodies are not accepted."));
        }
    }
    if host != Some(format!("127.0.0.1:{port}").as_str()) { return Err((403, "Invalid loopback host.")); }
    let token = bearer.and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| token.len() == 43 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        .ok_or((401, "Bridge token required."))?;
    let action = if *method == "GET" && *path == "/v1/workspaces" { Action::List } else {
        let tail = path.strip_prefix("/v1/workspaces/").ok_or((404, "Unknown endpoint."))?;
        let (id, operation) = tail.split_once('/').unwrap_or((tail, ""));
        let id = Uuid::parse_str(id).map_err(|_| (404, "Workspace unavailable."))?.to_string();
        match (*method, operation) {
            ("GET", "") => Action::Status(id),
            ("POST", "open") => Action::Perform(id, Operation::Open, "open"),
            ("POST", "close") => Action::Perform(id, Operation::Close, "close"),
            ("POST", "focus") => Action::Perform(id, Operation::Focus, "focus"),
            _ => return Err((404, "Unknown endpoint.")),
        }
    };
    Ok((token, action))
}

fn token_matches(token: &str, digest: Option<&[u8]>) -> bool {
    digest.is_some_and(|digest| hmac::verify(&key(), token.as_bytes(), digest).is_ok())
}

fn read_request(stream: &mut TcpStream) -> Result<String, Failure> {
    let deadline = Instant::now() + Duration::from_secs(1);
    stream.set_nonblocking(false).map_err(|_| (503, "Could not configure bridge connection."))?;
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { return Err((400, "Request timed out.")); };
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return Err((400, "Request timed out."));
        }
        match stream.read(&mut buffer) {
            Ok(count) if count > 0 && request.len() + count <= 8192 => request.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                return Err((400, "Request timed out.")),
            _ => return Err((400, "Invalid request.")),
        }
    }
    String::from_utf8(request).map_err(|_| (400, "Invalid request."))
}

fn process(app: &AppHandle, stream: &mut TcpStream, peer: SocketAddr, port: u16, stop: &AtomicBool) -> (u16, Value) {
    let failure = |code, message: &str| (code, json!({ "error": message }));
    let request = match read_request(stream) {
        Ok(request) => request,
        Err((code, message)) => return failure(code, message),
    };
    let (token, action) = match authorize(peer, port, &request) {
        Ok(parsed) => parsed,
        Err((code, message)) => return failure(code, message),
    };
    let state = app.state::<AppState>();
    let Ok(mut manager) = state.lock() else { return failure(503, "Manager unavailable."); };
    if stop.load(Ordering::Acquire) { return failure(503, "Bridge disabled."); }
    let Ok(db) = manager.db() else { return failure(503, "Catalog unavailable."); };
    let digest = db.query_row("SELECT digest FROM automation_token WHERE singleton=1", [], |row| row.get::<_, Vec<u8>>(0)).optional();
    let Ok(digest) = digest else { return failure(503, "Bridge authentication unavailable."); };
    if !token_matches(token, digest.as_deref()) { return failure(401, "Invalid bridge token."); }
    let (operation, id) = match &action {
        Action::List => ("list", None),
        Action::Status(id) => ("status", Some(id.as_str())),
        Action::Perform(id, _, name) => (*name, Some(id.as_str())),
    };
    let Ok(audit_id) = audit(&manager, operation, id, "pending") else { return failure(503, "Bridge audit unavailable."); };
    let result = match action {
        Action::List => manager.automation_list(None, false).map(|rows| json!(rows)).map_err(|e| (409, e)),
        Action::Status(id) => manager.automation_list(Some(&id), true).map_err(|e| (409, e))
            .and_then(|mut rows| rows.pop().ok_or((404, "Workspace unavailable.".into()))),
        Action::Perform(id, operation, _) => manager.automation_perform(&id, operation)
            .map(|()| json!({"accepted": true})).map_err(|e| (409, e)),
    };
    if manager.db().is_ok_and(|db| db.execute("UPDATE automation_audit SET outcome=?1 WHERE id=?2",
        params![if result.is_ok() { "ok" } else { "denied" }, audit_id]).is_ok()) {
        match result { Ok(value) => (200, value), Err((code, error)) => failure(code, &error) }
    } else {
        failure(503, "Could not finalize the bridge audit.")
    }
}

fn serve(app: AppHandle, listener: TcpListener, port: u16, stop: Arc<AtomicBool>) {
    serve_connections(listener, stop.clone(), move |mut stream, peer| {
        let (code, body) = process(&app, &mut stream, peer, port, &stop);
        let body = body.to_string();
        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
        let _ = write!(stream, "HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
            match code { 200 => "OK", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found",
                409 => "Conflict", _ => "Service Unavailable" }, body.len(), body);
    });
}

fn serve_connections(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    handle: impl Fn(TcpStream, SocketAddr) + Send + Sync + 'static,
) {
    let handle = Arc::new(handle);
    let mut workers = Vec::with_capacity(MAX_CONNECTIONS);
    while !stop.load(Ordering::Acquire) {
        workers.retain(|worker: &std::thread::JoinHandle<()>| !worker.is_finished());
        match listener.accept() {
            Ok((stream, peer)) if workers.len() < MAX_CONNECTIONS => {
                let handle = handle.clone();
                if let Ok(worker) = std::thread::Builder::new().name("slide-automation-request".into())
                    .spawn(move || handle(stream, peer)) {
                    workers.push(worker);
                }
            }
            Ok((mut stream, _)) => {
                if stream.set_nonblocking(true).is_ok() {
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.shutdown(Shutdown::Write);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_handles_other_connections_while_one_handler_is_busy() {
        use std::sync::mpsc;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let (entered, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(Some(blocked));
        let server = std::thread::spawn(move || serve_connections(listener, server_stop, move |_, _| {
            let first = blocked.lock().unwrap().take();
            entered.send(()).unwrap();
            if let Some(first) = first { let _ = first.recv(); }
        }));

        let idle = TcpStream::connect(address).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let ready = TcpStream::connect(address).unwrap();
        let served_before_release = started.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        stop.store(true, Ordering::Release);
        drop((idle, ready));
        server.join().unwrap();
        served_before_release.unwrap();
    }

    #[test]
    fn bridge_returns_503_and_retry_after_at_capacity() {
        use std::{io::{BufRead, BufReader}, sync::mpsc};

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let (entered, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel::<()>();
        let blocked = Mutex::new(blocked);
        let server = std::thread::spawn(move || serve_connections(listener, server_stop, move |_, _| {
            entered.send(()).unwrap();
            let _ = blocked.lock().unwrap().recv();
        }));
        let clients: Vec<_> = (0..MAX_CONNECTIONS).map(|_| TcpStream::connect(address).unwrap()).collect();
        for _ in &clients {
            started.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let mut extra = TcpStream::connect(address).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(extra, "GET /v1/workspaces HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let mut reply = BufReader::new(extra);
        let headers: Vec<_> = (0..5).map(|_| {
            let mut line = String::new();
            reply.read_line(&mut line).map(|_| line)
        }).collect();
        stop.store(true, Ordering::Release);
        drop(release);
        server.join().unwrap();
        let headers = headers.into_iter().collect::<std::io::Result<Vec<_>>>().unwrap();
        assert_eq!(headers[0], "HTTP/1.1 503 Service Unavailable\r\n");
        assert!(headers.iter().any(|line| line == "Retry-After: 1\r\n"));
        assert!(headers.iter().any(|line| line == "Connection: close\r\n"));
    }

    #[test]
    fn bridge_reads_headers_split_across_delayed_writes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client.write_all(b"GET /v1/workspaces HTTP/1.1\r\n").unwrap();
        let (mut stream, peer) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let (reading, started) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            reading.send(()).unwrap();
            read_request(&mut stream)
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(75));
        let token = "A".repeat(43);
        write!(client, "Host: {address}\r\nAuthorization: Bearer {token}\r\n\r\n").unwrap();
        let request = server.join().unwrap().unwrap();
        let (supplied, action) = authorize(peer, address.port(), &request).unwrap();
        assert_eq!(supplied, token);
        assert!(matches!(action, Action::List));
    }

    #[test]
    fn bridge_rejects_non_loopback_dns_rebinding_web_origins_and_bad_tokens() {
        let local = "127.0.0.1:42000".parse().unwrap();
        let remote = "192.0.2.1:42000".parse().unwrap();
        let token = "A".repeat(43);
        let request = |host: &str, extra: &str| format!(
            "GET /v1/workspaces HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n{extra}\r\n"
        );
        let valid = request("127.0.0.1:4330", "");
        assert_eq!(authorize(remote, 4330, &valid).unwrap_err().0, 403);
        assert_eq!(authorize(local, 4330, &request("rebind.test:4330", "")).unwrap_err().0, 403);
        assert_eq!(authorize(local, 4330, &request("127.0.0.1:4330", "Origin: http://website.test\r\n")).unwrap_err().0, 403);
        let digest = hmac::sign(&key(), token.as_bytes());
        assert!(token_matches(authorize(local, 4330, &valid).unwrap().0, Some(digest.as_ref())));
        assert!(!token_matches(&"B".repeat(43), Some(digest.as_ref())));
        assert_eq!(authorize(local, 4330, "GET /v1/workspaces HTTP/1.1\r\nHost: 127.0.0.1:4330\r\n\r\n").unwrap_err().0, 401);
    }
}
