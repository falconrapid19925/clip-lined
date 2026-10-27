//! Deep module: owns the SQLite schema, gzip envelope, type codes and
//! byte-compat rules from docs/interop.md. Single adapter (rusqlite), no seam.

use crate::capture::{Block, ClipEvent, Format};
use crate::envelope;
use anyhow::Context;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// log.type values, matching the original app's database.
pub mod type_code {
    pub const REGISTERED_IMAGE: i64 = 0;
    pub const TEXT: i64 = 1;
    pub const SECONDARY_TEXT: i64 = 2;
    pub const URL: i64 = 3;
    pub const FILES: i64 = 4;
    pub const IMAGE: i64 = 5;
}

#[derive(Debug, Clone, Default)]
pub struct StoreOpts {
    /// Some(key) opens/creates an SQLCipher-encrypted database (encryption feature).
    pub key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub query: Option<String>,
    pub starred_only: bool,
    pub kind: Option<i64>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            query: None,
            starred_only: false,
            kind: None,
            limit: 200,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClipRow {
    pub id: i64,
    pub created: String,
    pub title: String,
    pub kind: i64,
    pub appname: String,
    pub starred: bool,
    pub tags: String,
    pub datasize: i64,
    pub url: String,
}

pub struct Store {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    path: PathBuf,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS log(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created TIMESTAMP not null default CURRENT_TIMESTAMP,
  title CHAR(55) not null default '',
  type INTEGER not null default 0,
  description TEXT not null default '',
  data TEXT not null default '',
  datasize INTEGER not null default 0,
  appname CHAR(20) not null default '',
  apptitle CHAR(100) not null default '',
  starred INTEGER not null default 0,
  tags TEXT not null default ',',
  url TEXT not null default '');
CREATE TABLE IF NOT EXISTS options(key CHAR(30) PRIMARY KEY not null default '', value TEXT not null default '');
CREATE TABLE IF NOT EXISTS common(
  id INTEGER PRIMARY KEY AUTOINCREMENT, pid INTEGER not null default 0,
  pos INTEGER not null default 1000, type INTEGER not null default 2,
  name TEXT not null default '', data TEXT not null default '',
  description TEXT not null default '');
";

impl Store {
    /// Opens an existing database in place; creates schema when absent.
    pub fn open(path: &Path, _opts: StoreOpts) -> anyhow::Result<Store> {
        // ponytail: opts.key ignored until the `encryption` cargo feature lands (SQLCipher).
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        })
    }

    /// Stores an event. Deduplicates against the last row's payload.
    pub fn append(&self, ev: &ClipEvent) -> anyhow::Result<u64> {
        let payload = envelope::pack(&ev.blocks)?;
        if let Some(last) = self.last_payload()? {
            if last == payload {
                return Ok(0);
            }
        }
        let text = text_of(&ev.blocks);
        let (title, url, kind) = classify(&text, &ev.blocks);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO log(title,type,data,datasize,appname,apptitle,starred,tags,url)
             VALUES(?1,?2,?3,?4,?5,?6,0,',',?7)",
            params![
                truncate(&title, 55),
                kind,
                payload,
                payload.len() as i64,
                truncate(&ev.source_app, 20),
                truncate(&ev.source_title, 100),
                url
            ],
        )?;
        Ok(conn.last_insert_rowid() as u64)
    }

    pub fn list(&self, f: &Filter) -> anyhow::Result<Vec<ClipRow>> {
        let mut sql = String::from(
            "SELECT id,created,title,type,appname,starred,tags,datasize,url FROM log WHERE 1=1",
        );
        if f.starred_only {
            sql.push_str(" AND starred=1");
        }
        if let Some(k) = f.kind {
            sql.push_str(&format!(" AND type={k}"));
        }
        if f.query.is_some() {
            sql.push_str(" AND (title LIKE '%'||?1||'%' OR appname LIKE '%'||?1||'%' OR url LIKE '%'||?1||'%')");
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?2 OFFSET ?3");
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                f.query.clone().unwrap_or_default(),
                f.limit.max(1),
                f.offset
            ],
            |r| {
                Ok(ClipRow {
                    id: r.get(0)?,
                    created: r.get(1)?,
                    title: r.get(2)?,
                    kind: r.get(3)?,
                    appname: r.get(4).unwrap_or_default(),
                    starred: r.get::<_, i64>(5)? != 0,
                    tags: r.get(6).unwrap_or_default(),
                    datasize: r.get(7).unwrap_or_default(),
                    url: r.get(8).unwrap_or_default(),
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_data(&self, id: i64) -> anyhow::Result<Vec<Block>> {
        let conn = self.conn.lock().unwrap();
        let blob: Vec<u8> =
            conn.query_row("SELECT data FROM log WHERE id=?1", params![id], |r| {
                r.get(0)
            })?;
        drop(conn);
        envelope::unpack(&blob)
    }

    pub fn set_star(&self, id: i64, on: bool) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE log SET starred=?2 WHERE id=?1",
            params![id, on as i64],
        )?;
        Ok(())
    }

    pub fn delete(&self, ids: &[i64]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        for id in ids {
            conn.execute("DELETE FROM log WHERE id=?1", params![id])?;
        }
        Ok(())
    }

    /// Copies every row from a legacy database into this one, preserving ids.
    pub fn import_legacy(&self, legacy: &Path) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS src;",
            legacy.display().to_string().replace('\'', "''")
        ))?;
        let before: i64 = conn.query_row("SELECT count(*) FROM log", [], |r| r.get(0))?;
        conn.execute_batch(
            "INSERT OR IGNORE INTO log SELECT * FROM src.log;
             INSERT OR IGNORE INTO options SELECT * FROM src.options;",
        )?;
        let after: i64 = conn.query_row("SELECT count(*) FROM log", [], |r| r.get(0))?;
        conn.execute_batch("DETACH DATABASE src;")?;
        Ok((after - before) as u64)
    }

    fn last_payload(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT data FROM log ORDER BY id DESC LIMIT 1")?;
        let mut rows = stmt.query([])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }
}

/// UTF-16LE → String for CF_UNICODETEXT blocks.
fn text_of(blocks: &[Block]) -> String {
    for b in blocks {
        if b.format == Format::UnicodeText {
            let units: Vec<u16> = b
                .bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            if let Ok(s) = String::from_utf16(&units) {
                return s.trim_end_matches('\0').to_string();
            }
        }
    }
    String::new()
}

fn classify(text: &str, blocks: &[Block]) -> (String, String, i64) {
    if blocks.iter().any(|b| b.format == Format::ImageDib) {
        return ("Image".into(), String::new(), type_code::IMAGE);
    }
    if blocks.iter().any(|b| b.format == Format::FileDrop) {
        return ("Files".into(), String::new(), type_code::FILES);
    }
    let first_line = text.lines().next().unwrap_or_default().to_string();
    let looks_url = text.len() < 2048
        && (text.starts_with("http://") || text.starts_with("https://"))
        && !text.contains(char::is_whitespace);
    if looks_url {
        return (first_line.clone(), text.to_string(), type_code::URL);
    }
    (first_line, String::new(), type_code::TEXT)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
