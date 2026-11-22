use std::sync::Arc;

use sentinel_server::state::{App, Config};

#[tokio::main]
async fn main() {
    let mut port = 7710u16;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" {
            if let Some(v) = args.next() {
                port = v.parse().unwrap_or(port);
            }
        }
    }
    let paths = match sentinel_toc::Paths::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let web = std::env::var_os("SENTINEL_WEB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web"));
    let cfg = Config { paths, web, port };
    let app: Arc<App> = App::new(cfg.clone());
    let router = sentinel_server::router(app);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    println!("SentinelVI on http://{addr}");
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
