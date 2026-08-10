//! In-process SSH backend built on russh.
//!
//! The desktop tmuxmux shells out to the `ssh` binary; Android has no such
//! binary and a sandbox that forbids spawning one, so here we speak SSH
//! ourselves. Each connection owns a dedicated OS thread running a
//! single-threaded tokio runtime. The UI thread talks to it over channels:
//!
//!   UI  --ToSsh-->  connection thread   (tokio unbounded channel)
//!   UI  <-FromSsh-  connection thread   (std mpsc, drained each egui frame)

use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::*;
use russh::ChannelMsg;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::config::Host;

/// Commands from the UI to a connection.
#[derive(Debug)]
pub enum ToSsh {
    /// Run `tmux ls` on a fresh channel and reply with `SessionList`.
    ListSessions,
    /// Open a PTY channel attached to (or creating) the named session.
    Attach { session: String, rows: u16, cols: u16 },
    /// Raw bytes typed by the user, forwarded to the attached PTY.
    Input(Vec<u8>),
    /// Terminal was resized.
    Resize { rows: u16, cols: u16 },
    /// Detach the current PTY channel (tmux session survives on the server)
    /// and return to the command-wait state.
    Detach,
    /// Tear the whole SSH connection down.
    Disconnect,
}

/// Events from a connection to the UI.
#[derive(Debug)]
pub enum FromSsh {
    Status(String),
    Error(String),
    SessionList(Vec<String>),
    Attached,
    /// Bytes from the attached PTY, to feed the vt100 parser.
    Data(Vec<u8>),
    /// The attached PTY channel ended (session detached or exited).
    Detached,
    /// The whole connection is gone.
    Closed,
}

pub struct SshConnection {
    tx: UnboundedSender<ToSsh>,
    rx: std::sync::mpsc::Receiver<FromSsh>,
}

impl SshConnection {
    pub fn connect(host: Host) -> SshConnection {
        let (to_tx, to_rx) = unbounded_channel::<ToSsh>();
        let (from_tx, from_rx) = std::sync::mpsc::channel::<FromSsh>();

        std::thread::Builder::new()
            .name(format!("ssh-{}", host.host))
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = from_tx.send(FromSsh::Error(format!("runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(run_connection(host, to_rx, from_tx.clone()));
                let _ = from_tx.send(FromSsh::Closed);
            })
            .expect("spawn ssh thread");

        SshConnection { tx: to_tx, rx: from_rx }
    }

    pub fn send(&self, cmd: ToSsh) {
        let _ = self.tx.send(cmd);
    }

    /// Non-blocking drain of pending events.
    pub fn poll(&self) -> Vec<FromSsh> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    // Trust-on-first-use: accept any host key. A personal tool talking to
    // known hosts; a real fingerprint-pinning UI is future work.
    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

async fn run_connection(
    host: Host,
    mut to_rx: UnboundedReceiver<ToSsh>,
    from_tx: std::sync::mpsc::Sender<FromSsh>,
) {
    macro_rules! emit {
        ($e:expr) => {
            if from_tx.send($e).is_err() {
                return;
            }
        };
    }

    emit!(FromSsh::Status(format!("Connecting to {}:{}…", host.host, host.port)));

    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });

    let mut handle = match client::connect(config, (host.host.as_str(), host.port), ClientHandler)
        .await
    {
        Ok(h) => h,
        Err(e) => {
            emit!(FromSsh::Error(format!("connect failed: {e}")));
            return;
        }
    };

    // Authenticate: private key if provided, otherwise password.
    let user = if host.username.is_empty() {
        "root".to_string()
    } else {
        host.username.clone()
    };

    let auth_ok = if !host.private_key.trim().is_empty() {
        let pass = if host.key_passphrase.is_empty() {
            None
        } else {
            Some(host.key_passphrase.as_str())
        };
        match decode_secret_key(&host.private_key, pass) {
            Ok(key) => {
                let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                match handle
                    .authenticate_publickey(
                        &user,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                {
                    Ok(r) => r.success(),
                    Err(e) => {
                        emit!(FromSsh::Error(format!("key auth error: {e}")));
                        false
                    }
                }
            }
            Err(e) => {
                emit!(FromSsh::Error(format!("bad private key: {e}")));
                false
            }
        }
    } else {
        match handle.authenticate_password(&user, &host.password).await {
            Ok(r) => r.success(),
            Err(e) => {
                emit!(FromSsh::Error(format!("password auth error: {e}")));
                false
            }
        }
    };

    if !auth_ok {
        emit!(FromSsh::Error("authentication failed".into()));
        return;
    }

    emit!(FromSsh::Status(format!("Connected to {}", host.display_name())));

    // When a jump command is set, tmux runs at the far end of it (on the box
    // reached by that command); otherwise tmux runs directly on `host`.
    let prefix = if host.command.trim().is_empty() {
        String::new()
    } else {
        format!("{} ", host.command.trim())
    };

    // Command loop. `tmux ls` and attaching each use their own channel; while
    // attached we run an inner loop that also services input/resize.
    loop {
        let cmd = match to_rx.recv().await {
            Some(c) => c,
            None => return, // UI dropped the sender
        };
        match cmd {
            ToSsh::ListSessions => {
                match list_sessions(&mut handle, &prefix).await {
                    Ok(list) => emit!(FromSsh::SessionList(list)),
                    Err(e) => emit!(FromSsh::Error(format!("tmux ls: {e}"))),
                }
            }
            ToSsh::Attach { session, rows, cols } => {
                if let Err(e) =
                    attach_loop(&mut handle, &prefix, &session, rows, cols, &mut to_rx, &from_tx)
                        .await
                {
                    let _ = from_tx.send(FromSsh::Error(format!("attach: {e}")));
                }
                // Whether it detached cleanly or errored, we're back to the
                // selector; the tmux session lives on the server.
                let _ = from_tx.send(FromSsh::Detached);
            }
            ToSsh::Disconnect => {
                let _ = handle
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await;
                return;
            }
            // Input/Resize/Detach outside an attach are no-ops.
            _ => {}
        }
    }
}

/// Run `tmux ls`, returning the list of session names.
async fn list_sessions(
    handle: &mut client::Handle<ClientHandler>,
    prefix: &str,
) -> Result<Vec<String>, russh::Error> {
    let mut channel = handle.channel_open_session().await?;
    // Plain `tmux ls` (not `-F '#{...}'`): the `#{}` format string doesn't
    // survive quoting across a jump-command's extra shell hop (the `#` starts
    // a comment on the far shell). We parse the "name: N windows …" lines,
    // which also lets us ignore any login banner/motd noise from the jump.
    let cmd = format!("{prefix}tmux ls");
    channel.exec(true, cmd.as_str()).await?;

    let mut out: Vec<u8> = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { ref data }) => out.extend_from_slice(data),
            Some(ChannelMsg::ExtendedData { ref data, .. }) => out.extend_from_slice(data),
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => break,
            Some(ChannelMsg::ExitStatus { .. }) => {}
            Some(_) => {}
            None => break,
        }
    }
    let text = String::from_utf8_lossy(&out);
    let sessions = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            // tmux ls lines look like "name: 3 windows (created …) …".
            let (name, rest) = l.split_once(':')?;
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            if rest.contains("windows") || rest.contains("window") {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();
    Ok(sessions)
}

/// Open a PTY channel and stream it to/from the UI until detach/EOF.
async fn attach_loop(
    handle: &mut client::Handle<ClientHandler>,
    prefix: &str,
    session: &str,
    rows: u16,
    cols: u16,
    to_rx: &mut UnboundedReceiver<ToSsh>,
    from_tx: &std::sync::mpsc::Sender<FromSsh>,
) -> Result<(), russh::Error> {
    let mut channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;

    // `new-session -A` attaches if it exists, creates otherwise — one round
    // trip, no race. `-u` forces UTF-8 so box-drawing renders.
    let escaped = session.replace('\'', "'\\''");
    let cmd = format!("{prefix}tmux -u new-session -A -s '{escaped}'");
    channel.exec(true, cmd.as_str()).await?;

    if from_tx.send(FromSsh::Attached).is_err() {
        return Ok(());
    }

    loop {
        tokio::select! {
            cmd = to_rx.recv() => {
                match cmd {
                    Some(ToSsh::Input(bytes)) => {
                        channel.data(&bytes[..]).await?;
                    }
                    Some(ToSsh::Resize { rows, cols }) => {
                        channel.window_change(cols as u32, rows as u32, 0, 0).await?;
                    }
                    Some(ToSsh::Detach) => {
                        let _ = channel.eof().await;
                        return Ok(());
                    }
                    Some(ToSsh::Disconnect) => {
                        let _ = channel.eof().await;
                        return Ok(());
                    }
                    // Ignore ListSessions/Attach while already attached.
                    Some(_) => {}
                    None => return Ok(()),
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) => {
                        if from_tx.send(FromSsh::Data(data.to_vec())).is_err() {
                            return Ok(());
                        }
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        if from_tx.send(FromSsh::Data(data.to_vec())).is_err() {
                            return Ok(());
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close)
                    | Some(ChannelMsg::ExitStatus { .. }) => return Ok(()),
                    Some(_) => {}
                    None => return Ok(()),
                }
            }
        }
    }
}
