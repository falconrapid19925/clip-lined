//! Deep module: owns the SQLite schema, gzip envelope, type codes and
//! byte-compat rules from docs/interop.md. Single adapter (rusqlite), no seam.

use crate::capture::{Block, ClipEvent, Format};
use crate::envelope;
use anyhow::{Context, bail};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// log.type values, matching the original app's database.
pub mod type_code {
    pub const REGISTERED_IMAGE: i64 = 0;
    pub const TEXT: i64 = 1;
    pub const SECONDARY_TEXT: i64 = 2;
    pub const URL: i64 = 3;
    pub const FILES: i64 = 4;
    pub const IMAGE: i64 = 5;
}

#[derive(Clone, Default)]
pub struct StoreOpts {
    /// Some(key) opens/creates an SQLCipher-encrypted database (encryption feature).
    pub key: Option<String>,
}

impl std::fmt::Debug for StoreOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreOpts")
            .field("key", &self.key.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub query: Option<String>,
    pub starred_only: bool,
    pub kind: Option<i64>,
    /// Inclusive `created` bounds, SQLite `CURRENT_TIMESTAMP` format
    /// (`YYYY-MM-DD HH:MM:SS`; date-only prefixes also compare correctly).
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            query: None,
            starred_only: false,
            kind: None,
            from: None,
            to: None,
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

/// Row of snippets.sdb `common` (attached second database).
#[derive(Debug, Clone)]
pub struct Snippet {
    pub id: i64,
    pub pid: i64,
    pub pos: i64,
    pub kind: i64,
    pub name: String,
    pub data: String,
    pub description: String,
}

pub struct Store {
    conn: Mutex<Connection>,
    snips_attached: AtomicBool,
    search_data_version: AtomicI64,
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
-- Registered format IDs are assigned per Windows session. Keep names in an
-- additive table without changing ClipDiary's log rows or payload envelope.
CREATE TABLE IF NOT EXISTS clipline_format_names(
  clip_id INTEGER NOT NULL, format_id INTEGER NOT NULL, name TEXT NOT NULL,
  PRIMARY KEY(clip_id, format_id));
-- Plain SQLite triggers work in the legacy app as well and avoid stale names
-- (including when that app replaces or removes a row).
CREATE TRIGGER IF NOT EXISTS clipline_format_names_delete AFTER DELETE ON log BEGIN
  DELETE FROM clipline_format_names WHERE clip_id=old.id;
END;
CREATE TRIGGER IF NOT EXISTS clipline_format_names_update AFTER UPDATE OF data ON log BEGIN
  DELETE FROM clipline_format_names WHERE clip_id=old.id;
END;
";

const SNIPPETS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS snippets.common(
  id INTEGER PRIMARY KEY AUTOINCREMENT, pid INTEGER not null default 0,
  pos INTEGER not null default 1000, type INTEGER not null default 2,
  name TEXT not null default '', data TEXT not null default '',
  description TEXT not null default '');
";

// This index and these triggers are connection-local: ClipDiary's database is
// never altered with virtual tables or triggers that its SQLite cannot load.
// Trigrams preserve the existing substring-LIKE behavior, including short
// queries (which SQLite evaluates by scanning the index).
const SEARCH_SCHEMA: &str = "
CREATE VIRTUAL TABLE temp.clipline_search USING fts5(title, appname, url, tokenize='trigram');
CREATE TEMP TRIGGER clipline_search_insert AFTER INSERT ON main.log BEGIN
  INSERT INTO clipline_search(rowid,title,appname,url) VALUES(new.id,new.title,new.appname,new.url);
END;
CREATE TEMP TRIGGER clipline_search_delete AFTER DELETE ON main.log BEGIN
  DELETE FROM clipline_search WHERE rowid=old.id;
END;
CREATE TEMP TRIGGER clipline_search_update AFTER UPDATE OF title,appname,url ON main.log BEGIN
  DELETE FROM clipline_search WHERE rowid=old.id;
  INSERT INTO clipline_search(rowid,title,appname,url) VALUES(new.id,new.title,new.appname,new.url);
END;
";

fn rebuild_search(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DELETE FROM temp.clipline_search;
        INSERT INTO temp.clipline_search(rowid,title,appname,url)
        SELECT id,title,appname,url FROM main.log;",
    )?;
    Ok(())
}

impl Store {
    /// Opens an existing database in place; creates schema when absent.
    pub fn open(path: &Path, opts: StoreOpts) -> anyhow::Result<Store> {
        #[cfg(not(feature = "encryption"))]
        if opts.key.is_some() {
            bail!("encrypted databases require the encryption cargo feature");
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        #[cfg(feature = "encryption")]
        if let Some(key) = opts.key.as_deref() {
            if key.is_empty() {
                bail!("database encryption key must not be empty");
            }
            // pragma_update validates against pragma_pragma_list, which reads
            // the encrypted schema before the key is set. PRAGMA cannot bind
            // parameters, so quote the key as a SQL literal instead.
            conn.execute_batch(&format!("PRAGMA key = '{}';", key.replace('\'', "''")))
                .context("set database key")?;
        }
        // Force SQLCipher to validate the key before writing schema or pragmas.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get::<_, i64>(0))
            .context("cannot read database; it may be encrypted, have an incorrect key, or use an unsupported legacy cipher")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enable database WAL")?;
        conn.execute_batch(SCHEMA)
            .context("initialize history schema")?;
        // Search text from an encrypted history must not spill to a plaintext
        // SQLite temporary file.
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.execute_batch(SEARCH_SCHEMA)
            .context("initialize search index")?;
        let data_version = conn.pragma_query_value(None, "data_version", |r| r.get(0))?;
        rebuild_search(&conn).context("populate search index")?;
        Ok(Store {
            conn: Mutex::new(conn),
            snips_attached: AtomicBool::new(false),
            search_data_version: AtomicI64::new(data_version),
            path: path.to_path_buf(),
        })
    }

    /// Attaches snippets.sdb as second database `snippets`; errors if already attached.
    pub fn attach_snippets(&self, path: &Path) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        if self.snips_attached.load(Ordering::Relaxed) {
            bail!("snippets database already attached");
        }
        // SQLCipher inherits the main database key for this attachment. This
        // avoids silently writing plaintext snippets for an encrypted history.
        conn.execute(
            "ATTACH DATABASE ?1 AS snippets",
            [path.to_string_lossy().as_ref()],
        )
        .with_context(|| format!("attach {}", path.display()))?;
        if let Err(error) = conn.execute_batch(SNIPPETS_SCHEMA) {
            let _ = conn.execute_batch("DETACH DATABASE snippets");
            return Err(error)
                .context("cannot read snippets database with the history database key");
        }
        self.snips_attached.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn snips(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        if !self.snips_attached.load(Ordering::Relaxed) {
            bail!("snippets database not attached");
        }
        Ok(self.conn.lock().unwrap())
    }

    /// Stores an event. Deduplicates against the last row's payload.
    /// Empty events (clipboard open failure, no recognized formats) are skipped.
    pub fn append(&self, ev: &ClipEvent) -> anyhow::Result<u64> {
        if ev.blocks.is_empty() {
            return Ok(0);
        }
        let payload = envelope::pack(&ev.blocks)?;
        if self.last_payload()?.as_ref() == Some(&payload) {
            return Ok(0);
        }
        let text = text_of(&ev.blocks);
        let (title, url, kind) = classify(&text, &ev.blocks);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
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
        let id = tx.last_insert_rowid();
        for block in &ev.blocks {
            match &block.format {
                Format::Registered(format_id, name) if !name.is_empty() => {
                    tx.execute(
                        "INSERT OR REPLACE INTO clipline_format_names(clip_id,format_id,name) VALUES(?1,?2,?3)",
                        params![id, format_id, name],
                    )?;
                }
                _ => {}
            }
        }
        tx.commit()?;
        Ok(id as u64)
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
        sql.push_str(" AND (?6 IS NULL OR id IN (SELECT rowid FROM temp.clipline_search WHERE title LIKE ?6 OR appname LIKE ?6 OR url LIKE ?6))");
        if f.from.is_some() {
            sql.push_str(" AND created>=?2");
        }
        if f.to.is_some() {
            sql.push_str(" AND created<=?3");
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?4 OFFSET ?5");
        let conn = self.conn.lock().unwrap();
        // TEMP triggers see this connection's changes; rebuild if another
        // connection (including the legacy app) modified the shared database.
        if f.query.is_some() {
            let version: i64 = conn.pragma_query_value(None, "data_version", |r| r.get(0))?;
            if version != self.search_data_version.load(Ordering::Relaxed) {
                rebuild_search(&conn)?;
                self.search_data_version.store(version, Ordering::Relaxed);
            }
        }
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                f.query.clone().unwrap_or_default(),
                f.from.clone().unwrap_or_default(),
                f.to.clone().unwrap_or_default(),
                f.limit.max(1),
                f.offset,
                f.query.as_deref().map(|query| format!("%{query}%"))
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
        let mut blocks = envelope::unpack(&blob)?;
        let mut stmt =
            conn.prepare("SELECT format_id,name FROM clipline_format_names WHERE clip_id=?1")?;
        let entries = stmt.query_map(params![id], |r| {
            Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
        })?;
        for entry in entries {
            let (format_id, name) = entry?;
            for block in &mut blocks {
                match &mut block.format {
                    Format::Registered(id, stored_name) if *id == format_id => {
                        stored_name.clone_from(&name)
                    }
                    _ => {}
                }
            }
        }
        Ok(blocks)
    }

    pub fn set_star(&self, id: i64, on: bool) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE log SET starred=?2 WHERE id=?1",
            params![id, on as i64],
        )?;
        Ok(())
    }

    /// Writes tags comma-wrapped (`,a,b,`; empty list → `,`) per interop.md.
    pub fn set_tags(&self, id: i64, tags: &[String]) -> anyhow::Result<()> {
        let mut wrapped = String::from(",");
        for t in tags {
            wrapped.push_str(t);
            wrapped.push(',');
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE log SET tags=?2 WHERE id=?1", params![id, wrapped])?;
        Ok(())
    }

    /// Return distinct labels in sorted order across the full history.
    pub fn list_all_tags(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT tags FROM log")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut tags = std::collections::BTreeSet::new();
        for row in rows {
            for tag in row?.split(',').filter(|tag| !tag.is_empty()) {
                tags.insert(tag.to_owned());
            }
        }
        Ok(tags.into_iter().collect())
    }

    /// Rename an exact label everywhere atomically, merging any duplicates.
    pub fn rename_tag(&self, old: &str, new: &str) -> anyhow::Result<()> {
        if old.is_empty() || old.contains(',') || new.trim().is_empty() || new.contains(',') {
            bail!("labels must be non-empty and cannot contain commas");
        }
        self.rewrite_tag(old, Some(new))
    }

    /// Remove an exact label from every row atomically.
    pub fn delete_tag(&self, tag: &str) -> anyhow::Result<()> {
        if tag.is_empty() || tag.contains(',') {
            bail!("labels must be non-empty and cannot contain commas");
        }
        self.rewrite_tag(tag, None)
    }

    fn rewrite_tag(&self, old: &str, replacement: Option<&str>) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = {
            let mut stmt = tx.prepare("SELECT id,tags FROM log")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (id, wrapped) in rows {
            let tags: Vec<&str> = wrapped.split(',').filter(|tag| !tag.is_empty()).collect();
            if !tags.contains(&old) {
                continue;
            }
            let mut updated: Vec<&str> = Vec::new();
            for tag in tags {
                let next = if tag == old { replacement } else { Some(tag) };
                if let Some(tag) = next.filter(|tag| !updated.contains(tag)) {
                    updated.push(tag);
                }
            }
            let new_tags = if updated.is_empty() {
                ",".to_owned()
            } else {
                format!(",{},", updated.join(","))
            };
            tx.execute("UPDATE log SET tags=?2 WHERE id=?1", params![id, new_tags])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&self, ids: &[i64]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        for id in ids {
            conn.execute("DELETE FROM log WHERE id=?1", params![id])?;
        }
        Ok(())
    }

    /// Ask SQLite to overwrite deleted content in database pages. This does
    /// not guarantee erasure from WAL files, snapshots, or storage hardware.
    pub fn set_secure_delete(&self, enabled: bool) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.pragma_update(None, "secure_delete", enabled)?;
        Ok(())
    }

    /// Remove all history rows. The snippets database is unaffected.
    pub fn clear_history(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM log", [])?;
        Ok(())
    }

    /// Checkpoint and truncate the WAL when no other connection holds it open.
    /// This is not a guarantee of secure erasure from underlying storage.
    pub fn checkpoint_truncate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            bail!("could not truncate WAL: another connection is using the database");
        }
        Ok(())
    }

    /// Deletes rows with `from <= created <= to` (inclusive, string comparison
    /// over `CURRENT_TIMESTAMP` format).
    pub fn delete_range(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM log WHERE created>=?1 AND created<=?2",
            params![from, to],
        )?;
        Ok(())
    }

    pub fn snippet_list(&self, pid: i64) -> anyhow::Result<Vec<Snippet>> {
        let conn = self.snips()?;
        let mut stmt = conn.prepare(
            "SELECT id,pid,pos,type,name,data,description
             FROM snippets.common WHERE pid=?1 ORDER BY pos,name",
        )?;
        let rows = stmt.query_map(params![pid], |r| {
            Ok(Snippet {
                id: r.get(0)?,
                pid: r.get(1)?,
                pos: r.get(2)?,
                kind: r.get(3)?,
                name: r.get(4)?,
                data: r.get(5)?,
                description: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Inserts a text snippet (type 2) under `pid`; returns its id.
    pub fn snippet_add(&self, pid: i64, name: &str, data: &str) -> anyhow::Result<i64> {
        let conn = self.snips()?;
        conn.execute(
            "INSERT INTO snippets.common(pid,name,data) VALUES(?1,?2,?3)",
            params![pid, name, data],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a folder (type 1) under `pid`.
    pub fn snippet_add_folder(&self, pid: i64, name: &str) -> anyhow::Result<i64> {
        let conn = self.snips()?;
        conn.execute(
            "INSERT INTO snippets.common(pid,type,name,data) VALUES(?1,1,?2,'')",
            params![pid, name],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn snippet_rename(&self, id: i64, name: &str) -> anyhow::Result<()> {
        let conn = self.snips()?;
        conn.execute(
            "UPDATE snippets.common SET name=?2 WHERE id=?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn snippet_move(&self, id: i64, new_pid: i64) -> anyhow::Result<()> {
        let conn = self.snips()?;
        if new_pid != 0 {
            let valid: bool = conn.query_row(
                "WITH RECURSIVE descendants(id) AS (
                    SELECT ?1 UNION SELECT c.id FROM snippets.common c JOIN descendants d ON c.pid=d.id
                 ) SELECT EXISTS(SELECT 1 FROM snippets.common WHERE id=?2 AND type=1)
                      AND NOT EXISTS(SELECT 1 FROM descendants WHERE id=?2)",
                params![id, new_pid], |row| row.get(0),
            )?;
            if !valid {
                bail!("destination must be a folder outside the moved subtree");
            }
        }
        conn.execute(
            "UPDATE snippets.common SET pid=?2 WHERE id=?1",
            params![id, new_pid],
        )?;
        Ok(())
    }

    pub fn snippet_delete(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.snips()?;
        conn.execute(
            "WITH RECURSIVE descendants(id) AS (
                SELECT ?1 UNION SELECT c.id FROM snippets.common c JOIN descendants d ON c.pid=d.id
             ) DELETE FROM snippets.common WHERE id IN (SELECT id FROM descendants)",
            params![id],
        )?;
        Ok(())
    }

    /// Copies every row from a legacy database into this one, preserving ids.
    pub fn import_legacy(&self, legacy: &Path) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        // SQLCipher otherwise inherits the destination's key for ATTACH;
        // legacy databases must be imported unlocked, never with a guessed key.
        #[cfg(feature = "encryption")]
        let attach_sql = "ATTACH DATABASE ?1 AS src KEY ''";
        #[cfg(not(feature = "encryption"))]
        let attach_sql = "ATTACH DATABASE ?1 AS src";
        conn.execute(attach_sql, [legacy.to_string_lossy().as_ref()])
            .context("cannot attach legacy database; locked legacy databases must be unlocked before import")?;
        let result = (|| -> anyhow::Result<u64> {
            conn.query_row("SELECT count(*) FROM src.sqlite_master", [], |row| row.get::<_, i64>(0))
                .context("legacy database cannot be read; locked legacy databases must be unlocked before import")?;
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let import = (|| -> anyhow::Result<u64> {
                let before: i64 = conn.query_row("SELECT count(*) FROM log", [], |r| r.get(0))?;
                conn.execute_batch(
                    "INSERT OR IGNORE INTO log SELECT * FROM src.log;
                     INSERT OR IGNORE INTO options SELECT * FROM src.options;",
                )?;
                let after: i64 = conn.query_row("SELECT count(*) FROM log", [], |r| r.get(0))?;
                Ok((after - before) as u64)
            })();
            match import {
                Ok(count) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(count)
                }
                Err(err) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(err)
                }
            }
        })();
        let detach = conn.execute_batch("DETACH DATABASE src");
        match result {
            Err(err) => Err(err),
            Ok(count) => {
                detach?;
                Ok(count)
            }
        }
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
            // as_chunks requires Rust 1.88; keep the Rust 1.85 MSRV.
            #[allow(clippy::chunks_exact_to_as_chunks)]
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
    if blocks
        .iter()
        .any(|b| matches!(b.format, Format::ImageDib | Format::ImageDibV5))
    {
        return ("Image".into(), String::new(), type_code::IMAGE);
    }
    if blocks.iter().any(|b| b.format == Format::FileDrop) {
        return ("Files".into(), String::new(), type_code::FILES);
    }
    let first_line = text.lines().next().unwrap_or_default().to_string();
    let text = text.trim();
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
