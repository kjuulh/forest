//! Thin wrappers around system `ssh` and `rsync`. Using the binaries directly
//! is simpler than embedding an SSH client and lets users rely on their
//! existing `~/.ssh/config`.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};

use crate::config::Config;

/// Run a remote shell command. Returns combined stdout (stderr is inherited
/// to the dev-side terminal so users can see remote bootstrap progress).
pub fn run_remote(cfg: &Config, script: &str) -> anyhow::Result<String> {
    let mut cmd = Command::new("ssh");
    if let Some(key) = &cfg.ssh_key {
        cmd.arg("-i").arg(key);
    }
    cmd.arg(&cfg.host).arg(script);
    cmd.stderr(Stdio::inherit());
    let output = cmd.output().context("spawn ssh")?;
    if !output.status.success() {
        bail!(
            "ssh `{}` failed with status {}",
            preview(script),
            output.status,
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a remote command, piping the given bytes to its stdin, invoking
/// `on_stdout_line` for every line received on stdout while the command is
/// running, and returning the full stdout/stderr at the end. The per-line
/// callback is what lets the harness tee event streams to the terminal so
/// test output isn't a silent black box.
pub fn run_remote_streaming(
    cfg: &Config,
    command: &str,
    stdin_bytes: &[u8],
    mut on_stdout_line: impl FnMut(&str),
) -> anyhow::Result<RemoteOutput> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::thread;

    let mut cmd = Command::new("ssh");
    if let Some(key) = &cfg.ssh_key {
        cmd.arg("-i").arg(key);
    }
    cmd.arg(&cfg.host).arg(command);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawn ssh")?;

    // Feed the spec to stdin in full, then close so ssh doesn't wait for more.
    // Spec payloads are small (a few KB at most), so a blocking write is fine —
    // no risk of filling the pipe buffer while the remote is stalled.
    child
        .stdin
        .as_mut()
        .context("ssh stdin")?
        .write_all(stdin_bytes)
        .context("write to ssh stdin")?;
    drop(child.stdin.take());

    // Drain stderr on a helper thread so it can't backpressure the remote.
    let mut stderr_pipe = child.stderr.take().context("ssh stderr")?;
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let stdout_pipe = child.stdout.take().context("ssh stdout")?;
    let reader = BufReader::new(stdout_pipe);
    let mut accumulated = Vec::new();
    for line in reader.lines() {
        let line = line.context("read ssh stdout")?;
        on_stdout_line(&line);
        accumulated.extend_from_slice(line.as_bytes());
        accumulated.push(b'\n');
    }

    let status = child.wait().context("wait ssh")?;
    let stderr = stderr_handle.join().unwrap_or_default();

    Ok(RemoteOutput {
        success: status.success(),
        stdout: accumulated,
        stderr,
    })
}

pub struct RemoteOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// rsync `local` → `<host>:<remote>`. Does NOT trail-slash-translate, so pass
/// `local` exactly how rsync expects it.
pub fn rsync_to(cfg: &Config, local: &Path, remote: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("rsync");
    cmd.arg("-az").arg("--info=progress2");
    if let Some(key) = &cfg.ssh_key {
        cmd.arg("-e").arg(format!("ssh -i {}", key.display()));
    }
    cmd.arg(local).arg(format!("{}:{}", cfg.host, remote));
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd.status().context("spawn rsync")?;
    if !status.success() {
        bail!("rsync to {} failed", remote);
    }
    Ok(())
}

fn preview(s: &str) -> String {
    let one_line: String = s.chars().take(80).collect();
    one_line.replace('\n', " ⏎ ")
}
