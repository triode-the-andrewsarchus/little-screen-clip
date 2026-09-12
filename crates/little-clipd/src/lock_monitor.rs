//! System sleep, session lock, and lock screen detection for ambient display blanking.
//!
//! Listens for systemd-logind `PrepareForSleep`, `Session.Lock`, `Session.Unlock`,
//! and queries Noctalia lock screen state to automatically blank the screen-saver frame.

use crate::DaemonContext;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use zbus::proxy;

/// D-Bus interface proxy for `org.freedesktop.login1.Manager`.
#[proxy(
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    interface = "org.freedesktop.login1.Manager"
)]
trait Login1Manager {
    /// Signal emitted when the system is preparing for or resuming from sleep/suspend.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// D-Bus interface proxy for `org.freedesktop.login1.Session`.
#[proxy(
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto",
    interface = "org.freedesktop.login1.Session"
)]
trait Login1Session {
    /// Signal emitted when session is locked.
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;

    /// Signal emitted when session is unlocked.
    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<()>;

    /// Property indicating whether the session is currently locked.
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

/// Spawns background tasks monitoring systemd-logind D-Bus signals and Noctalia shell lock states.
pub async fn run_lock_monitor(ctx: Arc<DaemonContext>) {
    let dbus_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        run_dbus_monitor(dbus_ctx).await;
    });

    let noctalia_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        run_noctalia_watchdog(noctalia_ctx).await;
    });
}

async fn run_dbus_monitor(ctx: Arc<DaemonContext>) {
    loop {
        match zbus::connection::Connection::system().await {
            Ok(conn) => {
                info!("Connected to D-Bus system bus for lock/sleep monitoring");
                let mgr_proxy_res = Login1ManagerProxy::new(&conn).await;
                let session_proxy_res = Login1SessionProxy::new(&conn).await;

                match (mgr_proxy_res, session_proxy_res) {
                    (Ok(mgr_proxy), Ok(session_proxy)) => {
                        let mut sleep_stream = match mgr_proxy.receive_prepare_for_sleep().await {
                            Ok(s) => Some(s),
                            Err(e) => {
                                warn!("Failed to subscribe to PrepareForSleep: {}", e);
                                None
                            }
                        };

                        let mut lock_stream = match session_proxy.receive_lock().await {
                            Ok(s) => Some(s),
                            Err(e) => {
                                warn!("Failed to subscribe to Session.Lock: {}", e);
                                None
                            }
                        };

                        let mut unlock_stream = match session_proxy.receive_unlock().await {
                            Ok(s) => Some(s),
                            Err(e) => {
                                warn!("Failed to subscribe to Session.Unlock: {}", e);
                                None
                            }
                        };

                        let mut locked_hint_stream = session_proxy.receive_locked_hint_changed().await;

                        loop {
                            tokio::select! {
                                Some(sig) = async {
                                    if let Some(ref mut s) = sleep_stream {
                                        s.next().await
                                    } else {
                                        None
                                    }
                                } => {
                                    if let Ok(args) = sig.args() {
                                        info!("D-Bus: PrepareForSleep(start={}) received", args.start);
                                        if args.start {
                                            ctx.set_locked(true).await;
                                        } else {
                                            // When resuming, restore if not locked
                                            ctx.set_locked(false).await;
                                        }
                                    }
                                }
                                Some(_sig) = async {
                                    if let Some(ref mut s) = lock_stream {
                                        s.next().await
                                    } else {
                                        None
                                    }
                                } => {
                                    info!("D-Bus: Session.Lock signal received");
                                    ctx.set_locked(true).await;
                                }
                                Some(_sig) = async {
                                    if let Some(ref mut s) = unlock_stream {
                                        s.next().await
                                    } else {
                                        None
                                    }
                                } => {
                                    info!("D-Bus: Session.Unlock signal received");
                                    ctx.set_locked(false).await;
                                }
                                Some(prop) = locked_hint_stream.next() => {
                                    if let Ok(val) = prop.get().await {
                                        info!("D-Bus: Session.LockedHint changed to {}", val);
                                        ctx.set_locked(val).await;
                                    }
                                }
                                else => {
                                    break;
                                }
                            }
                        }
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        warn!("Failed to create systemd-logind proxy: {}. Retrying in 5s...", e);
                    }
                }
            }
            Err(e) => {
                debug!("D-Bus system connection failed: {}. Retrying in 5s...", e);
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_noctalia_watchdog(ctx: Arc<DaemonContext>) {
    let mut was_noctalia_locked = false;
    let mut consecutive_failures: u32 = 0;

    loop {
        let sleep_duration = if consecutive_failures == 0 {
            Duration::from_millis(1000)
        } else {
            let backoff_secs = (1u64 << consecutive_failures.min(5)).min(30);
            Duration::from_secs(backoff_secs)
        };

        tokio::time::sleep(sleep_duration).await;

        let output = tokio::process::Command::new("qs")
            .args(["-c", "noctalia-shell", "ipc", "call", "state", "all"])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                consecutive_failures = 0;
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(is_active) = val.get("state").and_then(|s| s.get("lockScreenActive")).and_then(|b| b.as_bool()) {
                        if is_active && !was_noctalia_locked {
                            info!("Noctalia watchdog: lockScreenActive became true -> Blanking display");
                            was_noctalia_locked = true;
                            ctx.set_locked(true).await;
                        } else if !is_active && was_noctalia_locked {
                            info!("Noctalia watchdog: lockScreenActive became false -> Restoring display");
                            was_noctalia_locked = false;
                            ctx.set_locked(false).await;
                        }
                    }
                }
            }
            _ => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                if consecutive_failures == 1 {
                    debug!("Noctalia watchdog: 'qs' IPC call unavailable, backing off polling");
                }
            }
        }
    }
}
