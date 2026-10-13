//! The vault server binary: bind, serve, and stop gracefully.
//!
//! A binary that refuses to start on a bad configuration is honest; the
//! `expect`s here are that refusal, and the crate lint denies them for
//! library code where they would be silent failure modes.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("vault"));
    let bind = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "0.0.0.0:2283".to_string());

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
