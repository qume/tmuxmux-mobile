//! Headless end-to-end test of the SSH backend against a real sshd+tmux.
//! Usage: cargo run --example ssh_test -- <host> <user> [key_path]
//! Defaults: 127.0.0.1, $USER, ~/.ssh/id_rsa

use std::time::{Duration, Instant};

use tmuxmux_mobile::config::Host;
use tmuxmux_mobile::ssh::{FromSsh, SshConnection, ToSsh};

fn drain(conn: &SshConnection, secs: u64, tag: &str) -> Vec<u8> {
    let mut data = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        for ev in conn.poll() {
            match ev {
                FromSsh::Status(s) => println!("[{tag}] status: {s}"),
                FromSsh::Error(e) => println!("[{tag}] ERROR: {e}"),
                FromSsh::SessionList(l) => println!("[{tag}] sessions: {l:?}"),
                FromSsh::Attached => println!("[{tag}] ATTACHED"),
                FromSsh::Detached => println!("[{tag}] detached"),
                FromSsh::Closed => println!("[{tag}] closed"),
                FromSsh::Data(b) => data.extend_from_slice(&b),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    data
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `ssh_test <config.json>`: load the first host from a Config JSON (used to
    // test the jump-command path against a real fleet). Lists sessions, then
    // attaches to the first one found.
    if let Some(path) = args.first().filter(|a| a.ends_with(".json")) {
        let s = std::fs::read_to_string(path).expect("read config");
        let cfg: tmuxmux_mobile::config::Config = serde_json::from_str(&s).expect("parse config");
        let h = cfg.hosts.into_iter().next().expect("no hosts");
        println!("jump test: host={} command={:?}", h.host, if h.command.is_empty() { "<none>" } else { "<set>" });
        let conn = SshConnection::connect(h);
        conn.send(ToSsh::ListSessions);
        let mut sessions = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(12);
        'outer: while Instant::now() < deadline {
            for ev in conn.poll() {
                match ev {
                    FromSsh::Status(s) => println!("[list] {s}"),
                    FromSsh::Error(e) => println!("[list] ERROR: {e}"),
                    FromSsh::SessionList(l) => { println!("[list] sessions: {l:?}"); sessions = l; break 'outer; }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let target = sessions.first().cloned().unwrap_or_else(|| "main".into());
        println!("--- attaching to '{target}' ---");
        conn.send(ToSsh::Attach { session: target, rows: 24, cols: 80 });
        let out = drain(&conn, 4, "attach");
        println!("--- screen bytes: {} ---", out.len());
        let text = String::from_utf8_lossy(&out);
        for line in text.lines().take(8) { println!("| {line}"); }
        conn.send(ToSsh::Disconnect);
        std::process::exit(if out.is_empty() { 1 } else { 0 });
    }

    let host = args.first().cloned().unwrap_or_else(|| "127.0.0.1".into());
    let user = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".into()));
    let key_path = args.get(2).cloned().unwrap_or_else(|| {
        format!("{}/.ssh/id_rsa", std::env::var("HOME").unwrap_or_default())
    });
    let private_key = std::fs::read_to_string(&key_path).unwrap_or_default();
    println!("connecting {user}@{host} with key {key_path} ({} bytes)", private_key.len());

    let h = Host {
        host,
        port: 22,
        username: user,
        private_key,
        ..Default::default()
    };

    let conn = SshConnection::connect(h);
    conn.send(ToSsh::ListSessions);
    drain(&conn, 3, "list");

    println!("--- attaching to 'mobiletest' (creates if missing) ---");
    conn.send(ToSsh::Attach {
        session: "mobiletest".into(),
        rows: 24,
        cols: 80,
    });
    drain(&conn, 2, "attach");

    // Type a command into the session.
    conn.send(ToSsh::Input(b"echo HELLO_FROM_MOBILE_$((6*7))\n".to_vec()));
    let out = drain(&conn, 2, "io");

    let text = String::from_utf8_lossy(&out);
    println!("--- rendered screen bytes: {} ---", out.len());
    let ok = text.contains("HELLO_FROM_MOBILE_42");
    println!("contains expected command echo/output: {ok}");

    conn.send(ToSsh::Detach);
    drain(&conn, 1, "detach");
    conn.send(ToSsh::Disconnect);

    println!("\nRESULT: {}", if ok { "PASS" } else { "FAIL (see output above)" });
    std::process::exit(if ok { 0 } else { 1 });
}
