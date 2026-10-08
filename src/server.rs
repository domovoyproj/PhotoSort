use crate::{
    analysis,
    library::{Library, Settings},
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Component, Path},
    sync::Arc,
    thread,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub fn serve(library: Arc<Library>, port: u16) -> Result<String> {
    let server =
        Arc::new(Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!(e.to_string()))?);
    let address = server.server_addr().to_ip().unwrap();
    let host = address.to_string();
    let url = format!("http://{host}");
    let token = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    // Bounded request workers prevent full-resolution previews exhausting memory.
    for _ in 0..4 {
        let server = server.clone();
        let library = library.clone();
        let token = token.clone();
        let host = host.clone();
        thread::spawn(move || {
            loop {
                if library.is_closed() {
                    break;
                }
                let mut request = match server.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(Some(request)) => request,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                let mut allowed = request
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("Host") && h.value.as_str() == host);
                if *request.method() == Method::Post {
                    allowed &= request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("X-PhotoSort-Token") && h.value.as_str() == token)
                }
                if !allowed {
                    respond(
                        request,
                        serde_json::to_vec(&json!({"error":"Доступ запрещён"})).unwrap(),
                        "application/json",
                        403,
                    );
                    continue;
                }
                let result = route(&library, &token, &mut request);
                match result {
                    Ok((bytes, mime)) => respond(request, bytes, &mime, 200),
                    Err(error) => respond(
                        request,
                        serde_json::to_vec(&json!({"error":error.to_string()})).unwrap(),
                        "application/json",
                        400,
                    ),
                }
            }
        });
    }
    Ok(url)
}
fn respond(request: Request, bytes: Vec<u8>, mime: &str, status: u16) {
    let response=Response::from_data(bytes).with_status_code(StatusCode(status)).with_header(Header::from_bytes("Content-Type",mime).unwrap()).with_header(Header::from_bytes("Cache-Control","no-store").unwrap()).with_header(Header::from_bytes("X-Content-Type-Options","nosniff").unwrap()).with_header(Header::from_bytes("Content-Security-Policy","default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; frame-ancestors 'none'").unwrap());
    let _ = request.respond(response);
}
fn ids(body: &Value) -> Result<Vec<i64>> {
    let array = body
        .get("ids")
        .and_then(Value::as_array)
        .context_or("Некорректный список фотографий")?;
    if array.len() > 100000 {
        bail!("Слишком много фотографий")
    };
    array
        .iter()
        .map(|v| v.as_i64().context_or("Некорректный идентификатор"))
        .collect()
}
trait ContextOption<T> {
    fn context_or(self, text: &str) -> Result<T>;
}
impl<T> ContextOption<T> for Option<T> {
    fn context_or(self, text: &str) -> Result<T> {
        self.ok_or_else(|| anyhow::anyhow!(text.to_string()))
    }
}
fn route(library: &Arc<Library>, token: &str, request: &mut Request) -> Result<(Vec<u8>, String)> {
    let url = url::Url::parse(&format!("http://127.0.0.1{}", request.url()))?;
    let path = url.path();
    let parameters: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let get = |name: &str, default: &str| parameters.get(name).cloned().unwrap_or(default.into());
    let value = if *request.method() == Method::Get {
        match path {
            "/api/session" => {
                json!({"token":token,"version":env!("CARGO_PKG_VERSION"),"engine":"Rust"})
            }
            "/api/photos" => library.listing(
                &get("view", "all"),
                &get("search", ""),
                get("offset", "0").parse().unwrap_or(0),
                &get("sort", "date"),
            )?,
            "/api/progress" => serde_json::to_value(library.progress.lock().clone())?,
            "/api/status" => library.status()?,
            "/api/group" => library.next_group(&get("kind", "similar"))?,
            "/" => {
                return Ok((
                    include_bytes!("../photosort/web/index.html").to_vec(),
                    "text/html; charset=utf-8".into(),
                ));
            }
            "/app.js" => {
                return Ok((
                    include_bytes!("../photosort/web/app.js").to_vec(),
                    "text/javascript; charset=utf-8".into(),
                ));
            }
            "/icon.ico" => {
                return Ok((
                    include_bytes!("../packaging/photosort.ico").to_vec(),
                    "image/x-icon".into(),
                ));
            }
            "/style.css" => {
                return Ok((
                    include_bytes!("../photosort/web/style.css").to_vec(),
                    "text/css; charset=utf-8".into(),
                ));
            }
            "/update.css" => {
                return Ok((
                    include_bytes!("../photosort/web/update.css").to_vec(),
                    "text/css; charset=utf-8".into(),
                ));
            }
            "/face-worker.js" => {
                return Ok((
                    include_bytes!("../photosort/web/face-worker.js").to_vec(),
                    "text/javascript; charset=utf-8".into(),
                ));
            }
            _ if path.starts_with("/media/") => {
                let id: i64 = path[7..].parse()?;
                let bytes = if parameters.contains_key("full") {
                    let photo = library.photo(id)?;
                    analysis::preview(
                        Path::new(photo.trash.as_deref().unwrap_or(&photo.path)),
                        &library.data.join("scratch"),
                    )?
                } else {
                    library.thumbnail(id)?
                };
                return Ok((bytes, "image/jpeg".into()));
            }
            _ if path.starts_with("/vision/") => {
                let relative = Path::new(&path[8..]);
                if relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                {
                    bail!("Некорректный путь")
                };
                let exe = std::env::current_exe()?;
                let root = exe.parent().unwrap().join("vision");
                let root = if root.exists() {
                    root
                } else {
                    Path::new(".photosort/tools/vision").to_path_buf()
                };
                let mime = match relative.extension().and_then(|e| e.to_str()) {
                    Some("wasm") => "application/wasm",
                    Some("js" | "mjs") => "text/javascript",
                    _ => "application/octet-stream",
                };
                return Ok((std::fs::read(root.join(relative))?, mime.into()));
            }
            _ => bail!("Не найдено"),
        }
    } else if *request.method() == Method::Post {
        let mut bytes = Vec::new();
        request
            .as_reader()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            bail!("Запрос слишком большой")
        };
        let body: Value = serde_json::from_slice(if bytes.is_empty() { b"{}" } else { &bytes })?;
        match path {
            "/api/scan" => {
                library.start(body["path"].as_str().unwrap_or(""), false)?;
                json!({"ok":true})
            }
            "/api/resume" => {
                library.start("", true)?;
                json!({"ok":true})
            }
            "/api/stop" | "/api/pause" => {
                library.pause();
                json!({"ok":true})
            }
            "/api/pick" => {
                let folder = rfd::FileDialog::new()
                    .set_title("Выберите папку")
                    .pick_folder();
                json!({"path":folder.map(|p|p.to_string_lossy().to_string()).unwrap_or_default()})
            }
            "/api/favorite" => {
                library.favorite(&ids(&body)?, body["value"].as_bool().unwrap_or(true))?
            }
            "/api/decision" => {
                library.decision(&ids(&body)?, body["decision"].as_str().unwrap_or("skip"))?
            }
            "/api/trash" => serde_json::to_value(library.move_photos(&ids(&body)?, false)?)?,
            "/api/restore" => serde_json::to_value(library.move_photos(&ids(&body)?, true)?)?,
            "/api/undo" => serde_json::to_value(library.undo()?)?,
            "/api/choose" => serde_json::to_value(library.choose(
                &ids(&body)?,
                body["winner"].as_i64(),
                body["trash_others"].as_bool().unwrap_or(false),
            )?)?,
            "/api/duplicates/plan" => {
                library.duplicate_plan(body["preferred_folder"].as_str().unwrap_or(""))?
            }
            "/api/duplicates/apply" => {
                serde_json::to_value(library.apply_duplicates(&ids(&body)?)?)?
            }
            "/api/export" => library.export(
                &ids(&body)?,
                body["path"].as_str().unwrap_or(""),
                body["structure"].as_bool().unwrap_or(true),
            )?,
            "/api/settings" => {
                let settings: Settings = serde_json::from_value(body)?;
                library.save_settings(settings)?;
                json!({"ok":true})
            }
            "/api/cache" => library.trim_cache()?,
            "/api/eyes" => {
                library.eyes(
                    body["id"].as_i64().context_or("Нет фотографии")?,
                    body["faces"].as_i64().unwrap_or(0),
                    body["score"].as_f64().unwrap_or(0.0),
                )?;
                json!({"ok":true})
            }
            _ => bail!("Не найдено"),
        }
    } else {
        bail!("Метод не поддерживается")
    };
    Ok((
        serde_json::to_vec(&value)?,
        "application/json; charset=utf-8".into(),
    ))
}
