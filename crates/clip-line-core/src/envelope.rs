//! Storage envelope: gzip outside, block list inside.
//! Inner layout (from interop research, see docs/interop.md):
//! `01 00 <u16 block#> <u32le format id> <u32le len> <bytes>` per block.

use crate::capture::{Block, Format};
use anyhow::{Context, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::{Read, Write};

pub fn pack(blocks: &[Block]) -> anyhow::Result<Vec<u8>> {
    let mut inner = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        inner.extend_from_slice(&[0x01, 0x00]);
        inner.extend_from_slice(&((i as u16 + 1).to_le_bytes())); // ponytail: block# start assumed 1; verify against original app
        inner.extend_from_slice(&b.format.envelope_id().to_le_bytes());
        inner.extend_from_slice(&(b.bytes.len() as u32).to_le_bytes());
        inner.extend_from_slice(&b.bytes);
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&inner).context("gzip encode")?;
    gz.finish().context("gzip finish")
}

pub fn unpack(bytes: &[u8]) -> anyhow::Result<Vec<Block>> {
    let mut gz = GzDecoder::new(bytes);
    let mut inner = Vec::new();
    gz.read_to_end(&mut inner).context("gzip decode")?;
    let mut blocks = Vec::new();
    let mut p = 0usize;
    while p + 12 <= inner.len() {
        if inner[p] != 0x01 || inner[p + 1] != 0x00 {
            bail!("bad envelope magic at block {}", blocks.len());
        }
        let fmt = u32::from_le_bytes(inner[p + 4..p + 8].try_into()?);
        let len = u32::from_le_bytes(inner[p + 8..p + 12].try_into()?) as usize;
        let end = p + 12 + len;
        if end > inner.len() {
            bail!("truncated block payload");
        }
        blocks.push(Block {
            format: Format::from_envelope_id(fmt, None),
            bytes: inner[p + 12..end].to_vec(),
        });
        p = end;
    }
    Ok(blocks)
}
