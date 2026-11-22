use std::path::PathBuf;

use clap::{Parser, Subcommand};
use sentinel_toc::{human_size, Paths, Toc};

#[derive(Parser)]
#[command(name = "sentinel", about = "SentinelVI archive inspector", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Data paths, archive sizes and index state.
    Info,
    /// Directory and name index.
    Toc {
        #[command(subcommand)]
        sub: TocCmd,
    },
    /// Export one carved payload to disk.
    Export {
        id: i64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Start the local server and open the browser.
    Ui {
        #[arg(long, default_value_t = 7710)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// Build a derived index.
    Index {
        #[command(subcommand)]
        sub: IndexCmd,
    },
    /// Texture databases.
    Textures {
        #[command(subcommand)]
        sub: TextureCmd,
    },
    /// Animation clips.
    Anim {
        #[command(subcommand)]
        sub: AnimCmd,
    },
    /// Audio banks.
    Audio {
        #[command(subcommand)]
        sub: AudioCmd,
    },
}

#[derive(Subcommand)]
enum TocCmd {
    Stats,
    Search {
        q: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Every nested archive header, grouped by top-level archive.
    Archives,
}

#[derive(Subcommand)]
enum IndexCmd {
    Placement,
    Anim,
    Audio,
}

#[derive(Subcommand)]
enum TextureCmd {
    List,
}

#[derive(Subcommand)]
enum AnimCmd {
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

#[derive(Subcommand)]
enum AudioCmd {
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

fn open_browser(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
}

fn progress(name: &str) -> impl Fn(u64, u64) + Sync + '_ {
    let last = std::sync::Mutex::new(0u64);
    move |done: u64, total: u64| {
        let mut l = last.lock().unwrap();
        if done == total || done.saturating_sub(*l) >= total / 20 || *l == 0 {
            *l = done;
            if total > 0 {
                println!("{name}: {done}/{total}");
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let paths = match Paths::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = run(cli, &paths) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, paths: &Paths) -> Result<(), String> {
    match cli.cmd {
        Cmd::Info => {
            let info = sentinel_toc::info(paths);
            println!("{} {}", info.product, info.version);
            println!("data root   {}", info.data_root);
            println!("game dir    {}", info.game_dir);
            for (name, size, present) in &info.archives {
                println!(
                    "{:<14} {:>10}  {}",
                    name,
                    size,
                    if *present { "present" } else { "missing" }
                );
            }
            for (kind, name) in [
                ("placement", "placement.sqlite"),
                ("anim", "anim.sqlite"),
                ("audio", "audio.sqlite"),
            ] {
                let p = paths.cache(name);
                match std::fs::metadata(&p) {
                    Ok(m) => println!("{kind:<14} {:>10}  {}", human_size(m.len() as i64), p.display()),
                    Err(_) => println!("{kind:<14} {:>10}  not built", "-"),
                }
            }
            Ok(())
        }
        Cmd::Toc { sub } => {
            let t = Toc::open(&paths.toc_db())?;
            match sub {
                TocCmd::Stats => {
                    let s = t.stats()?;
                    println!("== toc stats ==");
                    println!("streams                {}", s.streams);
                    println!("named                  {}", s.named);
                    println!("with extension         {}", s.with_extension);
                    println!("unique inferred names  {}", s.unique_names);
                    println!("unique folders         {}", s.folders);
                    println!("nested RPF9 streams    {}", s.nested);
                    println!("containing packs       {}", s.packs);
                    println!("name pool tokens       {}", s.pool);
                    println!();
                    println!("confidence");
                    for (k, n) in &s.by_confidence {
                        println!("  {:<12} {:>10}", k, n);
                    }
                    println!();
                    println!("archive");
                    for (k, n) in &s.by_archive {
                        println!("  {:<14} {:>10}", k, n);
                    }
                    Ok(())
                }
                TocCmd::Search { q, limit } => {
                    let (rows, total) = t.search(&q, limit)?;
                    println!("{total} names match");
                    for s in rows {
                        println!(
                            "{:>9}  {:<12}  {:<8}  {}",
                            s.id,
                            s.archive,
                            s.confidence,
                            s.name.unwrap_or_default()
                        );
                    }
                    Ok(())
                }
                TocCmd::Archives => {
                    for a in t.archives()? {
                        println!(
                            "{:<14} packs {:>5}  streams {:>9}  named {:>9}",
                            a.archive, a.packs, a.streams, a.named
                        );
                    }
                    Ok(())
                }
            }
        }
        Cmd::Export { id, out } => {
            let t = Toc::open(&paths.toc_db())?;
            let s = t
                .stream(id)?
                .ok_or_else(|| format!("no stream {id}"))?;
            let store = sentinel_toc::BinStore::open(&paths.bin_store())?;
            let bytes = store.payload(&s.archive, s.stream_offset)?;
            let name = sentinel_toc::export_name(&s);
            let dir = out.unwrap_or_else(|| paths.cache_dir().join("exports"));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(&name);
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            println!("{}  {} B", path.display(), bytes.len());
            Ok(())
        }
        Cmd::Ui { port, no_open } => {
            let web = web_dir()?;
            let cfg = sentinel_server::state::Config {
                paths: paths.clone(),
                web,
                port,
            };
            let url = format!("http://127.0.0.1:{port}");
            if !no_open {
                let u = url.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    open_browser(&u);
                });
            }
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(8)
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(async move {
                    let app = sentinel_server::state::App::new(cfg);
                    let router = sentinel_server::router(app);
                    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                    let listener = tokio::net::TcpListener::bind(addr)
                        .await
                        .map_err(|e| format!("bind {addr}: {e}"))?;
                    let bound = listener.local_addr().map_err(|e| e.to_string())?;
                    println!("listening on http://{bound}");
                    std::io::Write::flush(&mut std::io::stdout()).ok();
                    axum::serve(listener, router)
                        .await
                        .map_err(|e| e.to_string())
                })
        }
        Cmd::Index { sub } => {
            std::fs::create_dir_all(paths.cache_dir()).map_err(|e| e.to_string())?;
            match sub {
                IndexCmd::Placement => {
                    let out = paths.cache("placement.sqlite");
                    let r = sentinel_placement::build(
                        &paths.archive("levels"),
                        &out,
                        &progress("placement"),
                    )?;
                    println!(
                        "{} resources, {} entities stored of {} read, {} props stored of {} read, {} interior points, {:.0}s",
                        r.resources, r.entities_kept, r.entities, r.props_kept, r.props, r.interiors, r.seconds
                    );
                    println!("{}", out.display());
                }
                IndexCmd::Anim => {
                    let out = paths.cache("anim.sqlite");
                    let r = sentinel_anim::build(
                        &paths.archive("anim"),
                        &out,
                        &progress("animation"),
                    )?;
                    println!(
                        "{} clip records, {} named, {} resources, {:.0}s",
                        r.records, r.named, r.resources, r.seconds
                    );
                    println!("{}", out.display());
                }
                IndexCmd::Audio => {
                    let out = paths.cache("audio.sqlite");
                    let n = sentinel_server::audio::build(
                        &paths.archive("audio"),
                        &out,
                        &progress("audio"),
                    )?;
                    println!("{n} banks");
                    println!("{}", out.display());
                }
            }
            Ok(())
        }
        Cmd::Textures { sub } => match sub {
            TextureCmd::List => {
                let game = &paths.game;
                let mut n = 0;
                for (rel, size) in sentinel_server::state::ttpfd_files(game) {
                    let path = game.join(&rel);
                    let src = sentinel_core::source::MmapSource::open(&path, 3 << 30)
                        .map_err(|e| e.to_string())?;
                    match sentinel_core::tpfd::Rds5::parse(src.as_ref()) {
                        Ok(rds) => {
                            println!(
                                "{:<44} {:>10}  {:>6} textures",
                                rel,
                                human_size(size as i64),
                                rds.descs.len()
                            );
                            n += 1;
                        }
                        Err(e) => println!("{:<44} {:>10}  {e}", rel, human_size(size as i64)),
                    }
                }
                if n == 0 {
                    println!("no .ttpf databases under {}", game.display());
                }
                Ok(())
            }
        },
        Cmd::Anim { sub } => match sub {
            AnimCmd::List { limit } => {
                let path = paths.cache("anim.sqlite");
                if !path.is_file() {
                    return Err(format!(
                        "animation index not built: run `sentinel index anim` ({})",
                        path.display()
                    ));
                }
                let conn = sentinel_anim::open(&path)?;
                let (rows, total) = sentinel_anim::list(&conn, None, 0, limit)?;
                println!("{total} clips");
                for c in rows {
                    println!(
                        "{:>9}  {:<48} {:>6} frames  {:>7.2}s",
                        c.id,
                        c.name.unwrap_or_else(|| "-".into()),
                        c.frames,
                        c.duration
                    );
                }
                Ok(())
            }
        },
        Cmd::Audio { sub } => match sub {
            AudioCmd::List { limit } => {
                let path = paths.cache("audio.sqlite");
                if !path.is_file() {
                    return Err(format!(
                        "audio index not built: run `sentinel index audio` ({})",
                        path.display()
                    ));
                }
                let conn = sentinel_server::audio::open(&path)?;
                let (rows, total) = sentinel_server::audio::list(&conn, 0, limit)?;
                println!("{total} banks");
                for b in rows {
                    println!(
                        "0x{:X}  {:>9} B  {:>3} streams  {:>6} Hz  codec {}",
                        b.offset, b.size, b.streams, b.rate, b.codec
                    );
                }
                Ok(())
            }
        },
    }
}

fn web_dir() -> Result<PathBuf, String> {
    if let Some(v) = std::env::var_os("SENTINEL_WEB") {
        let p = PathBuf::from(v);
        if p.join("index.html").is_file() {
            return Ok(p);
        }
    }
    let here = std::env::current_exe().unwrap_or_default();
    let mut p = here.clone();
    for _ in 0..4 {
        p.pop();
    }
    let mut candidates = vec![p.join("web"), here.parent().unwrap_or(&here).join("web")];
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web"));
    for c in candidates {
        if c.join("index.html").is_file() {
            return Ok(c);
        }
    }
    Err("web directory not found; set SENTINEL_WEB".into())
}


