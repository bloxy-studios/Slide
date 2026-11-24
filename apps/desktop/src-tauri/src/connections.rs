use crate::manager::db_error;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
use url::Url;
use uuid::Uuid;

const FIXTURE_PORT: u16 = 4320;
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum Selection {
    #[default]
    Direct,
    Saved {
        id: String,
    },
    Workspace {
        endpoint: String,
    },
}

#[derive(Serialize)]
pub struct Saved {
    pub id: String,
    pub name: String,
    pub endpoint: String,
}

pub struct Proxy(Url);

impl Proxy {
    fn parse(input: &str) -> Result<Self, String> {
        let (scheme, authority) = input
            .trim()
            .split_once("://")
            .ok_or("Use an HTTP proxy endpoint such as http://127.0.0.1:4312.")?;
        if scheme != "http" {
            return Err("This build supports HTTP proxies only. HTTPS and SOCKS need separate verification.".into());
        }
        if authority.is_empty()
            || authority.len() > 300
            || authority.contains(['@', '/', '\\', '?', '#', ',', '%'])
            || authority
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(
                "Enter only the proxy host and port, without credentials or a path.".into(),
            );
        }
        let url = Url::parse(input.trim()).map_err(|_| "Invalid HTTP proxy host or port.")?;
        if url.host().is_none() || url.port_or_known_default() == Some(0) {
            return Err("Invalid HTTP proxy host or port.".into());
        }
        Ok(Self(url))
    }

    fn endpoint(&self) -> String {
        self.0.origin().ascii_serialization()
    }

    pub fn launch_flag(&self) -> String {
        format!("--proxy-server={}", self.endpoint())
    }

    fn open(&self) -> Result<TcpStream, String> {
        let host = match self.0.host().ok_or("The proxy is missing a host.")? {
            url::Host::Domain(host) => host.to_owned(),
            url::Host::Ipv4(host) => host.to_string(),
            url::Host::Ipv6(host) => host.to_string(),
        };
        let addresses = (host.as_str(), self.0.port_or_known_default().unwrap())
            .to_socket_addrs()
            .map_err(|_| "The required HTTP proxy host could not be resolved.")?;
        for address in addresses.take(3) {
            if let Ok(socket) = TcpStream::connect_timeout(&address, TIMEOUT) {
                return Ok(socket);
            }
        }
        Err("The required HTTP proxy is unavailable. No direct fallback was attempted.".into())
    }

    pub fn require_reachable(&self) -> Result<(), String> {
        self.open().map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::Manager;

    #[test]
    fn proxy_only_places_validated_endpoint_in_launch_arguments() {
        assert!(Proxy::parse("http://username:password@127.0.0.1:4312").is_err());
        assert!(Proxy::parse("http://127.0.0.1:4312,direct://").is_err());
        assert!(Proxy::parse("http://127.0.0.1:4312/route").is_err());
        assert!(Proxy::parse("socks5://127.0.0.1:4312").is_err());
        assert_eq!(
            Proxy::parse("http://127.0.0.1:4312").unwrap().launch_flag(),
            "--proxy-server=http://127.0.0.1:4312"
        );
    }

    #[test]
    fn bulk_import_is_account_scoped_and_rolls_back_when_any_endpoint_is_invalid() {
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(root.path().into()).unwrap();
        let db = manager.db_mut().unwrap();
        let other = crate::catalog::add_account(db, "Another account").unwrap();
        let mixed =
            "name,endpoint\nFixture,http://127.0.0.1:4312\nPrivate,http://user:pass@localhost:4312";
        assert!(import_csv(db, "local", mixed).is_err());
        assert!(list(db, "local").unwrap().is_empty());
        assert_eq!(
            import_csv(db, "local", "name,endpoint\nFixture,http://127.0.0.1:4312").unwrap(),
            1
        );
        let saved = list(db, "local").unwrap().pop().unwrap();
        assert!(list(db, &other).unwrap().is_empty());
        assert!(resolve(db, &other, &Selection::Saved { id: saved.id }).is_err());
    }
}

pub fn save(db: &Connection, account: &str, name: &str, endpoint: &str) -> Result<String, String> {
    let endpoint = Proxy::parse(endpoint)?.endpoint();
    let id = Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO saved_connections VALUES (?1,?2,?3,?4)",
        params![id, account, crate::catalog::name(name)?, endpoint],
    )
    .map_err(db_error)?;
    Ok(id)
}

pub fn import_csv(db: &mut Connection, account: &str, input: &str) -> Result<usize, String> {
    if input.len() > 32 * 1024 {
        return Err("Use at most 32 KiB of connection CSV.".into());
    }
    let mut reader = csv::Reader::from_reader(input.as_bytes());
    if reader
        .headers()
        .map_err(|_| "Invalid connection CSV header.")?
        .iter()
        .ne(["name", "endpoint"])
    {
        return Err("CSV must have the header name,endpoint. Do not include credentials.".into());
    }
    let transaction = db.transaction().map_err(db_error)?;
    let mut count = 0;
    for (index, row) in reader.records().enumerate() {
        if index >= 100 {
            return Err("Import at most 100 connections at once.".into());
        }
        let row = row.map_err(|_| format!("Invalid connection CSV row {}.", index + 2))?;
        save(&transaction, account, &row[0], &row[1])
            .map_err(|error| format!("Row {}: {error}", index + 2))?;
        count += 1;
    }
    if count == 0 {
        return Err("Add at least one connection row.".into());
    }
    transaction.commit().map_err(db_error)?;
    Ok(count)
}

pub fn list(db: &Connection, account: &str) -> Result<Vec<Saved>, String> {
    let mut query = db
        .prepare(
            "SELECT id,name,endpoint FROM saved_connections WHERE account_id=?1 ORDER BY name,id",
        )
        .map_err(db_error)?;
    let rows = query
        .query_map([account], |row| {
            Ok(Saved {
                id: row.get(0)?,
                name: row.get(1)?,
                endpoint: row.get(2)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<rusqlite::Result<_>>().map_err(db_error)
}

pub fn resolve(
    db: &Connection,
    account: &str,
    selection: &Selection,
) -> Result<Option<Proxy>, String> {
    match selection {
        Selection::Direct => Ok(None),
        Selection::Workspace { endpoint } => Proxy::parse(endpoint).map(Some),
        Selection::Saved { id } => {
            let endpoint: String = db
                .query_row(
                    "SELECT endpoint FROM saved_connections WHERE account_id=?1 AND id=?2",
                    params![account, id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_error)?
                .ok_or("The required saved connection is unavailable for this account.")?;
            Proxy::parse(&endpoint).map(Some)
        }
    }
}

pub fn validate(db: &Connection, account: &str, selection: &mut Selection) -> Result<(), String> {
    let proxy = resolve(db, account, selection)?;
    if let (Selection::Workspace { endpoint }, Some(proxy)) = (selection, proxy) {
        *endpoint = proxy.endpoint();
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    username: String,
    password: String,
}

pub fn test_route(
    proxy: Option<Proxy>,
    credentials: Option<Credentials>,
) -> Result<String, String> {
    let mut auth = String::new();
    if let Some(credentials) = credentials {
        let host = proxy.as_ref().and_then(|proxy| proxy.0.host());
        if !matches!(host, Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            && !matches!(host, Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        {
            return Err("Authenticated checks are limited to a loopback HTTP test proxy; credentials are never sent in cleartext to remote proxies by Slide.".into());
        }
        if credentials.username.contains(':')
            || credentials.username.len() > 256
            || credentials.password.len() > 256
        {
            return Err(
                "Use a test username without a colon and credentials of at most 256 bytes each."
                    .into(),
            );
        }
        auth = format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(format!("{}:{}", credentials.username, credentials.password))
        );
    }
    let host = format!("127.0.0.1:{FIXTURE_PORT}");
    let (mut socket, target, expected) = match proxy {
        Some(proxy) => (proxy.open()?, format!("http://{host}/route"), "proxy"),
        None => (
            TcpStream::connect_timeout(&([127, 0, 0, 1], FIXTURE_PORT).into(), TIMEOUT)
                .map_err(|_| "The local route fixture is not reachable.")?,
            "/route".into(),
            "direct",
        ),
    };
    socket
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|_| socket.set_write_timeout(Some(TIMEOUT)))
        .map_err(|_| "Could not set route probe timeouts.")?;
    write!(
        socket,
        "GET {target} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n"
    )
    .map_err(|_| "Could not send the fixture route probe.")?;
    let mut response = BufReader::new(socket.take(4096));
    let mut line = String::new();
    response
        .read_line(&mut line)
        .map_err(|_| "The route probe did not receive an HTTP response.")?;
    match line.split_whitespace().nth(1) {
        Some("407") => {
            return Err(
                "Proxy authentication is required or the test credentials were rejected.".into(),
            )
        }
        Some("200") => {}
        _ => return Err("The controlled route endpoint did not return HTTP 200.".into()),
    };
    loop {
        line.clear();
        if response
            .read_line(&mut line)
            .map_err(|_| "The route probe response was incomplete.")?
            == 0
            || line == "\r\n"
        {
            return Err(
                "The controlled endpoint did not confirm the required route marker.".into(),
            );
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("X-Slide-Fixture-Route") && value.trim() == expected {
                return Ok(format!(
                    "Fixture route confirmed: {expected} (manager probe; not a browser test)."
                ));
            }
        }
    }
}
