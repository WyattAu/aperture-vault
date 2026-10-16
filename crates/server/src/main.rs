//! The vault server binary: bind, serve, and stop gracefully.
//!
//! A binary that refuses to start on a bad configuration is honest; the
//! `expect`s here are that refusal, and the crate lint denies them for
//! library code where they would be silent failure modes.
//!
//! Usage:
//!
//! - `aperture-server [vault-root] [bind-addr]` — serve (defaults:
//!   `vault`, `0.0.0.0:2283` — the port Immich clients expect).
//! - `aperture-server add-user <vault-root> <email> <password>` — create
//!   the first user. There is no admin-signup dance; an account exists
//!   because the operator ran this.
//! - `aperture-server scan <vault-root> <directory>` — ingest a folder
//!   tree, skipping what the vault already holds by checksum.
//! - `aperture-server gc <vault-root>` — mark-sweep the objects root
//!   against the index (YP-CAS-GC-002). Operator-run, never automatic.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap_or_else(|| "serve".into());
    if first == "scan" {
        let (root, dir): (PathBuf, PathBuf) = match (args.next(), args.next()) {
            (Some(root), Some(dir)) => (root.into(), dir.into()),
            _ => {
                eprintln!("usage: aperture-server scan <vault-root> <directory>");
                std::process::exit(2);
            }
        };
        let state = aperture_server::AppState::new(&root)?;
        let index = state.lock_index()?;
        let report = aperture_pipeline::scan::scan_directory(state.store(), &index, &dir)
            .map_err(|e| format!("scan: {e}"))?;
        eprintln!(
            "scanned {} files: {} ingested, {} already present, {} failed",
            report.scanned,
            report.ingested,
            report.skipped,
            report.failed.len()
        );
        for (path, reason) in &report.failed {
            eprintln!("  failed: {}: {reason}", path.display());
        }
        return Ok(());
    }
    if first == "gc" {
        let root: PathBuf = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!("usage: aperture-server gc <vault-root>");
            std::process::exit(2);
        });
        let state = aperture_server::AppState::new(&root)?;
        let index = state.lock_index()?;
        let report =
            aperture_pipeline::gc::gc(state.store(), &index).map_err(|e| format!("gc: {e}"))?;
        eprintln!(
            "gc: removed {} objects, reclaimed {} bytes, kept {} marked",
            report.removed, report.bytes_reclaimed, report.kept
        );
        return Ok(());
    }
    if first == "add-user" {
        let (root, email, password): (PathBuf, String, String) =
            match (args.next(), args.next(), args.next()) {
                (Some(root), Some(email), Some(password)) => (root.into(), email, password),
                _ => {
                    eprintln!("usage: aperture-server add-user <vault-root> <email> <password>");
                    std::process::exit(2);
                }
            };
        let state = aperture_server::AppState::new(&root)?;
        let user = state
            .register_user(&email, &password, &email)
            .map_err(|e| format!("register: {e}"))?;
        eprintln!("user {} created ({})", user.username, user.id);
        return Ok(());
    }
    let root = PathBuf::from(first);
    let bind = args.next().unwrap_or_else(|| "0.0.0.0:2283".to_string());

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let state = aperture_server::AppState::new(&root)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        let app = aperture_server::build_router(state);
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        eprintln!("aperture-vault listening on {bind}");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        Ok(())
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
