//! The apexide daemon: owns the keyboard, runs software effects and the OLED,
//! and serves an HTTP/JSON API over a Unix socket (and optionally TCP).
//!
//! GUI apps can embed it with [`serve`], or drive a running daemon over the
//! socket like the CLI does.

pub mod api;
pub mod effects;
pub mod engine;
pub mod screen;
pub mod state;
pub mod store;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, bail};
use tokio::net::{TcpListener, UnixListener, UnixStream};

use crate::{api::App, engine::Engine, state::Shared, store::Store};

/// The release version (`0.1.7`), set by CI through `APEXIDE_VERSION` at build
/// time. Local builds report the workspace version with a `-dev` suffix, which
/// sorts below every release of that version.
pub const VERSION: &str = match option_env!("APEXIDE_VERSION") {
    Some(v) => v,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

pub struct ServeOptions {
    pub socket: PathBuf,
    /// Also listen on this TCP address (keep it on localhost).
    pub tcp: Option<SocketAddr>,
    pub config_dir: PathBuf,
}

/// Default socket path: `$XDG_RUNTIME_DIR/apexide.sock`.
pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("apexide.sock")
}

/// Run the daemon until SIGINT/SIGTERM, then restore onboard lighting.
pub async fn serve(opts: ServeOptions) -> anyhow::Result<()> {
    if opts.socket.exists() {
        if UnixStream::connect(&opts.socket).await.is_ok() {
            bail!(
                "another apexide daemon is already listening on {}",
                opts.socket.display()
            );
        }
        std::fs::remove_file(&opts.socket).ok();
    }

    let store = Store::new(opts.config_dir);
    let settings = store.load_state().context("loading saved state")?;
    let shared = Arc::new(Mutex::new(Shared {
        settings,
        ..Default::default()
    }));
    let engine = Engine::spawn(shared.clone());
    let router = api::router(App { shared, store });

    let unix = UnixListener::bind(&opts.socket)
        .with_context(|| format!("binding {}", opts.socket.display()))?;
    tracing::info!("listening on {}", opts.socket.display());
    let mut tasks = tokio::task::JoinSet::new();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(());
    {
        let router = router.clone();
        let mut rx = stop_rx.clone();
        tasks.spawn(async move {
            axum::serve(unix, router)
                .with_graceful_shutdown(async move {
                    rx.changed().await.ok();
                })
                .await
        });
    }
    if let Some(addr) = opts.tcp {
        let tcp = TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        tracing::info!("listening on http://{addr}");
        let mut rx = stop_rx.clone();
        tasks.spawn(async move {
            axum::serve(tcp, router)
                .with_graceful_shutdown(async move {
                    rx.changed().await.ok();
                })
                .await
        });
    }

    shutdown_signal().await;
    tracing::info!("shutting down");
    stop_tx.send(()).ok();
    while tasks.join_next().await.is_some() {}
    tokio::task::spawn_blocking(move || engine.shutdown()).await?;
    std::fs::remove_file(&opts.socket).ok();
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
