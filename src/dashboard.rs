//! Read-only, local dashboard. Ledger content is untrusted and may be sensitive.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    fmt::Write as _,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
    time::Instant,
};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn encoded(s: &str) -> String {
    s.bytes().map(|b| format!("%{b:02X}")).collect()
}

fn decoded(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' {
            return None;
        }
        let a = (bytes.next()? as char).to_digit(16)?;
        let b = (bytes.next()? as char).to_digit(16)?;
        out.push((a * 16 + b) as u8);
    }
    String::from_utf8(out).ok()
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "—".into(),
        _ => v.to_string(),
    }
}

fn field(out: &mut String, label: &str, v: &Value) {
    let _ = write!(
        out,
        "<div class=field><dt>{}</dt><dd>{}</dd></div>",
        escape(label),
        escape(&text(v))
    );
}

fn read_only_database(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(3))?;
    let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        app == 0x5359_4e41 && version == 2,
        "unsupported Synarch database"
    );
    db.execute_batch("BEGIN")?;
    Ok(db)
}

/// Opens an existing Synarch v2 database without migrations, locks or write access.
pub fn render(path: &Path, selected: Option<&str>) -> Result<String> {
    let db = read_only_database(path)?;
    let mut out = String::from(HEAD);
    let mut stmt = db.prepare("SELECT id,state FROM runs ORDER BY rowid DESC LIMIT 500")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut count = 0;
    for row in rows {
        let (id, raw) = row?;
        let run: Value = serde_json::from_str(&raw)?;
        count += 1;
        write!(
            out,
            "<a class=run href=\"/?run={}\"><span>{}</span><small>{} · iteration {}</small></a>",
            encoded(&id),
            escape(&id),
            escape(&text(&run["phase"])),
            escape(&text(&run["iteration"]))
        )?;
    }
    if count == 0 {
        out.push_str("<p class=muted>No runs in this database.</p>");
    }
    out.push_str("</nav><main>");
    if let Some(id) = selected {
        let raw: String = db.query_row("SELECT state FROM runs WHERE id=?1", [id], |r| r.get(0))?;
        let run: Value = serde_json::from_str(&raw)?;
        write!(
            out,
            "<div class=eyebrow>RUN / {}</div><h1>Workflow overview</h1><p class=request>{}</p><dl class=metrics>",
            escape(id),
            escape(&text(&run["request"]))
        )?;
        for (label, key) in [
            ("Phase", "phase"),
            ("Revision", "revision"),
            ("Iteration", "iteration"),
            ("Max iterations", "max_iterations"),
            ("Snapshot", "snapshot"),
            ("Engineering accepted", "engineering_accepted"),
            ("QA validated", "qa_validated"),
            ("Paused", "paused"),
        ] {
            field(&mut out, label, &run[key]);
        }
        write!(
            out,
            "</dl><p><a href=\"/?run={}&amp;diff=1\">View recorded snapshot diff →</a></p><section><h2>Checks & findings</h2>",
            encoded(id)
        )?;
        for (title, key) in [("Checks", "checks"), ("Findings", "findings")] {
            write!(
                out,
                "<h3>{title}</h3><pre>{}</pre>",
                escape(&serde_json::to_string_pretty(&run[key])?)
            )?;
        }
        out.push_str("</section><section><h2>Execution budget</h2><dl class=metrics>");
        let raw: String =
            db.query_row("SELECT control FROM controls WHERE run_id=?1", [id], |r| {
                r.get(0)
            })?;
        let control: Value = serde_json::from_str(&raw)?;
        for (label, value) in [
            ("Launches", &control["launches"]),
            ("Launch limit", &control["limits"]["max_agent_launches"]),
            ("Deadline (Unix ms)", &control["deadline_ms"]),
            ("Halt reason", &control["halt_reason"]),
        ] {
            field(&mut out, label, value);
        }
        out.push_str("</dl></section><section><h2>Attempts</h2>");
        let mut attempts = db
            .prepare("SELECT execution FROM attempts WHERE run_id=?1 ORDER BY revision,sequence")?;
        let rows = attempts.query_map([id], |r| r.get::<_, String>(0))?;
        let mut n = 0;
        for row in rows {
            let a: Value = serde_json::from_str(&row?)?;
            n += 1;
            write!(
                out,
                "<article><strong>{} · {}</strong><p>Revision {} / attempt {} · failure: {} · stop confirmed: {}</p></article>",
                escape(&text(&a["attempt"]["role"])),
                escape(&text(&a["state"])),
                escape(&text(&a["attempt"]["revision"])),
                escape(&text(&a["sequence"])),
                escape(&text(&a["failure_code"])),
                escape(&text(&a["stop_confirmed"]))
            )?;
        }
        if n == 0 {
            out.push_str("<p class=muted>No attempts yet.</p>");
        }
        out.push_str("</section><section><h2>Decision timeline</h2>");
        let mut events = db.prepare("SELECT sequence,created_at,payload FROM events WHERE run_id=?1 AND kind='transition' ORDER BY sequence DESC LIMIT 500")?;
        let rows = events.query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut n = 0;
        for row in rows {
            let (seq, time, raw) = row?;
            let event: Value = serde_json::from_str(&raw)?;
            n += 1;
            let report = &event["submission"]["report"];
            write!(
                out,
                "<article><strong>{} → {}</strong><span class=timestamp> #{} · {} UTC</span><p>{} · snapshot {}</p>",
                escape(&text(&event["from"])),
                escape(&text(&event["to"])),
                seq,
                escape(&time),
                escape(&text(&event["role"])),
                escape(&text(&event["submission"]["snapshot"]))
            )?;
            if report["kind"] == "decision" {
                write!(
                    out,
                    "<p><b>{}</b> — {}</p>",
                    escape(&text(&report["verdict"])),
                    escape(&text(&report["rationale"]))
                )?;
            }
            out.push_str("</article>");
        }
        if n == 0 {
            out.push_str("<p class=muted>No transitions yet.</p>");
        }
        out.push_str("</section>");
    } else {
        out.push_str("<div class=eyebrow>LOCAL OBSERVABILITY</div><h1>Workflow overview</h1><p class=muted>Select a run to inspect its state and history.</p>");
    }
    out.push_str("</main></div></body></html>");
    Ok(out)
}

/// Render the recorded baseline-to-snapshot diff. No browser-supplied paths or revisions are used.
pub fn render_diff(path: &Path, id: &str) -> Result<String> {
    let db = read_only_database(path)?;
    let (state, config): (String, String) =
        db.query_row("SELECT state,config FROM runs WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let state: Value = serde_json::from_str(&state)?;
    let config: Value = serde_json::from_str(&config)?;
    // Release the SQLite snapshot before starting Git.
    drop(db);
    let mut out = format!(
        "{} </nav><main><div class=eyebrow>RUN / {}</div><h1>Recorded snapshot diff</h1><p><a href=\"/?run={}\">← Back to overview</a></p>",
        HEAD,
        escape(id),
        encoded(id)
    );
    match (
        config["baseline"].as_str(),
        state["snapshot"].as_str(),
        config["actor_directory"].as_str(),
    ) {
        (Some(base), Some(snapshot), Some(directory)) => {
            write!(
                out,
                "<p>Baseline <code>{}</code><br>Snapshot <code>{}</code></p><p class=muted>Committed changes only. Uncommitted work and QA workspace files are not included. This is not a fresh acceptance check.</p>",
                escape(base),
                escape(snapshot)
            )?;
            match snapshot_diff(Path::new(directory), base, snapshot) {
                Ok(diff) if diff.is_empty() => out.push_str("<p>No committed changes between these snapshots.</p>"),
                Ok(diff) => write!(out, "<pre>{}</pre>", escape(&diff))?,
                Err(_) => out.push_str("<p role=alert>Diff unavailable: the recorded worktree or commits may be missing, invalid, too large, or inaccessible.</p>"),
            }
        }
        _ => out.push_str("<p>No recorded snapshot is available for this run.</p>"),
    }
    out.push_str("</main></div></body></html>");
    Ok(out)
}

fn snapshot_diff(directory: &Path, base: &str, snapshot: &str) -> Result<String> {
    for commit in [base, snapshot] {
        ensure!(
            matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid commit ID"
        );
    }
    ensure!(directory.is_absolute(), "absolute worktree required");
    let mut command = Command::new("git");
    // Do not inherit Git's repository selectors, external diff configuration, or optional locks.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let mut child = command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "--no-pager",
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(directory)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--ignore-submodules=all",
            base,
            snapshot,
            "--",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing diff pipe"))?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(131_073).read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            anyhow::bail!("diff timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("diff reader failed"))??;
    ensure!(
        status.success() && bytes.len() <= 131_072,
        "diff unavailable or exceeds 128 KiB"
    );
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

const HEAD: &str = include_str!("dashboard.html");

/// Listen only on IPv4 loopback; there are no mutation routes.
pub fn serve(path: &Path, port: u16) -> Result<()> {
    // Fail before listening on missing, foreign, or outdated databases.
    render(path, None)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let port = listener.local_addr()?.port();
    eprintln!("Dashboard: http://127.0.0.1:{port}/ (local data may be sensitive)");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(e) = handle(&mut stream, path, port) {
                    eprintln!("dashboard request failed: {e}");
                }
            }
            Err(e) => eprintln!("dashboard connection failed: {e}"),
        }
    }
    Ok(())
}

fn handle(stream: &mut TcpStream, path: &Path, port: u16) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut byte = [0; 1];
    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        request.push(byte[0]);
    }
    let req = String::from_utf8_lossy(&request);
    let mut lines = req.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let target = first.next().unwrap_or("");
    let headers: Vec<_> = lines.filter_map(|l| l.split_once(':')).collect();
    let hosts: Vec<_> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .collect();
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://{host}");
    let allowed = request.ends_with(b"\r\n\r\n")
        && hosts.len() == 1
        && hosts[0].1.trim() == host
        && headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("origin"))
            .all(|(_, v)| v.trim() == origin);
    let selected = if target == "/" {
        Some((None, false))
    } else {
        target.strip_prefix("/?run=").and_then(|s| {
            let (id, diff) = s
                .strip_suffix("&diff=1")
                .map_or((s, false), |id| (id, true));
            decoded(id)
                .filter(|s| s.len() <= 128)
                .map(|id| (Some(id), diff))
        })
    };
    let (status, body) = if !allowed {
        ("403 Forbidden", "Forbidden".to_string())
    } else if method != "GET" {
        ("405 Method Not Allowed", "Read-only dashboard".to_string())
    } else if let Some((selected, diff)) = selected {
        let result = if diff {
            render_diff(path, selected.as_deref().unwrap_or(""))
        } else {
            render(path, selected.as_deref())
        };
        match result {
            Ok(body) => ("200 OK", body),
            Err(_) => ("404 Not Found", "Run or database unavailable".to_string()),
        }
    } else {
        ("404 Not Found", "Not found".to_string())
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\nReferrer-Policy: no-referrer\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn request(path: &Path, message: &str) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let path = path.to_owned();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle(&mut stream, &path, port).unwrap();
        });
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        client
            .write_all(message.replace("{port}", &port.to_string()).as_bytes())
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        worker.join().unwrap();
        response
    }

    #[test]
    fn http_rejects_untrusted_host_origin_and_mutations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.sqlite");
        crate::store::Store::open(&db).unwrap();
        assert!(
            request(&db, "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                .starts_with("HTTP/1.1 200")
        );
        assert!(
            request(&db, "GET / HTTP/1.1\r\nHost: attacker.example\r\n\r\n")
                .starts_with("HTTP/1.1 403")
        );
        assert!(request(&db, "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://attacker.example\r\n\r\n").starts_with("HTTP/1.1 403"));
        assert!(
            request(&db, "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                .starts_with("HTTP/1.1 405")
        );
    }
}
