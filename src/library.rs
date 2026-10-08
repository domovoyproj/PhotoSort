use crate::analysis::{self, Description, HammingIndex};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use rayon::prelude::*;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ACTIVE: &str = "trash IS NULL AND missing=0";
const COLUMNS: &str = "id,path,root,name,size,mtime,width,height,captured,hash,dhash,sharpness,favorite,trash,missing,similar,burst,decision,reviewed,quality_hint,faces,eye_score,asset_group,brightness,media_kind,duration,fps,video_codec";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Photo {
    pub id: i64,
    pub path: String,
    pub root: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub width: u32,
    pub height: u32,
    pub captured: Option<f64>,
    pub hash: String,
    pub dhash: String,
    pub sharpness: f64,
    pub favorite: bool,
    pub trash: Option<String>,
    pub missing: bool,
    pub similar: Option<i64>,
    pub burst: Option<i64>,
    pub decision: String,
    pub reviewed: bool,
    pub quality_hint: String,
    pub faces: Option<i64>,
    pub eye_score: Option<f64>,
    pub asset_group: String,
    pub brightness: f64,
    pub media_kind: String,
    pub duration: f64,
    pub fps: f64,
    pub video_codec: String,
}
fn photo_row(r: &Row) -> rusqlite::Result<Photo> {
    Ok(Photo {
        id: r.get(0)?,
        path: r.get(1)?,
        root: r.get(2)?,
        name: r.get(3)?,
        size: r.get(4)?,
        mtime: r.get(5)?,
        width: r.get(6)?,
        height: r.get(7)?,
        captured: r.get(8)?,
        hash: r.get(9)?,
        dhash: r.get(10)?,
        sharpness: r.get(11)?,
        favorite: r.get(12)?,
        trash: r.get(13)?,
        missing: r.get(14)?,
        similar: r.get(15)?,
        burst: r.get(16)?,
        decision: r.get(17)?,
        reviewed: r.get(18)?,
        quality_hint: r.get(19)?,
        faces: r.get(20)?,
        eye_score: r.get(21)?,
        asset_group: r.get(22)?,
        brightness: r.get(23)?,
        media_kind: r.get(24)?,
        duration: r.get(25)?,
        fps: r.get(26)?,
        video_codec: r.get(27)?,
    })
}
#[derive(Clone, Default, Serialize)]
pub struct Progress {
    pub running: bool,
    pub paused: bool,
    pub done: i64,
    pub total: i64,
    pub errors: i64,
    pub message: String,
    pub last_error: String,
    pub job: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub sensitivity: u32,
    pub cache_mb: u64,
    pub preferred_folder: String,
    pub theme: String,
    pub density: String,
    pub ui_state: Value,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 6,
            cache_mb: 1024,
            preferred_folder: String::new(),
            theme: "light".into(),
            density: "normal".into(),
            ui_state: json!({}),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    id: i64,
    favorite: bool,
    decision: String,
    reviewed: bool,
    trash: Option<String>,
}
impl From<&Photo> for Snapshot {
    fn from(p: &Photo) -> Self {
        Self {
            id: p.id,
            favorite: p.favorite,
            decision: p.decision.clone(),
            reviewed: p.reviewed,
            trash: p.trash.clone(),
        }
    }
}
#[derive(Default, Serialize)]
pub struct Outcome {
    pub done: Vec<i64>,
    pub errors: Vec<Value>,
    pub action: Option<i64>,
}
pub struct Library {
    pub data: PathBuf,
    pub thumbs: PathBuf,
    db: Mutex<Connection>,
    mutation: Mutex<()>,
    pub progress: Mutex<Progress>,
    pause: AtomicBool,
    shutdown: AtomicBool,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    version: AtomicU64,
    cache_writes: AtomicU64,
    cache_trim: Mutex<()>,
    video_preview: Mutex<()>,
    counts: Mutex<(u64, Value)>,
}

impl Library {
    pub fn open(data: &Path) -> Result<Arc<Self>> {
        fs::create_dir_all(data)?;
        let data = canonical_path(data)?;
        let thumbs = data.join("thumbnails");
        fs::create_dir_all(&thumbs)?;
        fs::create_dir_all(data.join("scratch"))?;
        let db = Connection::open(data.join("library.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS photos(id INTEGER PRIMARY KEY,path TEXT UNIQUE NOT NULL,root TEXT NOT NULL,name TEXT,size INTEGER,mtime INTEGER,width INTEGER,height INTEGER,captured REAL,hash TEXT,dhash TEXT,sharpness REAL,favorite INTEGER DEFAULT 0,trash TEXT,missing INTEGER DEFAULT 0,similar INTEGER,burst INTEGER,seen TEXT);
          CREATE TABLE IF NOT EXISTS operations(id INTEGER PRIMARY KEY,photo INTEGER,source TEXT,destination TEXT,kind TEXT,state TEXT DEFAULT 'pending');
          CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS jobs(id INTEGER PRIMARY KEY,root TEXT,generation TEXT,state TEXT,done INTEGER DEFAULT 0,total INTEGER DEFAULT 0,errors INTEGER DEFAULT 0,enumerated INTEGER DEFAULT 0);
          CREATE TABLE IF NOT EXISTS scan_queue(job INTEGER,path TEXT,state TEXT DEFAULT 'pending',error TEXT,PRIMARY KEY(job,path));
          CREATE INDEX IF NOT EXISTS queue_pending ON scan_queue(job,state);
          CREATE TABLE IF NOT EXISTS actions(id INTEGER PRIMARY KEY,label TEXT,payload TEXT,state TEXT DEFAULT 'done');")?;
        let columns: HashSet<String> = db
            .prepare("PRAGMA table_info(photos)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        for (name, definition) in [
            ("ahash", "TEXT DEFAULT ''"),
            ("crop_hash", "TEXT DEFAULT ''"),
            ("brightness", "REAL DEFAULT 128"),
            ("quality_hint", "TEXT DEFAULT ''"),
            ("decision", "TEXT DEFAULT 'pending'"),
            ("reviewed", "INTEGER DEFAULT 0"),
            ("faces", "INTEGER"),
            ("eye_score", "REAL"),
            ("asset_group", "TEXT DEFAULT ''"),
            ("media_kind", "TEXT NOT NULL DEFAULT 'photo'"),
            ("duration", "REAL NOT NULL DEFAULT 0"),
            ("fps", "REAL NOT NULL DEFAULT 0"),
            ("video_codec", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !columns.contains(name) {
                db.execute_batch(&format!(
                    "ALTER TABLE photos ADD COLUMN {name} {definition}"
                ))?
            }
        }
        db.execute_batch("CREATE INDEX IF NOT EXISTS photos_hash ON photos(hash);CREATE INDEX IF NOT EXISTS photos_similar ON photos(similar);CREATE INDEX IF NOT EXISTS photos_burst ON photos(burst);CREATE INDEX IF NOT EXISTS photos_active ON photos(trash,missing);CREATE INDEX IF NOT EXISTS photos_assets ON photos(asset_group);UPDATE jobs SET state='paused' WHERE state IN ('running','enumerating');")?;
        // Legacy paths gain an asset key while preserving ids, decisions and trash locations.
        let legacy: Vec<(i64, String)> = db
            .prepare("SELECT id,path FROM photos WHERE asset_group=''")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, path) in legacy {
            db.execute(
                "UPDATE photos SET asset_group=? WHERE id=?",
                params![asset_key(Path::new(&path)), id],
            )?;
        }
        let library = Arc::new(Self {
            data,
            thumbs,
            db: Mutex::new(db),
            mutation: Mutex::new(()),
            progress: Mutex::new(Progress {
                message: "Готово к работе".into(),
                ..Progress::default()
            }),
            pause: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            worker: Mutex::new(None),
            version: AtomicU64::new(1),
            cache_writes: AtomicU64::new(0),
            cache_trim: Mutex::new(()),
            video_preview: Mutex::new(()),
            counts: Mutex::new((0, json!({}))),
        });
        library.recover()?;
        let resumable:Option<(i64,i64,i64,i64)>=library.db.lock().query_row("SELECT id,done,total,errors FROM jobs WHERE state='paused' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).ok();
        if let Some((id, done, total, errors)) = resumable {
            *library.progress.lock() = Progress {
                paused: true,
                job: Some(id),
                done,
                total,
                errors,
                message: "Есть незавершённое сканирование".into(),
                ..Progress::default()
            }
        }
        Ok(library)
    }
    pub fn settings(&self) -> Settings {
        self.db
            .lock()
            .query_row(
                "SELECT value FROM settings WHERE key='preferences'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save_settings(&self, mut settings: Settings) -> Result<()> {
        settings.sensitivity = settings.sensitivity.clamp(2, 12);
        settings.cache_mb = settings.cache_mb.clamp(32, 32768);
        if !["light", "dark", "system"].contains(&settings.theme.as_str()) {
            bail!("Неизвестная тема")
        }
        let old = self.settings();
        if old.sensitivity != settings.sensitivity && self.progress.lock().running {
            bail!("Дождитесь паузы сканирования перед изменением сходства")
        }
        self.db.lock().execute("INSERT INTO settings VALUES('preferences',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&settings)?])?;
        if old.sensitivity != settings.sensitivity {
            let _guard = self.mutation.lock();
            self.group()?
        }
        if old.cache_mb != settings.cache_mb {
            self.trim_cache()?;
        }
        Ok(())
    }
    fn touch(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }
    pub fn photo(&self, id: i64) -> Result<Photo> {
        self.db
            .lock()
            .query_row(
                &format!("SELECT {COLUMNS} FROM photos WHERE id=?"),
                [id],
                photo_row,
            )
            .context("Фотография не найдена")
    }
    fn photos_sql(&self, query: &str, values: &[&dyn rusqlite::ToSql]) -> Result<Vec<Photo>> {
        let db = self.db.lock();
        let mut statement = db.prepare(query)?;
        Ok(statement
            .query_map(values, photo_row)?
            .collect::<rusqlite::Result<_>>()?)
    }
    fn expand(&self, ids: &[i64]) -> Result<Vec<Photo>> {
        let mut seen = HashSet::new();
        let mut photos = Vec::new();
        for &id in ids {
            let p = self.photo(id)?;
            let mut pairs = self.photos_sql(
                &format!("SELECT {COLUMNS} FROM photos WHERE asset_group=? AND missing=0"),
                &[&p.asset_group],
            )?;
            if !pairs
                .iter()
                .any(|p| analysis::RAW.contains(&analysis::extension(Path::new(&p.path)).as_str()))
            {
                pairs = vec![p]
            };
            for p in pairs {
                if seen.insert(p.id) {
                    photos.push(p)
                }
            }
        }
        Ok(photos)
    }
    pub fn start(self: &Arc<Self>, folder: &str, resume: bool) -> Result<()> {
        let _guard = self.mutation.lock();
        if self.progress.lock().running {
            bail!("Сканирование уже выполняется")
        }
        if let Some(handle) = self.worker.lock().take() {
            let _ = handle.join();
        }
        let _video = self.video_preview.lock();
        let (job, root) = if resume {
            let db = self.db.lock();
            db.query_row(
                "SELECT id,root FROM jobs WHERE state='paused' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .context("Нет сканирования для продолжения")?
        } else {
            if folder.trim().is_empty() {
                bail!("Укажите папку")
            }
            let path = canonical_path(Path::new(folder)).context("Папка не найдена")?;
            if !path.is_dir() || path.starts_with(&self.data) {
                bail!("Укажите папку с фотографиями")
            }
            let db = self.db.lock();
            db.execute(
                "INSERT INTO jobs(root,generation,state) VALUES(?,?,'enumerating')",
                params![path.to_string_lossy(), uuid::Uuid::new_v4().to_string()],
            )?;
            (db.last_insert_rowid(), path.to_string_lossy().to_string())
        };
        self.pause.store(false, Ordering::Relaxed);
        self.shutdown.store(false, Ordering::Relaxed);
        *self.progress.lock() = Progress {
            running: true,
            job: Some(job),
            message: "Поиск файлов…".into(),
            ..Progress::default()
        };
        let library = self.clone();
        *self.worker.lock() = Some(thread::spawn(move || {
            if let Err(error) = library.scan(job, Path::new(&root)) {
                let _ = library.db.lock().execute(
                    "UPDATE jobs SET state='paused',errors=errors+1 WHERE id=?",
                    [job],
                );
                let mut p = library.progress.lock();
                p.running = false;
                p.paused = true;
                p.errors += 1;
                p.last_error = error.to_string();
                p.message = "Сканирование приостановлено из-за ошибки".into();
            }
        }));
        Ok(())
    }
    pub fn pause(&self) {
        self.pause.store(true, Ordering::Relaxed);
    }
    pub fn close(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.pause();
        if let Some(handle) = self.worker.lock().take() {
            let _ = handle.join();
        }
        let _ = self.trim_cache();
    }
    pub fn is_closed(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
    fn stopped(&self) -> bool {
        self.pause.load(Ordering::Relaxed) || self.shutdown.load(Ordering::Relaxed)
    }
    fn scan(&self, job: i64, root: &Path) -> Result<()> {
        let (enumerated, generation): (bool, String) = self.db.lock().query_row(
            "SELECT enumerated,generation FROM jobs WHERE id=?",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if !enumerated {
            let mut batch = Vec::new();
            let walker = walkdir::WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() == 0
                        || (![".photosort-trash", ".git", ".photosort", "target"]
                            .contains(&entry.file_name().to_string_lossy().as_ref())
                            && !entry.path().starts_with(&self.data)
                            && !entry.path().is_symlink())
                });
            for entry in walker {
                if self.stopped() {
                    break;
                }
                match entry {
                    Ok(entry)
                        if entry.file_type().is_file() && analysis::supported(entry.path()) =>
                    {
                        batch.push(entry.path().to_string_lossy().to_string());
                        if batch.len() >= 500 {
                            self.enqueue(job, &batch)?;
                            batch.clear();
                        }
                    }
                    Err(error) => self.error(job, &error.to_string())?,
                    _ => {}
                }
            }
            self.enqueue(job, &batch)?;
            if !self.stopped() {
                self.db.lock().execute("UPDATE jobs SET enumerated=1,state='running',total=(SELECT COUNT(*) FROM scan_queue WHERE job=?) WHERE id=?",params![job,job])?;
            }
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(std::thread::available_parallelism().map_or(2, |n| n.get().min(4)))
            .build()?;
        while !self.stopped() {
            let paths: Vec<String> = {
                let db = self.db.lock();
                let mut statement=db.prepare("SELECT path FROM scan_queue WHERE job=? AND state='pending' ORDER BY path LIMIT 16")?;
                statement
                    .query_map([job], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            if paths.is_empty() {
                break;
            }
            let old: HashMap<String, Photo> = {
                let db = self.db.lock();
                let placeholders = vec!["?"; paths.len()].join(",");
                let mut statement = db.prepare(&format!(
                    "SELECT {COLUMNS} FROM photos WHERE path IN ({placeholders}) AND ahash!='' AND crop_hash!=''"
                ))?;
                statement
                    .query_map(rusqlite::params_from_iter(paths.iter()), photo_row)?
                    .map(|row| row.map(|photo| (photo.path.clone(), photo)))
                    .collect::<rusqlite::Result<_>>()?
            };
            let results: Vec<_> = pool.install(|| {
                paths
                    .par_iter()
                    .map(|path| {
                        if self.stopped() {
                            return (path, None);
                        }
                        let result = (|| -> Result<Option<(String, i64, i64, Description)>> {
                            let path = Path::new(path);
                            if path.is_symlink() {
                                bail!("Ссылка пропущена")
                            }
                            let meta = fs::metadata(path)?;
                            let mtime = modified(&meta);
                            let size = meta.len() as i64;
                            if let Some(p) = old.get(&path.to_string_lossy().to_string()) {
                                if p.trash.is_some() {
                                    return Ok(None);
                                }
                                if p.mtime == mtime && p.size == size {
                                    if self.thumbs.join(format!("{}.jpg", p.hash)).exists() {
                                        return Ok(None);
                                    }
                                    if analysis::fingerprint(path)? == p.hash {
                                        return Ok(None);
                                    }
                                }
                            }
                            let hash = analysis::fingerprint(path)?;
                            let description = analysis::describe(
                                path,
                                &self.thumbs.join(format!("{hash}.jpg")),
                                &self.data.join("scratch"),
                            )?;
                            let after = fs::metadata(path)?;
                            if after.len() != meta.len() || modified(&after) != mtime {
                                bail!("Файл изменился во время чтения")
                            }
                            Ok(Some((hash, size, mtime, description)))
                        })();
                        (path, Some(result))
                    })
                    .collect()
            });
            for (path, result) in results {
                if let Some(result) = result {
                    match result {
                        Ok(description) => {
                            let db = self.db.lock();
                            if let Some((hash, size, mtime, d)) = description {
                                db.execute("INSERT INTO photos(path,root,name,size,mtime,width,height,captured,hash,dhash,sharpness,seen,ahash,crop_hash,brightness,quality_hint,asset_group,media_kind,duration,fps,video_codec) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET root=excluded.root,name=excluded.name,size=excluded.size,mtime=excluded.mtime,width=excluded.width,height=excluded.height,captured=excluded.captured,hash=excluded.hash,dhash=excluded.dhash,sharpness=excluded.sharpness,seen=excluded.seen,ahash=excluded.ahash,crop_hash=excluded.crop_hash,brightness=excluded.brightness,quality_hint=excluded.quality_hint,asset_group=excluded.asset_group,media_kind=excluded.media_kind,duration=excluded.duration,fps=excluded.fps,video_codec=excluded.video_codec,missing=0,faces=NULL,eye_score=NULL",params![path,root.to_string_lossy(),Path::new(path).file_name().unwrap_or_default().to_string_lossy(),size,mtime,d.width,d.height,d.captured,hash,d.dhash,d.sharpness,generation,d.ahash,d.crop_hash,d.brightness,d.quality_hint,asset_key(Path::new(path)),d.media_kind,d.duration,d.fps,d.video_codec])?;
                            } else {
                                db.execute("UPDATE photos SET seen=?,missing=0 WHERE path=? AND trash IS NULL",params![generation,path])?;
                            }
                            db.execute(
                                "UPDATE scan_queue SET state='done' WHERE job=? AND path=?",
                                params![job, path],
                            )?;
                            db.execute("UPDATE jobs SET done=done+1 WHERE id=?", [job])?;
                        }
                        Err(error) => {
                            self.db.lock().execute("UPDATE scan_queue SET state='error',error=? WHERE job=? AND path=?",params![error.to_string(),job,path])?;
                            self.error(job, &error.to_string())?;
                        }
                    }
                    let (done, total, errors): (i64, i64, i64) = self.db.lock().query_row(
                        "SELECT done,total,errors FROM jobs WHERE id=?",
                        [job],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    let mut p = self.progress.lock();
                    p.done = done;
                    p.total = total;
                    p.errors = errors;
                    p.message = Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                }
            }
            self.touch();
            let processed = self.progress.lock().done;
            if processed > 0 && processed % 256 == 0 {
                self.trim_cache()?;
            }
        }
        if self.stopped() {
            return self.pause_job(job);
        }
        self.progress.lock().message = "Группировка кадров…".into();
        let errors: i64 =
            self.db
                .lock()
                .query_row("SELECT errors FROM jobs WHERE id=?", [job], |r| r.get(0))?;
        if errors == 0 {
            let rows: Vec<(i64, String)> = {
                let db = self.db.lock();
                let mut statement =
                    db.prepare("SELECT id,path FROM photos WHERE trash IS NULL AND seen != ?")?;
                statement
                    .query_map([&generation], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let db = self.db.lock();
            for (id, path) in rows {
                if Path::new(&path).starts_with(root) {
                    db.execute("UPDATE photos SET missing=1 WHERE id=?", [id])?;
                }
            }
        }
        if let Err(error) = self.group() {
            if self.stopped() {
                return self.pause_job(job);
            }
            return Err(error);
        }
        if self.stopped() {
            return self.pause_job(job);
        }
        self.db
            .lock()
            .execute("UPDATE jobs SET state='complete' WHERE id=?", [job])?;
        {
            let mut p = self.progress.lock();
            p.running = false;
            p.paused = false;
            p.message = "Библиотека обновлена".into();
        }
        self.touch();
        self.trim_cache()?;
        Ok(())
    }
    fn pause_job(&self, job: i64) -> Result<()> {
        self.db
            .lock()
            .execute("UPDATE jobs SET state='paused' WHERE id=?", [job])?;
        let mut p = self.progress.lock();
        p.running = false;
        p.paused = true;
        p.message = "Сканирование на паузе. Можно продолжить".into();
        Ok(())
    }
    fn enqueue(&self, job: i64, paths: &[String]) -> Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for path in paths {
            tx.execute(
                "INSERT OR IGNORE INTO scan_queue(job,path) VALUES(?,?)",
                params![job, path],
            )?;
        }
        tx.execute(
            "UPDATE jobs SET total=(SELECT COUNT(*) FROM scan_queue WHERE job=?) WHERE id=?",
            params![job, job],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn error(&self, job: i64, error: &str) -> Result<()> {
        self.db
            .lock()
            .execute("UPDATE jobs SET errors=errors+1 WHERE id=?", [job])?;
        self.progress.lock().last_error = error.into();
        Ok(())
    }
    pub fn group(&self) -> Result<()> {
        let sensitivity = self.settings().sensitivity;
        let rows: Vec<(
            i64,
            String,
            Option<f64>,
            u32,
            u32,
            String,
            String,
            String,
            String,
            f64,
        )> = {
            let db = self.db.lock();
            let mut statement=db.prepare(&format!("SELECT id,path,captured,width,height,dhash,ahash,crop_hash,media_kind,duration FROM photos WHERE {ACTIVE} ORDER BY captured,id"))?;
            statement
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut tree = HammingIndex::new(sensitivity);
        let mut averages = HammingIndex::new(sensitivity.saturating_sub(2));
        let mut cropped = HammingIndex::new(sensitivity.saturating_sub(2));
        let mut representatives = HashMap::<i64, (f64, u64, u64, u64, bool, f64)>::new();
        let mut previous = HashMap::<PathBuf, (f64, i64)>::new();
        let mut assignments = Vec::with_capacity(rows.len());
        for (index, (id, path, captured, w, h, d, a, c, kind, duration)) in
            rows.into_iter().enumerate()
        {
            let video = kind == "video";
            if index % 256 == 0 && self.stopped() && self.progress.lock().running {
                bail!("Группировка приостановлена");
            }
            let d = u64::from_str_radix(&d, 16).unwrap_or(0);
            let a = u64::from_str_radix(&a, 16).unwrap_or(d);
            let c = u64::from_str_radix(&c, 16).unwrap_or(a);
            let ratio = w as f64 / h.max(1) as f64;
            let candidates: HashSet<_> = tree
                .find(d)
                .into_iter()
                .chain(averages.find(a))
                .chain(cropped.find(c))
                .collect();
            let mut similar = id;
            let mut best = 100;
            for candidate in candidates {
                let (r, rd, ra, rc, candidate_video, candidate_duration) =
                    representatives[&candidate];
                let ds = (d ^ rd).count_ones();
                let av = (a ^ ra).count_ones();
                let cr = (c ^ rc).count_ones();
                let close = if video {
                    (duration - candidate_duration).abs()
                        <= (duration.min(candidate_duration) * 0.05).max(0.25)
                        && ds <= sensitivity
                        && av <= sensitivity
                        && cr <= sensitivity
                } else {
                    (ds <= sensitivity && av <= sensitivity + 4)
                        || (cr <= sensitivity.saturating_sub(2) && av <= sensitivity + 2)
                };
                if video == candidate_video && (ratio - r).abs() < 0.18 && close {
                    let score = ds + av + cr;
                    if score < best {
                        similar = candidate;
                        best = score
                    }
                }
            }
            if similar == id {
                tree.add(d, id);
                averages.add(a, id);
                cropped.add(c, id);
                representatives.insert(id, (ratio, d, a, c, video, duration));
            }
            let folder = Path::new(&path)
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf();
            let burst = if let Some(time) = captured.filter(|_| !video) {
                let burst = previous
                    .get(&folder)
                    .filter(|(old, _)| time - *old >= 0.0 && time - *old <= 8.0)
                    .map(|(_, id)| *id)
                    .unwrap_or(id);
                previous.insert(folder, (time, burst));
                burst
            } else {
                id
            };
            assignments.push((similar, burst, id));
        }
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for (similar, burst, id) in assignments {
            tx.execute(
                "UPDATE photos SET similar=?,burst=? WHERE id=?",
                params![similar, burst, id],
            )?;
        }
        tx.commit()?;
        self.touch();
        Ok(())
    }
    fn counts(&self) -> Result<Value> {
        let version = self.version.load(Ordering::Relaxed);
        {
            let cache = self.counts.lock();
            if cache.0 == version {
                return Ok(cache.1.clone());
            }
        }
        let db = self.db.lock();
        let mut counts = serde_json::Map::new();
        for (name, condition) in [
            ("all", ACTIVE.to_string()),
            ("favorites", format!("{ACTIVE} AND favorite=1")),
            ("trash", "trash IS NOT NULL".into()),
            ("pending", format!("{ACTIVE} AND reviewed=0")),
            ("videos", format!("{ACTIVE} AND media_kind='video'")),
            ("photos", format!("{ACTIVE} AND media_kind='photo'")),
        ] {
            let count: i64 = db.query_row(
                &format!("SELECT COUNT(*) FROM photos WHERE {condition}"),
                [],
                |r| r.get(0),
            )?;
            counts.insert(name.into(), json!(count));
        }
        for (name, field) in [
            ("duplicates", "hash"),
            ("similar", "similar"),
            ("bursts", "burst"),
        ] {
            let count:i64=db.query_row(&format!("SELECT COALESCE(SUM(n),0) FROM (SELECT COUNT(*) n FROM photos WHERE {ACTIVE} GROUP BY {field} HAVING COUNT(*)>1)"),[],|r|r.get(0))?;
            counts.insert(name.into(), json!(count));
        }
        counts.insert(
            "bytes".into(),
            json!(db.query_row(
                &format!("SELECT COALESCE(SUM(size),0) FROM photos WHERE {ACTIVE}"),
                [],
                |r| r.get::<_, i64>(0)
            )?),
        );
        drop(db);
        let value = Value::Object(counts);
        *self.counts.lock() = (version, value.clone());
        Ok(value)
    }
    pub fn listing(&self, view: &str, search: &str, offset: i64, sort: &str) -> Result<Value> {
        self.listing_filtered(view, search, offset, sort, "")
    }
    pub fn listing_filtered(
        &self,
        view: &str,
        search: &str,
        offset: i64,
        sort: &str,
        root: &str,
    ) -> Result<Value> {
        let field = match view {
            "duplicates" => Some("hash"),
            "similar" => Some("similar"),
            "bursts" => Some("burst"),
            _ => None,
        };
        let mut condition = match view {
            "trash" => "trash IS NOT NULL".into(),
            "favorites" => format!("{ACTIVE} AND favorite=1"),
            "pending" => format!("{ACTIVE} AND reviewed=0"),
            "videos" => format!("{ACTIVE} AND media_kind='video'"),
            "photos" => format!("{ACTIVE} AND media_kind='photo'"),
            _ => ACTIVE.to_string(),
        };
        if let Some(field) = field {
            condition += &format!(
                " AND {field} IN(SELECT {field} FROM photos WHERE {ACTIVE} GROUP BY {field} HAVING COUNT(*)>1)"
            )
        }
        condition += " AND instr(lower(name),lower(?))>0 AND (?='' OR root=?)";
        let order = match sort {
            "name" => "name COLLATE NOCASE",
            "quality" => "sharpness DESC",
            "size" => "size DESC",
            _ => "COALESCE(captured,mtime/1000000000) DESC",
        };
        let order = if let Some(field) = field {
            format!("{field},{order}")
        } else {
            order.into()
        };
        let total: i64 = self.db.lock().query_row(
            &format!("SELECT COUNT(*) FROM photos WHERE {condition}"),
            params![search, root, root],
            |r| r.get(0),
        )?;
        let photos=self.photos_sql(&format!("SELECT {COLUMNS} FROM photos WHERE {condition} ORDER BY {order},id LIMIT 80 OFFSET ?"),&[&search,&root,&root,&offset.max(0)])?;
        Ok(
            json!({"photos":photos,"total":total,"counts":self.counts()?,"progress":self.progress.lock().clone()}),
        )
    }
    fn record(&self, label: &str, snapshots: &[Snapshot]) -> Result<Option<i64>> {
        if snapshots.is_empty() {
            return Ok(None);
        }
        let db = self.db.lock();
        db.execute(
            "INSERT INTO actions(label,payload) VALUES(?,?)",
            params![label, serde_json::to_string(snapshots)?],
        )?;
        Ok(Some(db.last_insert_rowid()))
    }
    pub fn favorite(&self, ids: &[i64], value: bool) -> Result<Value> {
        let _guard = self.mutation.lock();
        let photos = self.expand(ids)?;
        let snapshots: Vec<_> = photos
            .iter()
            .filter(|p| p.favorite != value)
            .map(Snapshot::from)
            .collect();
        let action = self.record("Избранное", &snapshots)?;
        {
            let db = self.db.lock();
            for p in &photos {
                db.execute(
                    "UPDATE photos SET favorite=? WHERE id=?",
                    params![value, p.id],
                )?;
            }
        }
        self.touch();
        Ok(json!({"ok":true,"action":action}))
    }
    pub fn decision(&self, ids: &[i64], decision: &str) -> Result<Value> {
        if !["keep", "reject", "skip", "pending"].contains(&decision) {
            bail!("Неизвестное решение")
        }
        let _guard = self.mutation.lock();
        let photos = self.expand(ids)?;
        let snapshots: Vec<_> = photos.iter().map(Snapshot::from).collect();
        let action = self.record("Решение", &snapshots)?;
        {
            let db = self.db.lock();
            for p in &photos {
                db.execute("UPDATE photos SET decision=?,reviewed=?,favorite=CASE WHEN ?='keep' THEN 1 ELSE favorite END WHERE id=?",params![decision,decision!="pending",decision,p.id])?;
            }
        }
        self.touch();
        Ok(json!({"ok":true,"action":action}))
    }
    fn transfer(&self, photo: &Photo, restore: bool) -> Result<()> {
        let root = fs::canonicalize(&photo.root).context("Исходная папка недоступна")?;
        let original = PathBuf::from(&photo.path);
        let trash_dir = root.join(".photosort-trash");
        if trash_dir.exists()
            && (trash_dir.is_symlink() || fs::canonicalize(&trash_dir)? != trash_dir)
        {
            bail!("Папка корзины содержит внешнюю ссылку")
        }
        let parent = original.parent().context("Некорректный путь")?;
        if parent.exists() && !fs::canonicalize(parent)?.starts_with(&root) {
            bail!("Исходная папка изменилась")
        }
        let (source, target) = if restore {
            let source = PathBuf::from(photo.trash.as_ref().context("Файл не в корзине")?);
            if fs::canonicalize(source.parent().context("Некорректный путь")?)?
                != fs::canonicalize(&trash_dir)?
            {
                bail!("Некорректный путь корзины")
            }
            (source, original)
        } else {
            if photo.trash.is_some() {
                return Ok(());
            }
            let target = trash_dir.join(format!(
                "{}.{}",
                uuid::Uuid::new_v4(),
                analysis::extension(&original)
            ));
            (original, target)
        };
        if source.is_symlink() {
            bail!("Файл не должен быть ссылкой")
        }
        if analysis::fingerprint(&source)? != photo.hash {
            bail!("Файл изменился. Пересканируйте папку")
        }
        if target.exists() {
            bail!("Путь уже занят: существующий файл сохранён")
        }
        fs::create_dir_all(target.parent().context("Некорректный путь")?)?;
        if !fs::canonicalize(target.parent().unwrap())?.starts_with(&root) {
            bail!("Внешняя ссылка в пути")
        }
        let op = {
            let db = self.db.lock();
            db.execute(
                "INSERT INTO operations(photo,source,destination,kind) VALUES(?,?,?,?)",
                params![
                    photo.id,
                    source.to_string_lossy(),
                    target.to_string_lossy(),
                    if restore { "restore" } else { "trash" }
                ],
            )?;
            db.last_insert_rowid()
        };
        if let Err(error) = safe_transfer(&source, &target, &photo.hash) {
            let _ = self
                .db
                .lock()
                .execute("UPDATE operations SET state='cancelled' WHERE id=?", [op]);
            return Err(error);
        }
        let trash = if restore {
            None
        } else {
            Some(target.to_string_lossy().to_string())
        };
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE photos SET trash=?,missing=0 WHERE id=?",
            params![trash, photo.id],
        )?;
        tx.execute("UPDATE operations SET state='done' WHERE id=?", [op])?;
        tx.commit()?;
        Ok(())
    }
    pub fn move_photos(&self, ids: &[i64], restore: bool) -> Result<Outcome> {
        let _guard = self.mutation.lock();
        self.ensure_idle()?;
        let photos = self.expand(ids)?;
        let mut outcome = Outcome::default();
        let mut snapshots = Vec::new();
        for p in photos {
            if p.trash.is_some() != restore {
                continue;
            }
            match self.transfer(&p, restore) {
                Ok(()) => {
                    snapshots.push(Snapshot::from(&p));
                    outcome.done.push(p.id)
                }
                Err(error) => outcome
                    .errors
                    .push(json!({"id":p.id,"message":error.to_string()})),
            }
        }
        outcome.action = self.record(
            if restore {
                "Восстановление"
            } else {
                "Корзина"
            },
            &snapshots,
        )?;
        self.group()?;
        self.touch();
        Ok(outcome)
    }
    fn ensure_idle(&self) -> Result<()> {
        if self.progress.lock().running {
            bail!("Поставьте сканирование на паузу перед перемещением файлов")
        }
        Ok(())
    }
    pub fn undo(&self) -> Result<Outcome> {
        let _guard = self.mutation.lock();
        self.ensure_idle()?;
        let (id, payload): (i64, String) = self
            .db
            .lock()
            .query_row(
                "SELECT id,payload FROM actions WHERE state='done' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("Нет действий для отмены")?;
        let snapshots: Vec<Snapshot> = serde_json::from_str(&payload)?;
        let mut remaining = Vec::new();
        let mut outcome = Outcome::default();
        for old in snapshots {
            let result = (|| -> Result<()> {
                let p = self.photo(old.id)?;
                if p.trash.is_some() != old.trash.is_some() {
                    self.transfer(&p, old.trash.is_none())?;
                }
                self.db.lock().execute(
                    "UPDATE photos SET favorite=?,decision=?,reviewed=? WHERE id=?",
                    params![old.favorite, old.decision, old.reviewed, old.id],
                )?;
                Ok(())
            })();
            match result {
                Ok(()) => outcome.done.push(old.id),
                Err(error) => {
                    outcome
                        .errors
                        .push(json!({"id":old.id,"message":error.to_string()}));
                    remaining.push(old)
                }
            }
        }
        self.db.lock().execute(
            "UPDATE actions SET state=?,payload=? WHERE id=?",
            params![
                if remaining.is_empty() {
                    "undone"
                } else {
                    "done"
                },
                serde_json::to_string(&remaining)?,
                id
            ],
        )?;
        self.group()?;
        self.touch();
        Ok(outcome)
    }
    pub fn recover(&self) -> Result<()> {
        let ops: Vec<(i64, i64, String, String, String)> = {
            let db = self.db.lock();
            let mut statement = db.prepare(
                "SELECT id,photo,source,destination,kind FROM operations WHERE state='pending'",
            )?;
            statement
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, photo, source, destination, kind) in ops {
            let source = Path::new(&source);
            let target = Path::new(&destination);
            if !source.exists() && target.exists() {
                if analysis::fingerprint(target)? != self.photo(photo)?.hash {
                    continue;
                }
                let trash = if kind == "trash" {
                    Some(target.to_string_lossy().to_string())
                } else {
                    None
                };
                let db = self.db.lock();
                db.execute(
                    "UPDATE photos SET trash=?,missing=0 WHERE id=?",
                    params![trash, photo],
                )?;
                db.execute("UPDATE operations SET state='done' WHERE id=?", [id])?;
            } else if source.exists() {
                if target.exists() {
                    let same = same_file::is_same_file(source, target).unwrap_or(false);
                    if !same
                        && analysis::fingerprint(target).ok().as_deref()
                            != Some(self.photo(photo)?.hash.as_str())
                    {
                        continue;
                    }
                    fs::remove_file(target)?;
                }
                self.db
                    .lock()
                    .execute("UPDATE operations SET state='cancelled' WHERE id=?", [id])?;
            }
        }
        Ok(())
    }
    pub fn next_group(&self, kind: &str) -> Result<Value> {
        let field = match kind {
            "bursts" => "burst",
            "duplicates" => "hash",
            _ => "similar",
        };
        let (key, total, remaining): (Option<String>, i64, i64) = {
            let db = self.db.lock();
            let key=db.query_row(&format!("SELECT CAST({field} AS TEXT) FROM photos WHERE {ACTIVE} GROUP BY {field} HAVING COUNT(DISTINCT asset_group)>1 AND SUM(reviewed)<COUNT(*) ORDER BY MIN(id) LIMIT 1"),[],|r|r.get(0)).ok();
            let total=db.query_row(&format!("SELECT COUNT(*) FROM(SELECT {field} FROM photos WHERE {ACTIVE} GROUP BY {field} HAVING COUNT(DISTINCT asset_group)>1)"),[],|r|r.get(0))?;
            let remaining=db.query_row(&format!("SELECT COUNT(*) FROM(SELECT {field} FROM photos WHERE {ACTIVE} GROUP BY {field} HAVING COUNT(DISTINCT asset_group)>1 AND SUM(reviewed)<COUNT(*))"),[],|r|r.get(0))?;
            (key, total, remaining)
        };
        let photos = if let Some(key) = &key {
            self.photos_sql(&format!("SELECT {COLUMNS} FROM photos WHERE {ACTIVE} AND reviewed=0 AND CAST({field} AS TEXT)=? ORDER BY favorite DESC,sharpness DESC,id LIMIT 200"),&[key])?
        } else {
            vec![]
        };
        Ok(json!({"key":key,"kind":kind,"total":total,"remaining":remaining,"photos":photos}))
    }
    pub fn choose(&self, ids: &[i64], winner: Option<i64>, trash_others: bool) -> Result<Outcome> {
        let _guard = self.mutation.lock();
        self.ensure_idle()?;
        let photos = self.expand(ids)?;
        let winner_asset = winner
            .map(|id| self.photo(id))
            .transpose()?
            .map(|p| p.asset_group);
        if let Some(id) = winner {
            if !photos.iter().any(|p| p.id == id) {
                bail!("Выбранный кадр не входит в группу")
            }
        }
        let mut snapshots = Vec::new();
        let mut outcome = Outcome::default();
        for p in photos {
            if p.trash.is_some() {
                continue;
            }
            let chosen = winner_asset.as_deref() == Some(p.asset_group.as_str());
            if !chosen && winner.is_some() && trash_others {
                if let Err(error) = self.transfer(&p, false) {
                    outcome
                        .errors
                        .push(json!({"id":p.id,"message":error.to_string()}));
                    continue;
                }
            }
            snapshots.push(Snapshot::from(&p));
            self.db.lock().execute("UPDATE photos SET favorite=CASE WHEN ? THEN 1 ELSE favorite END,decision=?,reviewed=1 WHERE id=?",params![chosen,if winner.is_none(){"skip"}else if chosen{"keep"}else{"reject"},p.id])?;
            outcome.done.push(p.id);
        }
        outcome.action = self.record("Выбор лучшего кадра", &snapshots)?;
        self.group()?;
        self.touch();
        Ok(outcome)
    }
    pub fn duplicate_plan(&self, preferred: &str) -> Result<Value> {
        let photos=self.photos_sql(&format!("SELECT {COLUMNS} FROM photos WHERE {ACTIVE} AND hash IN(SELECT hash FROM photos WHERE {ACTIVE} GROUP BY hash HAVING COUNT(*)>1) ORDER BY hash,id"),&[])?;
        let mut groups = HashMap::<String, Vec<Photo>>::new();
        for photo in photos {
            groups.entry(photo.hash.clone()).or_default().push(photo)
        }
        let mut remove = Vec::new();
        let mut keep = Vec::new();
        let mut bytes = 0;
        let mut warnings = 0;
        for mut group in groups.into_values() {
            group.sort_by_key(|p| {
                (
                    !p.favorite,
                    !(!preferred.is_empty() && Path::new(&p.path).starts_with(preferred)),
                    p.path.len(),
                    p.id,
                )
            });
            let retained = group.remove(0);
            keep.push(json!({"id":retained.id,"name":retained.name,"path":retained.path}));
            for p in group {
                if p.favorite {
                    warnings += 1;
                    continue;
                }
                bytes += p.size;
                remove.push(json!({"id":p.id,"name":p.name,"path":p.path}));
            }
        }
        Ok(json!({"keep":keep,"remove":remove,"bytes":bytes,"protected_favorites":warnings}))
    }
    pub fn apply_duplicates(&self, ids: &[i64]) -> Result<Outcome> {
        // Revalidate the preview against current files and keep at least one active copy of every hash.
        let _guard = self.mutation.lock();
        self.ensure_idle()?;
        let photos = self.expand(ids)?;
        let selected: HashSet<_> = photos.iter().map(|p| p.id).collect();
        let mut outcome = Outcome::default();
        let mut snapshots = Vec::new();
        for p in photos {
            if p.favorite || p.trash.is_some() {
                continue;
            }
            let alternatives = self.photos_sql(
                &format!("SELECT {COLUMNS} FROM photos WHERE {ACTIVE} AND hash=?"),
                &[&p.hash],
            )?;
            if !alternatives.iter().any(|other| {
                !selected.contains(&other.id)
                    && Path::new(&other.path).exists()
                    && analysis::fingerprint(Path::new(&other.path))
                        .ok()
                        .as_deref()
                        == Some(p.hash.as_str())
            }) {
                outcome
                    .errors
                    .push(json!({"id":p.id,"message":"Не найдена проверенная сохраняемая копия"}));
                continue;
            }
            match self.transfer(&p, false) {
                Ok(()) => {
                    snapshots.push(Snapshot::from(&p));
                    outcome.done.push(p.id)
                }
                Err(error) => outcome
                    .errors
                    .push(json!({"id":p.id,"message":error.to_string()})),
            }
        }
        outcome.action = self.record("Удаление точных дублей", &snapshots)?;
        self.group()?;
        self.touch();
        Ok(outcome)
    }
    pub fn export(&self, ids: &[i64], destination: &str, structure: bool) -> Result<Value> {
        let _guard = self.mutation.lock();
        if destination.trim().is_empty() {
            bail!("Укажите папку экспорта")
        };
        fs::create_dir_all(destination)?;
        let destination = fs::canonicalize(destination)?;
        if destination.starts_with(&self.data) {
            bail!("Папка индекса не подходит для экспорта")
        }
        let photos = if ids.is_empty() {
            self.photos_sql(
                &format!("SELECT {COLUMNS} FROM photos WHERE {ACTIVE} AND favorite=1"),
                &[],
            )?
        } else {
            self.expand(ids)?
        };
        let mut done = 0;
        let mut errors = Vec::new();
        for p in photos {
            if p.trash.is_some() || p.missing {
                continue;
            }
            let result = (|| -> Result<()> {
                let source = Path::new(&p.path);
                let root = Path::new(&p.root);
                let relative = if structure {
                    let label = root.file_name().unwrap_or_default();
                    PathBuf::from(label).join(
                        source
                            .strip_prefix(root)
                            .context("Изменился исходный путь")?,
                    )
                } else {
                    PathBuf::from(&p.name)
                };
                let mut target = destination.join(relative);
                if target == source {
                    bail!("Исходная и конечная папка совпадают")
                };
                fs::create_dir_all(target.parent().unwrap())?;
                if !fs::canonicalize(target.parent().unwrap())?.starts_with(&destination) {
                    bail!("Внешняя ссылка в папке экспорта")
                };
                if target.exists() {
                    target = target.with_file_name(format!(
                        "{}_{}.{}",
                        source.file_stem().unwrap_or_default().to_string_lossy(),
                        p.id,
                        analysis::extension(source)
                    ));
                }
                copy_exclusive(source, &target, &p.hash)?;
                Ok(())
            })();
            match result {
                Ok(()) => done += 1,
                Err(e) => errors.push(json!({"id":p.id,"message":e.to_string()})),
            }
        }
        Ok(json!({"done":done,"errors":errors,"destination":destination}))
    }
    pub fn eyes(&self, id: i64, faces: i64, score: f64) -> Result<()> {
        if !(0.0..=1.0).contains(&score) || !(0..=100).contains(&faces) {
            bail!("Некорректный результат анализа")
        };
        self.db.lock().execute(
            "UPDATE photos SET faces=?,eye_score=? WHERE id=?",
            params![faces, score, id],
        )?;
        self.touch();
        Ok(())
    }
    pub fn thumbnail(&self, id: i64) -> Result<Vec<u8>> {
        let photo = self.photo(id)?;
        let path = self.thumbs.join(format!("{}.jpg", photo.hash));
        let generated = !path.exists();
        if generated {
            let source = Path::new(photo.trash.as_deref().unwrap_or(&photo.path));
            if photo.media_kind == "video" {
                let before = fs::metadata(source)?;
                if before.len() != photo.size as u64 || modified(&before) != photo.mtime {
                    bail!("Видео изменилось. Повторите импорт папки");
                }
                let frame =
                    crate::video::frame(source, &self.data.join("scratch"), photo.duration * 0.5)?;
                let after = fs::metadata(source)?;
                if after.len() != before.len() || modified(&after) != modified(&before) {
                    bail!("Видео изменилось во время создания превью");
                }
                analysis::save_thumbnail(&frame, &path)?;
            } else {
                analysis::describe(source, &path, &self.data.join("scratch"))?;
            }
        }
        let bytes = fs::read(path)?;
        if generated && self.cache_writes.fetch_add(1, Ordering::Relaxed) % 64 == 63 {
            self.trim_cache()?;
        }
        Ok(bytes)
    }
    pub fn video_source(&self, id: i64, compatible: bool) -> Result<PathBuf> {
        let photo = self.photo(id)?;
        if photo.media_kind != "video" || photo.missing {
            bail!("Видео недоступно");
        }
        let source = PathBuf::from(photo.trash.as_deref().unwrap_or(&photo.path));
        let before = fs::metadata(&source)?;
        if before.len() != photo.size as u64 || modified(&before) != photo.mtime {
            bail!("Видео изменилось. Повторите импорт папки");
        }
        if !compatible {
            return Ok(source);
        }
        let _guard = self.video_preview.lock();
        if self.is_closed() {
            bail!("Приложение закрывается");
        }
        let target = self.thumbs.join(format!("video-{}.webm", photo.hash));
        if target.is_file() {
            return Ok(target);
        }
        self.trim_cache()?;
        let temporary = self
            .thumbs
            .join(format!("video-{}.part", uuid::Uuid::new_v4()));
        let budget = (self.settings().cache_mb * 1024 * 1024 * 3 / 4).min(512 * 1024 * 1024);
        let result = (|| -> Result<()> {
            crate::video::transcode(&source, &temporary, budget, || self.is_closed())?;
            let metadata = crate::video::metadata(&temporary, &self.data.join("scratch"))?;
            if metadata.duration + 0.25 < photo.duration || fs::metadata(&temporary)?.len() > budget
            {
                bail!(
                    "Ролик слишком большой для совместимого превью. Увеличьте лимит кеша или используйте внешний проигрыватель"
                );
            }
            let after = fs::metadata(&source)?;
            if after.len() != before.len() || modified(&after) != modified(&before) {
                bail!("Видео изменилось во время подготовки");
            }
            fs::rename(&temporary, &target)?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result?;
        self.trim_cache_except(Some(&target))?;
        Ok(target)
    }
    pub fn trim_cache(&self) -> Result<Value> {
        self.trim_cache_except(None)
    }
    fn trim_cache_except(&self, protected: Option<&Path>) -> Result<Value> {
        let _guard = self.cache_trim.lock();
        let budget = self.settings().cache_mb * 1024 * 1024;
        let mut files = Vec::new();
        let mut bytes = 0;
        for entry in fs::read_dir(&self.thumbs)? {
            let entry = entry?;
            let owned_video = entry.file_name().to_str().is_some_and(|name| {
                name.strip_prefix("video-")
                    .and_then(|s| s.strip_suffix(".webm"))
                    .is_some_and(|hash| {
                        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                    })
            });
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("jpg") && !owned_video
            {
                continue;
            }
            let meta = entry.metadata()?;
            if meta.is_file() {
                bytes += meta.len();
                files.push((
                    meta.modified().unwrap_or(UNIX_EPOCH),
                    meta.len(),
                    entry.path(),
                ));
            }
        }
        files.sort_by_key(|f| f.0);
        let mut removed = 0;
        for (_, size, path) in files {
            if bytes <= budget {
                break;
            }
            if protected == Some(path.as_path()) {
                continue;
            }
            if let Err(error) = fs::remove_file(&path) {
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    continue;
                }
                return Err(error.into());
            }
            bytes = bytes.saturating_sub(size);
            removed += 1
        }
        Ok(json!({"bytes":bytes,"limit":budget,"removed":removed}))
    }
    pub fn status(&self) -> Result<Value> {
        let roots: Vec<String> = {
            let db = self.db.lock();
            let mut q = db.prepare("SELECT DISTINCT root FROM photos ORDER BY root")?;
            q.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let undo = self
            .db
            .lock()
            .query_row(
                "SELECT label FROM actions WHERE state='done' ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok();
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"engine":"Rust","settings":self.settings(),"roots":roots,"undo":undo,"progress":self.progress.lock().clone(),"codecs":analysis::decoder_path().is_some(),"video_codecs":crate::video::tool("ffmpeg").is_some() && crate::video::tool("ffprobe").is_some()}),
        )
    }
}
fn canonical_path(path: &Path) -> Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        // Match pathlib's 0.1 database paths, including ordinary and network drives.
        let value = resolved.to_string_lossy();
        if let Some(network) = value.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{network}")));
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(local));
        }
    }
    Ok(resolved)
}
pub fn asset_key(path: &Path) -> String {
    if crate::video::is_video(path) {
        return format!("video:{}", path.to_string_lossy().to_lowercase());
    }
    format!(
        "{}/{}",
        path.parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_lowercase(),
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    )
}
pub fn modified(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}
pub fn copy_exclusive(source: &Path, target: &Path, expected: &str) -> Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .context("Имя уже занято или папка недоступна")?;
    let result = (|| -> Result<()> {
        let mut block = vec![0; 1024 * 1024];
        loop {
            let size = input.read(&mut block)?;
            if size == 0 {
                break;
            }
            output.write_all(&block[..size])?
        }
        output.set_times(fs::FileTimes::new().set_modified(input.metadata()?.modified()?))?;
        output.sync_all()?;
        drop(output);
        if analysis::fingerprint(target)? != expected {
            bail!("Копия не прошла проверку SHA-256")
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(target);
    }
    result
}
pub fn safe_transfer(source: &Path, target: &Path, expected: &str) -> Result<()> {
    if target.exists() {
        bail!("Путь уже занят")
    }
    if fs::hard_link(source, target).is_err() {
        copy_exclusive(source, target, expected)?;
    }
    match analysis::fingerprint(source) {
        Ok(hash) if hash == expected => {}
        Ok(_) => {
            let _ = fs::remove_file(target);
            bail!("Файл изменился во время перемещения")
        }
        Err(error) => {
            let _ = fs::remove_file(target);
            return Err(error);
        }
    }
    if let Err(error) = fs::remove_file(source) {
        let _ = fs::remove_file(target);
        return Err(error.into());
    }
    Ok(())
}
