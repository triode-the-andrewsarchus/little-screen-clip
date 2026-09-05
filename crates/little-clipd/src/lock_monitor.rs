use crate::DaemonContext;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use zbus::proxy;

#[proxy(
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    interface = "org.freedesktop.login1.Manager"
)]
trait Login1Manager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[proxy(
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto",
    interface = "org.freedesktop.login1.Session"
)]
trait Login1Session {
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

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
    let mut interval = tokio::time::interval(Duration::from_millis(1000));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut was_noctalia_locked = false;

    loop {
        interval.tick().await;

        let output = tokio::process::Command::new("qs")
            .args(["-c", "noctalia-shell", "ipc", "call", "state", "all"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
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
        }
    }
}
