//! Protected local orchestrator console. All reads and writes require an authenticated session.
use crate::{
    access::{Access, SESSION_TTL},
    console::Console,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 65_536;
const MAX_CONNECTIONS: usize = 16;
const SHELL: &str = include_str!("console.html");
const STYLE: &str = include_str!("console.css");
const SCRIPT: &str = include_str!("console.js");
const FAVICON: &str = include_str!("favicon.svg");

#[derive(Clone, Copy)]
enum Content {
    Html,
    Json,
    Css,
    Javascript,
    Svg,
}

// Keep Referrer-Policy at same-origin. Chromium can serialize a form POST's
// Origin as "null" under no-referrer, which would defeat the exact-Origin check.
fn response(status: &str, content: Content, body: &str, extra: &str) -> String {
    let (mime, csp) = match content {
        Content::Html => (
            "text/html; charset=utf-8",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
        Content::Json => ("application/json; charset=utf-8", "default-src 'none'"),
        Content::Css => ("text/css; charset=utf-8", "default-src 'none'"),
        Content::Javascript => ("text/javascript; charset=utf-8", "default-src 'none'"),
        Content::Svg => ("image/svg+xml", "default-src 'none'"),
    };
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: {csp}\r\nReferrer-Policy: same-origin\r\n{extra}\r\n{body}",
        body.len()
    )
}

fn json<T: Serialize>(status: &str, value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(body) => response(status, Content::Json, &body, ""),
        Err(_) => response(
            "500 Internal Server Error",
            Content::Json,
            r#"{"ok":false,"error":"serialization failed"}"#,
            "",
        ),
    }
}

fn html(status: &str, body: impl Into<String>) -> String {
    response(status, Content::Html, &body.into(), "")
}

fn page(csrf: &str) -> String {
    SHELL.replace("{{CSRF}}", csrf)
}

fn login_page() -> String {
    html(
        "200 OK",
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="description" content="Synarch local orchestrator console"><meta name="theme-color" content="#151814"><title>Synarch · Enter</title><link rel="icon" href="/assets/favicon.svg" type="image/svg+xml"><link rel="stylesheet" href="/assets/app.css"></head><body class="login-body"><main class="login"><div class="login-mark" aria-hidden="true"></div><p class="eyebrow">Local access</p><h1>Synarch is listening.</h1><p class="lede">Enter the one-time code printed in the terminal where Synarch is running. It expires after five minutes.</p><form method="post" action="/login"><label for="code">One-time code</label><input id="code" name="code" type="password" autocomplete="off" spellcheck="false" required maxlength="64"><button type="submit">Open the console</button></form><p class="fine">Never share this code. Stop the app with Ctrl+C in its terminal. Other processes running as you are not isolated from this app.</p></main></body></html>"##,
    )
}

fn not_found(auth: bool) -> String {
    let body = if auth {
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Synarch · Not found</title><link rel="icon" href="/assets/favicon.svg" type="image/svg+xml"><link rel="stylesheet" href="/assets/app.css"></head><body class="login-body"><main class="login"><p class="eyebrow">404</p><h1>That page is not here.</h1><p class="lede">Return to the console and continue from the run list.</p><a class="button-link" href="/">Back to Synarch</a></main></body></html>"##
    } else {
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Synarch · Not found</title><link rel="icon" href="/assets/favicon.svg" type="image/svg+xml"><link rel="stylesheet" href="/assets/app.css"></head><body class="login-body"><main class="login"><p class="eyebrow">404</p><h1>Nothing to see here.</h1><p class="lede">This local app only serves authenticated pages.</p><a class="button-link" href="/">Enter Synarch</a></main></body></html>"##
    };
    html("404 Not Found", body)
}

fn fields(body: &str) -> Option<std::collections::HashMap<String, String>> {
    let url = reqwest::Url::parse(&format!("http://localhost/?{body}")).ok()?;
    let mut fields = std::collections::HashMap::new();
    for (key, value) in url.query_pairs() {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return None;
        }
    }
    Some(fields)
}

fn session_id(value: Option<&String>) -> Result<&str> {
    let value = value.map_or("", String::as_str);
    if value.is_empty() {
        return Ok("");
    }
    anyhow::ensure!(
        value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "invalid session ID"
    );
    Ok(value)
}

fn selected_session(target: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    let parsed = reqwest::Url::parse(&format!("http://localhost/?{query}")).ok()?;
    parsed
        .query_pairs()
        .find(|(key, _)| key == "session")
        .map(|(_, value)| value.into_owned())
        .filter(|value| {
            value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn api_error(status: &str, message: &str) -> String {
    json(status, &serde_json::json!({"ok": false, "error": message}))
}

fn process(access: &mut Access, console: &mut Console, raw: &[u8], port: u16, now: u64) -> String {
    let forbidden = || {
        api_error(
            "403 Forbidden",
            "Request denied. Reload the page and sign in again.",
        )
    };
    let bad = || api_error("400 Bad Request", "Bad request");
    let Ok(raw) = std::str::from_utf8(raw) else {
        return bad();
    };
    let Some((head, body)) = raw.split_once("\r\n\r\n") else {
        return bad();
    };
    let mut lines = head.split("\r\n");
    let Some(first) = lines.next() else {
        return bad();
    };
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some("HTTP/1.1"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return bad();
    };
    let mut headers = std::collections::HashMap::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            return bad();
        };
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || headers
                .insert(key.to_ascii_lowercase(), value.trim())
                .is_some()
        {
            return bad();
        }
    }
    if headers.contains_key("transfer-encoding") {
        return bad();
    }
    let length = match headers.get("content-length") {
        Some(value) => match value.parse::<usize>() {
            Ok(n) => n,
            Err(_) => return bad(),
        },
        None => 0,
    };
    if length != body.len() || length > LIMIT {
        return bad();
    }
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://{host}");
    if headers.get("host") != Some(&host.as_str())
        || headers.get("origin").is_some_and(|v| *v != origin)
    {
        return forbidden();
    }
    if method == "POST"
        && (headers.get("origin") != Some(&origin.as_str())
            || headers.get("content-type") != Some(&"application/x-www-form-urlencoded")
            || !headers.contains_key("content-length"))
    {
        return forbidden();
    }
    let cookie = headers
        .get("cookie")
        .and_then(|cookie| {
            let mut matches = cookie
                .split(';')
                .filter_map(|value| value.trim().strip_prefix("synarch_session="));
            let first = matches.next();
            if matches.next().is_some() {
                None
            } else {
                first
            }
        })
        .unwrap_or("");
    let authenticated = access.authorize(cookie, now);
    let csrf = access.csrf(cookie, now).map(str::to_owned);
    match (method, target) {
        ("GET", "/") if body.is_empty() => {
            if let Some(csrf) = csrf {
                html("200 OK", page(&csrf))
            } else {
                login_page()
            }
        }
        ("GET", path) if path.starts_with("/assets/") && body.is_empty() => match path {
            "/assets/app.css" => response("200 OK", Content::Css, STYLE, ""),
            "/assets/app.js" => response("200 OK", Content::Javascript, SCRIPT, ""),
            "/assets/favicon.svg" => response("200 OK", Content::Svg, FAVICON, ""),
            _ => not_found(authenticated),
        },
        ("POST", "/login") => {
            let Some(code) = body.strip_prefix("code=") else {
                return forbidden();
            };
            if let Some((session, _)) = access.exchange(code, now) {
                response(
                    "303 See Other",
                    Content::Html,
                    "",
                    &format!(
                        "Location: /\r\nSet-Cookie: synarch_session={session}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={SESSION_TTL}\r\n"
                    ),
                )
            } else {
                forbidden()
            }
        }
        ("POST", "/logout") => {
            let Some(form) = fields(body) else {
                return bad();
            };
            if form.len() != 1
                || !access.mutation_allowed(
                    cookie,
                    form.get("csrf").map_or("", String::as_str),
                    now,
                )
            {
                return forbidden();
            }
            access.logout();
            response(
                "303 See Other",
                Content::Html,
                "",
                "Location: /\r\nSet-Cookie: synarch_session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0\r\n",
            )
        }
        ("GET", path)
            if (path == "/api/state" || path.starts_with("/api/state?")) && body.is_empty() =>
        {
            if !authenticated {
                return forbidden();
            }
            let selected = selected_session(target);
            let state = console.state(selected.as_deref());
            json("200 OK", &state)
        }
        ("POST", "/api/chat") => {
            let Some(form) = fields(body) else {
                return bad();
            };
            if form.len() < 2
                || form.len() > 3
                || !access.mutation_allowed(
                    cookie,
                    form.get("csrf").map_or("", String::as_str),
                    now,
                )
            {
                return forbidden();
            }
            let selected = match session_id(form.get("session")) {
                Ok("") => None,
                Ok(value) => Some(value),
                Err(_) => return bad(),
            };
            let Some(text) = form.get("text") else {
                return bad();
            };
            match console.send_chat(selected, text) {
                Ok(session) => json(
                    "202 Accepted",
                    &serde_json::json!({"ok": true, "session": session}),
                ),
                Err(error) => api_error("409 Conflict", &error.to_string()),
            }
        }
        ("POST", "/api/new") => {
            let Some(form) = fields(body) else {
                return bad();
            };
            if form.len() != 1
                || !access.mutation_allowed(
                    cookie,
                    form.get("csrf").map_or("", String::as_str),
                    now,
                )
            {
                return forbidden();
            }
            match console.create_intake() {
                Ok(session) => json(
                    "200 OK",
                    &serde_json::json!({"ok": true, "session": session.id}),
                ),
                Err(error) => api_error("503 Service Unavailable", &error.to_string()),
            }
        }
        ("POST", "/api/start") => {
            let Some(form) = fields(body) else {
                return bad();
            };
            if form.len() != 3
                || !access.mutation_allowed(
                    cookie,
                    form.get("csrf").map_or("", String::as_str),
                    now,
                )
            {
                return forbidden();
            }
            let Ok(session) = session_id(form.get("session")) else {
                return bad();
            };
            if session.is_empty() {
                return bad();
            }
            let Some(message) = form.get("message") else {
                return bad();
            };
            if message.len() > 128
                || !message
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return bad();
            }
            match console.start_run(session, message) {
                Ok(session) => json(
                    "200 OK",
                    &serde_json::json!({"ok": true, "session": session}),
                ),
                Err(error) => api_error("422 Unprocessable Entity", &error.to_string()),
            }
        }
        ("POST", "/api/interrupt") => {
            let Some(form) = fields(body) else {
                return bad();
            };
            if form.len() != 2
                || !access.mutation_allowed(
                    cookie,
                    form.get("csrf").map_or("", String::as_str),
                    now,
                )
            {
                return forbidden();
            }
            let Ok(session) = session_id(form.get("session")) else {
                return bad();
            };
            if session.is_empty() {
                return bad();
            }
            match console.interrupt(session) {
                Ok(interrupted) => json(
                    "200 OK",
                    &serde_json::json!({"ok": true, "interrupted": interrupted}),
                ),
                Err(error) => api_error("409 Conflict", &error.to_string()),
            }
        }
        ("GET", path) if path.starts_with("/api/transcript") && body.is_empty() => {
            if !authenticated {
                return forbidden();
            }
            let Some(query) = path.split_once('?').map(|(_, query)| query) else {
                return bad();
            };
            let Ok(url) = reqwest::Url::parse(&format!("http://localhost/?{query}")) else {
                return bad();
            };
            let root = url
                .query_pairs()
                .find(|(key, _)| key == "root")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let session = url
                .query_pairs()
                .find(|(key, _)| key == "session")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            if root.len() > 128
                || session.len() > 128
                || !root
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                || !session
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return bad();
            }
            match console.transcript(&root, &session) {
                Ok(value) => json("200 OK", &value),
                Err(error) => api_error("404 Not Found", &error.to_string()),
            }
        }
        ("GET", path) if path.starts_with("/api/diff") && body.is_empty() => {
            if !authenticated {
                return forbidden();
            }
            let session = selected_session(path).unwrap_or_default();
            if session.is_empty() {
                return bad();
            }
            match console.diff(&session) {
                Ok(value) => json("200 OK", &value),
                Err(error) => api_error("404 Not Found", &error.to_string()),
            }
        }
        ("GET", _) if authenticated => not_found(true),
        ("GET", _) => not_found(false),
        ("POST", _) => api_error("404 Not Found", "Not found"),
        _ => api_error("405 Method Not Allowed", "Unsupported method"),
    }
}

fn handle(
    stream: &mut TcpStream,
    access: &Arc<Mutex<Access>>,
    console: &Arc<Mutex<Console>>,
    port: u16,
    app_started: Instant,
) -> Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let started = Instant::now();
    let mut input = Vec::new();
    let mut byte = [0u8; 1];
    while input.len() < 16_384 && !input.ends_with(b"\r\n\r\n") {
        let remaining = Duration::from_secs(3)
            .checked_sub(started.elapsed())
            .context("request timed out")?;
        stream.set_read_timeout(Some(remaining))?;
        if stream.read(&mut byte)? == 0 {
            break;
        }
        input.push(byte[0]);
    }
    if !input.ends_with(b"\r\n\r\n") {
        stream.write_all(api_error("400 Bad Request", "Bad request").as_bytes())?;
        return Ok(());
    }
    let header = String::from_utf8_lossy(&input);
    let lengths: Vec<_> = header
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim())
        .collect();
    let length = match lengths.as_slice() {
        [] => 0,
        [value] => value.parse::<usize>().unwrap_or(LIMIT + 1),
        _ => LIMIT + 1,
    };
    if length > LIMIT {
        stream.write_all(api_error("400 Bad Request", "Bad request").as_bytes())?;
        return Ok(());
    }
    let start = input.len();
    input.resize(start + length, 0);
    let mut position = start;
    while position < input.len() {
        let remaining = Duration::from_secs(3)
            .checked_sub(started.elapsed())
            .context("request timed out")?;
        stream.set_read_timeout(Some(remaining))?;
        let read = stream.read(&mut input[position..])?;
        anyhow::ensure!(read != 0, "incomplete request body");
        position += read;
    }
    let now = app_started.elapsed().as_secs();
    let mut access = access
        .lock()
        .map_err(|_| anyhow::anyhow!("access state unavailable"))?;
    let mut console = console
        .lock()
        .map_err(|_| anyhow::anyhow!("console state unavailable"))?;
    let output = process(&mut access, &mut console, &input, port, now);
    drop(console);
    drop(access);
    stream.write_all(output.as_bytes())?;
    Ok(())
}

/// Start the protected console. The one-time secret is written only to the controlling terminal.
pub fn serve(port: u16, database: &Path) -> Result<()> {
    let mut tty = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("the protected console requires a controlling terminal")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let port = listener.local_addr()?.port();
    let (access, code) = Access::new(0)?;
    let console = Console::new(database);
    let access = Arc::new(Mutex::new(access));
    let console = Arc::new(Mutex::new(console));
    let active = Arc::new(AtomicUsize::new(0));
    writeln!(
        tty,
        "Synarch orchestrator: http://127.0.0.1:{port}/\nOne-time code (expires in 5 minutes): {code}\nStop with Ctrl+C."
    )?;
    tty.flush()?;
    let started = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        api_error("503 Service Unavailable", "Too many local requests").as_bytes(),
                    );
                    continue;
                }
                let access = Arc::clone(&access);
                let console = Arc::clone(&console);
                let active = Arc::clone(&active);
                thread::Builder::new()
                    .name("synarch-http".into())
                    .spawn(move || {
                        let result = handle(&mut stream, &access, &console, port, started);
                        active.fetch_sub(1, Ordering::SeqCst);
                        if let Err(error) = result {
                            eprintln!("local app request failed: {error}");
                        }
                    })?;
            }
            Err(error) => eprintln!("local app connection failed: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::Console;

    fn console() -> Console {
        let dir = tempfile::tempdir().unwrap();
        Console::new(&dir.path().join("state.sqlite"))
    }

    #[test]
    fn rejects_untrusted_host_origin_and_mutations() {
        let (mut access, code) = Access::new(0).unwrap();
        let mut console = console();
        let login = format!(
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\ncode={}",
            5 + code.len(),
            code
        );
        let response = process(&mut access, &mut console, login.as_bytes(), 8788, 1);
        assert!(response.starts_with("HTTP/1.1 303"));
        assert!(response.contains("HttpOnly; Secure; SameSite=Strict"));
        assert!(!response.contains(&code));
        assert!(
            process(&mut access, &mut console, login.as_bytes(), 8788, 2)
                .starts_with("HTTP/1.1 403")
        );
        assert!(
            process(
                &mut access,
                &mut console,
                b"GET / HTTP/1.1\r\nHost: attacker.test\r\n\r\n",
                8788,
                2
            )
            .starts_with("HTTP/1.1 403")
        );
        let (_, code) = Access::new(0).unwrap();
        let cross = format!(
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: https://evil.test\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\ncode={}",
            5 + code.len(),
            code
        );
        assert!(
            process(&mut access, &mut console, cross.as_bytes(), 8788, 2)
                .starts_with("HTTP/1.1 403")
        );
        let mutation = b"POST /api/new HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 6\r\n\r\ncsrf=x";
        assert!(process(&mut access, &mut console, mutation, 8788, 2).starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn authenticated_pages_are_served_with_a_csrf_meta_tag() {
        let (mut access, code) = Access::new(0).unwrap();
        let mut console = console();
        let body = format!("code={code}");
        let request = format!(
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let login = process(&mut access, &mut console, request.as_bytes(), 8788, 1);
        let cookie = login
            .split("Set-Cookie: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let get = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nCookie: {cookie}\r\n\r\n");
        let page = process(&mut access, &mut console, get.as_bytes(), 8788, 2);
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("synarch-csrf"));
        assert!(page.contains("/assets/app.js"));
        assert!(!page.contains("<form method=\"post\" action=\"/preview\""));
        let asset = format!(
            "GET /assets/app.js HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nCookie: {cookie}\r\n\r\n"
        );
        let asset = process(&mut access, &mut console, asset.as_bytes(), 8788, 2);
        assert!(asset.contains("text/javascript"));
        let _ = login;
    }

    #[test]
    fn expired_sessions_fall_back_to_login_and_bad_lengths_are_rejected() {
        let (mut access, code) = Access::new(0).unwrap();
        let mut console = console();
        let body = format!("code={code}");
        let request = format!(
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let login = process(&mut access, &mut console, request.as_bytes(), 8788, 1);
        let cookie = login
            .split("Set-Cookie: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let expired = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nCookie: {cookie}\r\n\r\n");
        let expired = process(&mut access, &mut console, expired.as_bytes(), 8788, 9999);
        assert!(expired.starts_with("HTTP/1.1 200"));
        assert!(expired.contains("Synarch is listening"));
        assert!(!expired.contains("synarch-csrf"));
        let bad = b"POST /api/new HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 6\r\n\r\ncsrf=";
        assert!(
            process(&mut access, &mut console, bad, 8788, 1).starts_with("HTTP/1.1 400")
                || process(&mut access, &mut console, bad, 8788, 1).starts_with("HTTP/1.1 403")
        );
        let dup = b"GET / HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nHost: attacker.test\r\n\r\n";
        assert!(process(&mut access, &mut console, dup, 8788, 1).starts_with("HTTP/1.1 400"));
    }
}

#[cfg(test)]
mod console_api_tests {
    use super::*;
    use crate::console::FakeApi;
    use std::sync::Arc;

    fn login(access: &mut Access, console: &mut Console, code: &str, now: u64) -> String {
        let body = format!("code={code}");
        let request = format!(
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = process(access, console, request.as_bytes(), 8788, now);
        response
            .split("Set-Cookie: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    fn csrf(access: &mut Access, console: &mut Console, cookie: &str, now: u64) -> String {
        let get = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nCookie: {cookie}\r\n\r\n");
        let page = process(access, console, get.as_bytes(), 8788, now);
        page.split("name=\"synarch-csrf\" content=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn state_and_new_run_are_authenticated_json_and_start_rejects_stale_proposals() {
        let (mut access, code) = Access::new(0).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeApi::new());
        let mut console = Console::with_backend(&dir.path().join("state.sqlite"), backend);
        console.ensure_ready().unwrap();
        let intake = console.create_intake().unwrap();
        let cookie = login(&mut access, &mut console, &code, 1);
        let csrf = csrf(&mut access, &mut console, &cookie, 2);
        let state = format!(
            "GET /api/state?session={} HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nCookie: {cookie}\r\n\r\n",
            intake.id
        );
        let state = process(&mut access, &mut console, state.as_bytes(), 8788, 2);
        assert!(state.starts_with("HTTP/1.1 200"));
        assert!(state.contains("\"connected\":true"));
        assert!(state.contains(&intake.id));
        let unauthorized = b"GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:8788\r\n\r\n";
        assert!(
            process(&mut access, &mut console, unauthorized, 8788, 2).starts_with("HTTP/1.1 403")
        );
        let body = format!("csrf={csrf}");
        let new_run = format!(
            "POST /api/new HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nCookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let new_run = process(&mut access, &mut console, new_run.as_bytes(), 8788, 3);
        assert!(new_run.starts_with("HTTP/1.1 200"));
        assert!(new_run.contains("\"ok\":true"));
        let stale = format!("csrf={csrf}&session={}&message=msg_missing", intake.id);
        let start = format!(
            "POST /api/start HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nOrigin: http://127.0.0.1:8788\r\nCookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{stale}",
            stale.len()
        );
        let start = process(&mut access, &mut console, start.as_bytes(), 8788, 4);
        assert!(start.starts_with("HTTP/1.1 422"), "{start}");
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;
    use crate::console::{Console, FakeApi};
    use std::sync::Arc;

    #[test]
    fn network_handler_rejects_an_oversized_length_before_reading_a_body() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (access, _) = Access::new(0).unwrap();
        let access = Arc::new(Mutex::new(access));
        let console = Arc::new(Mutex::new(Console::with_backend(
            &std::env::temp_dir().join(format!("synarch-network-{port}.sqlite")),
            Arc::new(FakeApi::new()),
        )));
        let worker_access = Arc::clone(&access);
        let worker_console = Arc::clone(&console);
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle(
                &mut stream,
                &worker_access,
                &worker_console,
                port,
                Instant::now(),
            )
            .unwrap();
        });
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(
            client,
            "POST /login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 999999\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        worker.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }
}
