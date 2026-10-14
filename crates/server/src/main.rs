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

#![allow(clippy::expect_used)]

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap_or_else(|| "serve".into());
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
