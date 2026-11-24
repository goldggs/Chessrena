//! Manages an account-free Cloudflare Quick Tunnel process for the local agent API.
//!
//! The tunnel URL is public and temporary. Seat tokens remain the authorization boundary.

use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[path = "tunnel_install.rs"]
mod tunnel_install;

#[derive(Clone, Debug, Default)]
pub(crate) struct TunnelStatus {
    pub(crate) url: Option<String>,
    pub(crate) message: String,
}

pub(crate) struct TunnelController {
    status: Arc<Mutex<TunnelStatus>>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TunnelController {
    pub(crate) fn start() -> Self {
        let status = Arc::new(Mutex::new(TunnelStatus {
            url: None,
            message: "Starting temporary secure link…".to_owned(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let worker = spawn_worker(&status, &stop, &child, false);
        Self {
            status,
            stop,
            child,
            worker,
        }
    }

    pub(crate) fn retry(&mut self) {
        if self
            .worker
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
        {
            self.stop.store(false, Ordering::Relaxed);
            set_status(&self.status, None, "Retrying temporary secure link…");
            self.worker = spawn_worker(&self.status, &self.stop, &self.child, true);
        }
    }

    pub(crate) fn status(&self) -> TunnelStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| TunnelStatus {
                url: None,
                message: "Tunnel status is unavailable.".to_owned(),
            })
    }
}

impl Drop for TunnelController {
    fn drop(&mut self) {
        // Signal the owner thread to terminate cloudflared; it checks within 150 ms.
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut process) = child.take() {
                terminate(&mut process);
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn spawn_worker(
    status: &Arc<Mutex<TunnelStatus>>,
    stop: &Arc<AtomicBool>,
    child: &Arc<Mutex<Option<Child>>>,
    download_if_missing: bool,
) -> Option<thread::JoinHandle<()>> {
    let worker_status = Arc::clone(status);
    let worker_stop = Arc::clone(stop);
    let worker_child = Arc::clone(child);
    let worker = thread::Builder::new()
        .name("chessrena-quick-tunnel".to_owned())
        .spawn(move || {
            run_tunnel(
                worker_status,
                worker_stop,
                worker_child,
                download_if_missing,
            )
        })
        .ok();
    if worker.is_none() {
        set_status(status, None, "Could not start the tunnel manager thread.");
    }
    worker
}

fn run_tunnel(
    status: Arc<Mutex<TunnelStatus>>,
    stop: Arc<AtomicBool>,
    child_handle: Arc<Mutex<Option<Child>>>,
    download_if_missing: bool,
) {
    let executable = match find_cloudflared() {
        Some(executable) => executable,
        None if download_if_missing => {
            set_status(
                &status,
                None,
                "Downloading the official cloudflared executable and verifying its SHA-256…",
            );
            match tunnel_install::download() {
                Ok(path) => path,
                Err(error) => {
                    set_status(&status, None, &error);
                    return;
                }
            }
        }
        None => {
            set_status(
                &status,
                None,
                "cloudflared is missing. Retry to download the verified official executable, or install it yourself.",
            );
            return;
        }
    };
    let mut child = match Command::new(executable)
        .args([
            "tunnel",
            "--no-autoupdate",
            "--url",
            "http://127.0.0.1:18764",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            set_status(
                &status,
                None,
                &format!("Could not launch cloudflared: {error}"),
            );
            return;
        }
    };

    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    if let Some(stream) = child.stdout.take() {
        spawn_log_reader(stream, line_tx.clone());
    }
    if let Some(stream) = child.stderr.take() {
        spawn_log_reader(stream, line_tx.clone());
    }
    drop(line_tx);
    match child_handle.lock() {
        Ok(mut slot) => *slot = Some(child),
        Err(_) => {
            terminate(&mut child);
            set_status(&status, None, "Tunnel process tracking failed.");
            return;
        }
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            if let Some(mut process) = take_child(&child_handle) {
                terminate(&mut process);
            }
            set_status(&status, None, "Temporary link stopped.");
            return;
        }
        while let Ok(line) = line_rx.try_recv() {
            if let Some(url) = extract_quick_tunnel_url(&line) {
                set_status(&status, Some(url), "Temporary HTTPS link is live.");
            }
        }
        let result = child_handle
            .lock()
            .map_err(|_| "Tunnel process tracking failed".to_owned())
            .and_then(|mut slot| {
                slot.as_mut()
                    .ok_or_else(|| "Tunnel process is no longer available".to_owned())?
                    .try_wait()
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(Some(exit)) => {
                let _ = take_child(&child_handle);
                let message = if exit.success() {
                    "The temporary link stopped. Use Retry secure link to create another."
                } else {
                    "cloudflared exited unexpectedly. Check network access, then retry the link."
                };
                set_status(&status, None, message);
                return;
            }
            Err(error) => {
                set_status(
                    &status,
                    None,
                    &format!("Could not monitor cloudflared: {error}"),
                );
                if let Some(mut process) = take_child(&child_handle) {
                    terminate(&mut process);
                }
                return;
            }
            Ok(None) => {}
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn spawn_log_reader<R: Read + Send + 'static>(stream: R, tx: std::sync::mpsc::Sender<String>) {
    let _ = thread::Builder::new()
        .name("chessrena-tunnel-log".to_owned())
        .spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
}

fn take_child(child: &Arc<Mutex<Option<Child>>>) -> Option<Child> {
    child.lock().ok()?.take()
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn set_status(status: &Arc<Mutex<TunnelStatus>>, url: Option<String>, message: &str) {
    if let Ok(mut current) = status.lock() {
        current.url = url;
        current.message = message.to_owned();
    }
}

fn find_cloudflared() -> Option<PathBuf> {
    let executable_name = if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    if let Some(local_data) = std::env::var_os("LOCALAPPDATA") {
        let bundled = PathBuf::from(local_data)
            .join("Chessrena")
            .join(executable_name);
        if bundled.is_file() && tunnel_install::is_verified(&bundled) {
            return Some(bundled);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(executable_name))
        .find(|candidate| candidate.is_file())
}

fn extract_quick_tunnel_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .map(|part| part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && !"-._:/".contains(c)))
        .find(|part| {
            part.starts_with("https://")
                && part.ends_with(".trycloudflare.com")
                && part[8..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::extract_quick_tunnel_url;

    #[test]
    fn accepts_only_https_quick_tunnel_hostnames() {
        assert_eq!(
            extract_quick_tunnel_url("INF | https://quiet-tree.trycloudflare.com"),
            Some("https://quiet-tree.trycloudflare.com".to_owned())
        );
        assert_eq!(
            extract_quick_tunnel_url("https://example.com is ready"),
            None
        );
        assert_eq!(
            extract_quick_tunnel_url("http://quiet-tree.trycloudflare.com"),
            None
        );
        assert_eq!(
            extract_quick_tunnel_url("https://quiet-tree.trycloudflare.com.evil.test"),
            None
        );
    }
}
