#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use anyhow::{Context, Result};
use fs2::FileExt;
use photosort::{library::Library, server};
use std::{fs::OpenOptions, path::PathBuf};
fn main() {
    if let Err(error) = run() {
        eprintln!("PhotoSort: {error:#}");
        if !std::env::args().any(|arg| arg == "--scan" || arg == "--browser") {
            let _ = rfd::MessageDialog::new()
                .set_title("PhotoSort")
                .set_description(format!("Не удалось открыть приложение: {error:#}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let data = value("--data").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".photosort".into()))
            .join("PhotoSort")
    });
    std::fs::create_dir_all(&data)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("instance.lock"))?;
    lock.try_lock_exclusive()
        .context("Эта библиотека уже открыта в другом экземпляре PhotoSort")?;
    let library = Library::open(&data)?;
    if let Some(root) = value("--scan") {
        library.start(&root, false)?;
        while library.progress.lock().running {
            std::thread::sleep(std::time::Duration::from_millis(100))
        }
        let progress = library.progress.lock().clone();
        println!("{}", serde_json::to_string(&progress)?);
        library.close();
        return Ok(());
    }
    let url = server::serve(
        library.clone(),
        value("--port").and_then(|s| s.parse().ok()).unwrap_or(0),
    )?;
    println!("PhotoSort Rust: {url}");
    if args.iter().any(|s| s == "--browser") {
        loop {
            std::thread::park()
        }
    }
    #[cfg(all(windows, feature = "desktop"))]
    {
        use tao::{
            dpi::LogicalSize,
            event::{Event, WindowEvent},
            event_loop::{ControlFlow, EventLoop},
            window::WindowBuilder,
        };
        let event_loop = EventLoop::new();
        let window = WindowBuilder::new()
            .with_title("PhotoSort")
            .with_inner_size(LogicalSize::new(1440.0, 940.0))
            .with_min_inner_size(LogicalSize::new(860.0, 640.0))
            .build(&event_loop)?;
        let origin = url.clone();
        let _webview = wry::WebViewBuilder::new()
            .with_url(&url)
            .with_navigation_handler(move |target| {
                target.starts_with(&format!("{origin}/")) || target == origin
            })
            .build(&window)?;
        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::Wait;
            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } = event
            {
                library.close();
                *flow = ControlFlow::Exit
            }
        });
    }
    #[cfg(not(all(windows, feature = "desktop")))]
    {
        loop {
            std::thread::park()
        }
    }
}
