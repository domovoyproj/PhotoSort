use photosort::{
    analysis::{self, BKTree, HammingIndex},
    library::Library,
};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    library: Arc<Library>,
    root: PathBuf,
    _temp: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("build/rust-tests");
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("test-")
            .tempdir_in(base)
            .unwrap();
        let root = temp.path().join("photos");
        fs::create_dir(&root).unwrap();
        let library = Library::open(&temp.path().join("index")).unwrap();
        Self {
            _temp: temp,
            root,
            library,
        }
    }
    fn image(&self, name: &str, seed: u32) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let image = image::RgbImage::from_fn(160, 120, |x, y| {
            image::Rgb([
                ((x * 3 + seed) % 256) as u8,
                ((y * 4 + seed) % 256) as u8,
                ((x + y + seed) % 256) as u8,
            ])
        });
        image.save(&path).unwrap();
        path
    }
    fn scan(&self) {
        self.library
            .start(self.root.to_str().unwrap(), false)
            .unwrap();
        self.wait();
        let progress = self.library.progress.lock().clone();
        assert_eq!(progress.errors, 0, "{}", progress.last_error);
    }
    fn wait(&self) {
        let start = Instant::now();
        while self.library.progress.lock().running {
            assert!(start.elapsed() < Duration::from_secs(90));
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn photos(&self, view: &str) -> Vec<Value> {
        self.library.listing(view, "", 0, "name").unwrap()["photos"]
            .as_array()
            .unwrap()
            .clone()
    }
    fn db(&self) -> Connection {
        Connection::open(self.library.data.join("library.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.library.close();
    }
}
#[test]
fn duplicates_and_incremental_scan() {
    let f = Fixture::new();
    let one = f.image("one.jpg", 0);
    fs::copy(&one, f.root.join("copy.jpg")).unwrap();
    f.scan();
    assert_eq!(f.photos("duplicates").len(), 2);
    let p = f.photos("all")[0].clone();
    let thumb = f
        .library
        .thumbs
        .join(format!("{}.jpg", p["hash"].as_str().unwrap()));
    let before = fs::metadata(&thumb).unwrap().modified().unwrap();
    f.scan();
    assert_eq!(before, fs::metadata(thumb).unwrap().modified().unwrap());
}
#[test]
fn trash_restore_and_undo_persist() {
    let f = Fixture::new();
    let path = f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    assert!(
        f.library
            .move_photos(&[id], false)
            .unwrap()
            .errors
            .is_empty()
    );
    assert!(!path.exists());
    assert_eq!(f.photos("trash").len(), 1);
    f.library.close();
    let reopened = Library::open(&f.library.data).unwrap();
    assert!(reopened.undo().unwrap().errors.is_empty());
    assert!(path.exists());
    reopened.close();
}
#[test]
fn conflicts_never_overwrite() {
    let f = Fixture::new();
    let path = f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    f.library.move_photos(&[id], false).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert_eq!(f.library.move_photos(&[id], true).unwrap().errors.len(), 1);
    assert_eq!(fs::read(path).unwrap(), b"replacement");
    assert!(f.library.photo(id).unwrap().trash.is_some());
}
#[test]
fn changed_file_is_protected() {
    let f = Fixture::new();
    let path = f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    fs::write(&path, b"edited").unwrap();
    assert_eq!(f.library.move_photos(&[id], false).unwrap().errors.len(), 1);
    assert!(path.exists());
}
#[test]
fn cull_decisions_and_undo() {
    let f = Fixture::new();
    f.image("one.jpg", 0);
    f.image("two.jpg", 0);
    f.scan();
    let ids: Vec<_> = f
        .photos("all")
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    f.library.choose(&ids, Some(ids[0]), true).unwrap();
    assert_eq!(f.photos("favorites").len(), 1);
    assert_eq!(f.photos("trash").len(), 1);
    f.library.undo().unwrap();
    assert_eq!(f.photos("all").len(), 2);
    assert_eq!(f.photos("favorites").len(), 0);
    assert_eq!(f.photos("trash").len(), 0);
}
#[test]
fn duplicate_rules_keep_favorites() {
    let f = Fixture::new();
    let source = f.image("one.jpg", 0);
    fs::copy(&source, f.root.join("copy.jpg")).unwrap();
    f.scan();
    let favorite = f.photos("all")[0]["id"].as_i64().unwrap();
    f.library.favorite(&[favorite], true).unwrap();
    let plan = f.library.duplicate_plan("").unwrap();
    assert_eq!(plan["keep"][0]["id"], favorite);
    let remove: Vec<_> = plan["remove"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    f.library.apply_duplicates(&remove).unwrap();
    assert_eq!(f.photos("all").len(), 1);
    assert_eq!(f.photos("favorites").len(), 1);
}
#[test]
fn duplicate_plan_cannot_remove_all_copies() {
    let f = Fixture::new();
    let source = f.image("one.jpg", 0);
    fs::copy(&source, f.root.join("copy.jpg")).unwrap();
    f.scan();
    let ids: Vec<_> = f
        .photos("all")
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(f.library.apply_duplicates(&ids).unwrap().errors.len(), 2);
    assert_eq!(f.photos("all").len(), 2);
}
#[test]
fn export_is_verified_and_exclusive() {
    let f = Fixture::new();
    let source = f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    let out = f._temp.path().join("export");
    f.library
        .export(&[id], out.to_str().unwrap(), false)
        .unwrap();
    assert_eq!(
        analysis::fingerprint(&source).unwrap(),
        analysis::fingerprint(&out.join("one.jpg")).unwrap()
    );
    fs::write(out.join("one.jpg"), b"existing").unwrap();
    f.library
        .export(&[id], out.to_str().unwrap(), false)
        .unwrap();
    assert_eq!(fs::read(out.join("one.jpg")).unwrap(), b"existing");
    assert!(source.exists());
}
#[test]
fn paused_queue_survives_restart() {
    let f = Fixture::new();
    for i in 0..60 {
        f.image(&format!("{i:03}.jpg"), i);
    }
    f.library.start(f.root.to_str().unwrap(), false).unwrap();
    f.library.pause();
    f.wait();
    assert!(f.library.progress.lock().paused);
    f.library.close();
    let next = Library::open(&f.library.data).unwrap();
    assert!(next.progress.lock().paused);
    next.start("", true).unwrap();
    let start = Instant::now();
    while next.progress.lock().running {
        assert!(start.elapsed() < Duration::from_secs(90));
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(next.listing("all", "", 0, "name").unwrap()["total"], 60);
    next.close();
}
#[test]
fn crash_recovery_preserves_source_or_target() {
    let f = Fixture::new();
    let source = f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    let trash = f.root.join(".photosort-trash/recover.jpg");
    fs::create_dir_all(trash.parent().unwrap()).unwrap();
    f.db()
        .execute(
            "INSERT INTO operations(photo,source,destination,kind) VALUES(?,?,?,'trash')",
            params![id, source.to_string_lossy(), trash.to_string_lossy()],
        )
        .unwrap();
    fs::hard_link(&source, &trash).unwrap();
    f.library.recover().unwrap();
    assert!(source.exists());
    assert!(!trash.exists());
    f.db()
        .execute(
            "INSERT INTO operations(photo,source,destination,kind) VALUES(?,?,?,'trash')",
            params![id, source.to_string_lossy(), trash.to_string_lossy()],
        )
        .unwrap();
    fs::rename(&source, &trash).unwrap();
    f.library.recover().unwrap();
    assert!(f.library.photo(id).unwrap().trash.is_some());
    assert!(
        f.library
            .move_photos(&[id], true)
            .unwrap()
            .errors
            .is_empty()
    );
}
#[test]
fn raw_jpeg_pair_selection() {
    let f = Fixture::new();
    let jpeg = f.image("capture.jpg", 0);
    f.scan();
    let p = f
        .library
        .photo(f.photos("all")[0]["id"].as_i64().unwrap())
        .unwrap();
    let raw = f.root.join("capture.dng");
    fs::copy(&jpeg, &raw).unwrap();
    f.db().execute("INSERT INTO photos(path,root,name,size,mtime,width,height,hash,dhash,sharpness,asset_group) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![raw.to_string_lossy(),p.root,"capture.dng",p.size,p.mtime,p.width,p.height,p.hash,p.dhash,p.sharpness,p.asset_group]).unwrap();
    f.library.favorite(&[p.id], true).unwrap();
    assert_eq!(f.photos("favorites").len(), 2);
    assert_eq!(f.library.move_photos(&[p.id], false).unwrap().done.len(), 2);
    assert!(!jpeg.exists() && !raw.exists());
    f.library.undo().unwrap();
    assert!(jpeg.exists() && raw.exists());
}
#[test]
fn settings_and_review_persist() {
    let f = Fixture::new();
    f.image("one.jpg", 0);
    f.scan();
    let id = f.photos("all")[0]["id"].as_i64().unwrap();
    let mut settings = f.library.settings();
    settings.theme = "dark".into();
    settings.sensitivity = 4;
    settings.ui_state = serde_json::json!({"view":"favorites","offset":80});
    f.library.save_settings(settings).unwrap();
    f.library.decision(&[id], "skip").unwrap();
    f.library.close();
    let next = Library::open(&f.library.data).unwrap();
    assert_eq!(next.settings().theme, "dark");
    assert_eq!(next.photo(id).unwrap().decision, "skip");
    next.close();
}
#[test]
fn metric_search_matches_bruteforce() {
    let mut tree = BKTree::default();
    for value in [0u64, 7, 15, 255, 65535] {
        tree.add(value, value as i64);
    }
    let mut found = tree.find(0, 3);
    found.sort();
    assert_eq!(found, vec![0, 7]);
}
#[test]
fn segmented_hamming_search_matches_bruteforce() {
    let values: Vec<u64> = (0..3000u64)
        .map(|i| i.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(17))
        .collect();
    for radius in [0, 2, 6, 12] {
        let mut index = HammingIndex::new(radius);
        for (id, &hash) in values.iter().enumerate() {
            index.add(hash, id as i64);
        }
        index.add(values[0], 3000);
        for &base in values.iter().step_by(200) {
            for query in [base, base ^ 0b101010, base ^ 0xfff] {
                let mut expected: Vec<i64> = values
                    .iter()
                    .enumerate()
                    .filter(|(_, hash)| (**hash ^ query).count_ones() <= radius)
                    .map(|(id, _)| id as i64)
                    .collect();
                if (values[0] ^ query).count_ones() <= radius {
                    expected.push(3000);
                }
                let mut actual = index.find(query);
                actual.sort();
                assert_eq!(actual, expected, "radius={radius}");
            }
        }
    }
}
#[test]
fn burst_groups_and_child_scan() {
    let f = Fixture::new();
    f.image("a/one.jpg", 0);
    f.image("b/one.jpg", 0);
    f.image("a/two.jpg", 0);
    f.scan();
    for p in f.photos("all") {
        let time = if p["path"].as_str().unwrap().contains("two") {
            102.0
        } else {
            100.0
        };
        f.db()
            .execute(
                "UPDATE photos SET captured=? WHERE id=?",
                params![time, p["id"].as_i64().unwrap()],
            )
            .unwrap();
    }
    f.library.group().unwrap();
    assert_eq!(f.photos("bursts").len(), 2);
    f.library
        .start(f.root.join("a").to_str().unwrap(), false)
        .unwrap();
    f.wait();
    assert_eq!(f.photos("all").len(), 3);
}
#[test]
fn api_rejects_cross_site_mutations() {
    let f = Fixture::new();
    let url = photosort::server::serve(f.library.clone(), 0).unwrap();
    let host = url.trim_start_matches("http://");
    fn request(host: &str, data: &str) -> String {
        let mut stream = std::net::TcpStream::connect(host).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(data.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
    let response = request(
        host,
        &format!(
            "POST /api/undo HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{{}}"
        ),
    );
    assert!(response.contains("403"));
    let response = request(
        host,
        "GET /api/session HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(response.contains("403"));
    f.library.close();
    thread::sleep(Duration::from_millis(250));
}

#[test]
#[ignore = "Run explicitly for 100,000-row scale validation"]
fn metadata_benchmark_100k() {
    let f = Fixture::new();
    let mut db = f.db();
    let tx = db.transaction().unwrap();
    for i in 0..100000i64 {
        let hash = format!(
            "{:016x}",
            ((i as u64).wrapping_mul(0x9e3779b97f4a7c15)).rotate_left(17)
        );
        tx.execute("INSERT INTO photos(path,root,name,size,mtime,width,height,hash,dhash,ahash,crop_hash,sharpness,asset_group) VALUES(?,?,?,1,1,1200,800,?,?,?,?,1,?)",params![format!("/benchmark/{i}.jpg"),"/benchmark",format!("{i}.jpg"),format!("{i}"),hash,hash,hash,format!("{i}")]).unwrap();
    }
    tx.commit().unwrap();
    let started = Instant::now();
    f.library.group().unwrap();
    let grouped = started.elapsed();
    let started = Instant::now();
    let page = f.library.listing("all", "", 0, "name").unwrap();
    assert_eq!(page["total"], 100000);
    println!(
        "100k metadata: grouping={grouped:?}, listing={:?}",
        started.elapsed()
    );
}

#[test]
fn legacy_v01_database_preserves_favorites_and_restores_trash() {
    let f = Fixture::new();
    let original = f.image("legacy.jpg", 5);
    let original = fs::canonicalize(original).unwrap();
    let original = PathBuf::from(original.to_string_lossy().trim_start_matches(r"\\?\"));
    let legacy_root = fs::canonicalize(&f.root).unwrap();
    let legacy_root = PathBuf::from(legacy_root.to_string_lossy().trim_start_matches(r"\\?\"));
    let hash = analysis::fingerprint(&original).unwrap();
    let trash_dir = f.root.join(".photosort-trash");
    fs::create_dir(&trash_dir).unwrap();
    let trash = trash_dir.join("legacy-copy.jpg");
    fs::copy(&original, &trash).unwrap();
    let data = f._temp.path().join("legacy-index");
    fs::create_dir(&data).unwrap();
    let db = Connection::open(data.join("library.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE photos(id INTEGER PRIMARY KEY,path TEXT UNIQUE NOT NULL,root TEXT NOT NULL,name TEXT,size INTEGER,mtime INTEGER,width INTEGER,height INTEGER,captured REAL,hash TEXT,dhash TEXT,sharpness REAL,favorite INTEGER DEFAULT 0,trash TEXT,missing INTEGER DEFAULT 0,similar INTEGER,burst INTEGER,seen TEXT);CREATE TABLE operations(id INTEGER PRIMARY KEY,photo INTEGER,source TEXT,destination TEXT,kind TEXT,state TEXT DEFAULT 'pending');").unwrap();
    db.execute("INSERT INTO photos(id,path,root,name,size,mtime,width,height,hash,dhash,sharpness,favorite) VALUES(41,?,?,?,1,1,160,120,?,'0000000000000000',1,1)",params![original.to_str().unwrap(),legacy_root.to_str().unwrap(),"legacy.jpg",hash]).unwrap();
    let restored = legacy_root.join("legacy-copy.jpg");
    db.execute("INSERT INTO photos(id,path,root,name,size,mtime,width,height,hash,dhash,sharpness,trash) VALUES(42,?,?,?,1,1,160,120,?,'0000000000000000',1,?)",params![restored.to_str().unwrap(),legacy_root.to_str().unwrap(),"legacy-copy.jpg",hash,trash.to_str().unwrap()]).unwrap();
    drop(db);
    let migrated = Library::open(&data).unwrap();
    assert!(migrated.photo(41).unwrap().favorite);
    assert_eq!(migrated.move_photos(&[42], true).unwrap().done, vec![42]);
    assert_eq!(analysis::fingerprint(&restored).unwrap(), hash);
    migrated.start(f.root.to_str().unwrap(), false).unwrap();
    while migrated.progress.lock().running {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(migrated.progress.lock().errors, 0);
    assert!(migrated.photo(41).unwrap().favorite);
    let db = Connection::open(data.join("library.sqlite3")).unwrap();
    let average: String = db
        .query_row("SELECT ahash FROM photos WHERE id=41", [], |row| row.get(0))
        .unwrap();
    let rows: Vec<(i64, String, String)> = db
        .prepare("SELECT id,path,ahash FROM photos")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!average.is_empty(), "Migration rows: {rows:?}");
    assert_eq!(rows.len(), 2);
    drop(db);
    migrated.close();
}

#[test]
fn cache_budget_evicts_only_previews() {
    let f = Fixture::new();
    let original = f.image("untouched.jpg", 8);
    let before = analysis::fingerprint(&original).unwrap();
    let note = f.library.thumbs.join("notes.txt");
    fs::write(&note, b"preserve non-preview files").unwrap();
    for i in 0..3 {
        let file = fs::File::create(f.library.thumbs.join(format!("large-{i}.jpg"))).unwrap();
        file.set_len(16 * 1024 * 1024).unwrap();
    }
    let mut settings = f.library.settings();
    settings.cache_mb = 32;
    f.library.save_settings(settings).unwrap();
    let size: u64 = fs::read_dir(&f.library.thumbs)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum();
    assert!(size <= 32 * 1024 * 1024 + fs::metadata(&note).unwrap().len());
    assert_eq!(analysis::fingerprint(&original).unwrap(), before);
    assert!(note.exists());
}

#[test]
fn cache_remains_bounded_during_preview_regeneration() {
    let f = Fixture::new();
    f.image("regenerate.jpg", 9);
    f.scan();
    let photo = f
        .library
        .photo(f.photos("all")[0]["id"].as_i64().unwrap())
        .unwrap();
    let mut settings = f.library.settings();
    settings.cache_mb = 32;
    f.library.save_settings(settings).unwrap();
    for i in 0..3 {
        let file = fs::File::create(f.library.thumbs.join(format!("old-{i}.jpg"))).unwrap();
        file.set_len(16 * 1024 * 1024).unwrap();
    }
    let thumbnail = f.library.thumbs.join(format!("{}.jpg", photo.hash));
    for _ in 0..64 {
        if thumbnail.exists() {
            fs::remove_file(&thumbnail).unwrap();
        }
        let bytes = f.library.thumbnail(photo.id).unwrap();
        assert_eq!(&bytes[..2], b"\xff\xd8");
    }
    let size: u64 = fs::read_dir(&f.library.thumbs)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum();
    assert!(size <= 32 * 1024 * 1024);
}
