use anyhow::Result;
use axum::Router;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;

use super::api;

pub async fn start_server(port: u16) -> Result<()> {
    let app = Router::new()
        .merge(api::routes())
        .layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("recall web server running at http://{}", addr);

    // Try to open the browser
    let url = format!("http://127.0.0.1:{}", port);
    let _ = std::process::Command::new("open").arg(&url).spawn();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
