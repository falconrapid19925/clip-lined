# Interop notes: ClipDiary database format

This file records interface-level facts about Softvoile ClipDiary 5.7's on-disk
format, established by observation of a lawfully installed copy for
interoperability. clipLine is an independent implementation and is not
affiliated with, endorsed by, or derived from ClipDiary. No decompiled code is
included; this document is clean-room documentation of a file format.

## Locations

- History database: `%APPDATA%\Clipdiary\diary.db` (SQLite, WAL)
- Snippets database: `%APPDATA%\Clipdiary\snippets.sdb`
- Config: `%APPDATA%\Clipdiary\clipdiary.cfg` (INI)

## Schema (verbatim)

```sql
CREATE TABLE log(
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

CREATE TABLE options(key CHAR(30) PRIMARY KEY not null default '', value TEXT not null default '');

CREATE TABLE common(            -- snippets.sdb
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  pid INTEGER not null default 0,   -- parent id, tree structure
  pos INTEGER not null default 1000,
  type INTEGER not null default 2,  -- 2 = text snippet, 0 = placeholder
  name TEXT not null default '',
  data TEXT not null default '',
  description TEXT not null default '');
```

## Payload envelope

Every `log.data` value is a gzip stream (`1F 8B`). Inside, after gunzip, a
block list:

```
01 00            magic
u16 le           block number, starting at 1
u32 le           clipboard format id (CF_ value or registered id)
u32 le           payload length
bytes            payload
```

Blocks repeat back to back. A clip may carry several blocks (e.g. a URL clip
with an HTML Format companion block).

## Type codes (log.type)

| type | meaning |
|------|---------|
| 0 | registered image format (PNG-like, screen capture tools) |
| 1 | plain text (CF_UNICODETEXT) |
| 2 | secondary text block (rare) |
| 3 | URL (often with HTML companion) |
| 4 | file drop list (CF_HDROP) |
| 5 | image (CF_DIB, often with PNG twin) |

## Field conventions

- `title` truncated to 55 chars, `appname` 20, `apptitle` 100
- `tags` comma-wrapped list: `,Important,Work,`
- `datasize == LENGTH(data)` holds for every observed row

## Compatibility goal

Rows written by clipLine keep this shape so both programs can read the same
history database. Block numbering (`start at 1`) was inferred from samples and
is the one field still to verify against a wider corpus.
