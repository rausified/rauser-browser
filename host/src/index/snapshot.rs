//! The hand-written SQLite file-format writer for the `index.db` snapshot.

use super::*;

#[derive(Clone)]
pub(super) enum SqlValue {
    Null,
    Integer(i64),
    Text(String),
}

pub(super) struct SqlTable {
    pub(super) name: &'static str,
    pub(super) sql: &'static str,
    pub(super) rows: Vec<Vec<SqlValue>>,
}

pub(super) fn sqlite_snapshot(
    derived: &Derived,
    dirty: &BTreeSet<String>,
    events: &BTreeMap<String, CachedEvent>,
) -> Result<SqliteImage> {
    let artifacts = derived
        .artifacts
        .values()
        .map(|row| {
            vec![
                SqlValue::Text(row.artifact_id.clone()),
                SqlValue::Text(row.artifact_type.clone()),
                SqlValue::Text(row.site.clone()),
                SqlValue::Text(row.title.clone()),
                SqlValue::Text(row.url.clone()),
                SqlValue::Text(row.first_seen.clone()),
                SqlValue::Text(row.last_seen.clone()),
                sql_u64(row.focused_secs),
                sql_u64(row.background_secs),
                sql_u64(row.opens),
                sql_u64(row.days.len() as u64),
                row.last_mode.clone().map_or(SqlValue::Null, SqlValue::Text),
                row.why.clone().map_or(SqlValue::Null, SqlValue::Text),
            ]
        })
        .collect();
    // One row per logged visit: a container's copy would count it twice
    // in any per-day total a reader takes.
    let visits = derived
        .visits
        .iter()
        .filter(|row| !row.contained)
        .map(|row| {
            vec![
                SqlValue::Text(row.event_id.clone()),
                SqlValue::Text(row.artifact_id.clone()),
                SqlValue::Text(row.occurred_at.clone()),
                SqlValue::Text(row.local_date.clone()),
                sql_u64(row.focused_secs),
                sql_u64(row.background_secs),
                sql_u64(row.opens),
                row.mode.clone().map_or(SqlValue::Null, SqlValue::Text),
                SqlValue::Text(row.title.clone()),
                SqlValue::Text(row.url.clone()),
            ]
        })
        .collect();
    let edges = derived
        .edges
        .iter()
        .map(|row| {
            vec![
                SqlValue::Text(row.source.clone()),
                SqlValue::Text(row.target.clone()),
                SqlValue::Text(row.kind.clone()),
                sql_u64(row.weight),
                SqlValue::Text(row.last_seen.clone()),
            ]
        })
        .collect();
    let dirty_rows = dirty
        .iter()
        .map(|id| vec![SqlValue::Text(id.clone())])
        .collect();
    let provenance_rows = events
        .iter()
        .flat_map(|(id, event)| {
            event.url_records().into_iter().map(move |(role, stored)| {
                vec![
                    SqlValue::Text(id.clone()),
                    SqlValue::Text(event.artifact_id().into()),
                    SqlValue::Text(role),
                    SqlValue::Text(stored.url),
                    stored.stripped_params.map_or(SqlValue::Null, |names| {
                        SqlValue::Text(serde_json::to_string(&names).expect("query names"))
                    }),
                ]
            })
        })
        .collect();
    build_sqlite(&[
        SqlTable {
            name: "url_provenance",
            sql: "CREATE TABLE url_provenance(event_id TEXT,artifact_id TEXT,role TEXT,url TEXT,stripped_params TEXT)",
            rows: provenance_rows,
        },
        SqlTable {
            name: "artifacts",
            sql: "CREATE TABLE artifacts(artifact_id TEXT,artifact_type TEXT,site TEXT,title TEXT,url TEXT,first_seen TEXT,last_seen TEXT,focused_secs INTEGER,background_secs INTEGER,opens INTEGER,days_opened INTEGER,last_mode TEXT,why TEXT)",
            rows: artifacts,
        },
        SqlTable {
            name: "visits",
            sql: "CREATE TABLE visits(event_id TEXT,artifact_id TEXT,occurred_at TEXT,local_date TEXT,focused_secs INTEGER,background_secs INTEGER,opens INTEGER,mode TEXT,title TEXT,url TEXT)",
            rows: visits,
        },
        SqlTable {
            name: "edges",
            sql: "CREATE TABLE edges(source_artifact_id TEXT,target_artifact_id TEXT,kind TEXT,weight INTEGER,last_seen TEXT)",
            rows: edges,
        },
        SqlTable {
            name: "dirty_artifacts",
            sql: "CREATE TABLE dirty_artifacts(artifact_id TEXT)",
            rows: dirty_rows,
        },
    ])
}

pub(super) fn sql_u64(value: u64) -> SqlValue {
    SqlValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

/// SQLite stores a table-leaf payload of up to U-35 bytes (U is the usable
/// page size: the whole page, as no bytes are reserved) on the leaf itself.
/// A larger payload must spill to overflow pages, which this writer does
/// not emit, so such a row is left out instead of written unreadably.
pub(super) const SQLITE_MAX_LOCAL_PAYLOAD: usize = SQLITE_PAGE_SIZE - 35;

pub(super) struct SqliteImage {
    pub(super) bytes: Vec<u8>,
    /// Rows left out because their payload would need overflow pages.
    pub(super) skipped_rows: usize,
}

/// Emit the small subset of the SQLite file format this append-derived
/// snapshot needs: schema and rowid table B-trees, without secondary indexes
/// or overflow records. A 64 KiB page keeps every bounded event row local;
/// a row over the local payload limit is skipped and counted.
pub(super) fn build_sqlite(tables: &[SqlTable]) -> Result<SqliteImage> {
    let root_count = tables.len() + 1;
    let mut pages = vec![vec![0u8; SQLITE_PAGE_SIZE]; root_count];
    let mut next_page = root_count + 1;
    let mut skipped_rows = 0;
    for (table_index, table) in tables.iter().enumerate() {
        let root_page = table_index + 2;
        let mut cells = Vec::with_capacity(table.rows.len());
        for (index, row) in table.rows.iter().enumerate() {
            match table_leaf_cell((index + 1) as u64, row)? {
                Some(cell) => cells.push(cell),
                None => skipped_rows += 1,
            }
        }
        let chunks = partition_cells(&cells, 8)?;
        if chunks.len() <= 1 {
            write_leaf_page(
                &mut pages[root_page - 1],
                0,
                chunks.first().map(Vec::as_slice).unwrap_or(&[]),
            )?;
            continue;
        }
        let mut children = Vec::new();
        for chunk in chunks {
            let page_number = next_page;
            next_page += 1;
            let mut page = vec![0u8; SQLITE_PAGE_SIZE];
            write_leaf_page(&mut page, 0, &chunk)?;
            let max_rowid = decode_cell_rowid(chunk.last().expect("nonempty leaf chunk"))?;
            children.push((page_number as u32, max_rowid));
            pages.push(page);
        }
        write_interior_page(&mut pages[root_page - 1], &children)?;
    }

    let schema_rows = tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            vec![
                SqlValue::Text("table".into()),
                SqlValue::Text(table.name.into()),
                SqlValue::Text(table.name.into()),
                SqlValue::Integer((index + 2) as i64),
                SqlValue::Text(table.sql.into()),
            ]
        })
        .collect::<Vec<_>>();
    let schema_cells = schema_rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            table_leaf_cell((index + 1) as u64, row)?
                .context("SQLite schema row exceeds the local payload limit")
        })
        .collect::<Result<Vec<_>>>()?;
    write_leaf_page(&mut pages[0], 100, &schema_cells)?;
    let page_count = pages.len() as u32;
    write_database_header(&mut pages[0], page_count);
    Ok(SqliteImage {
        bytes: pages.into_iter().flatten().collect(),
        skipped_rows,
    })
}

pub(super) fn partition_cells(cells: &[Vec<u8>], header: usize) -> Result<Vec<Vec<Vec<u8>>>> {
    if cells.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    let mut chunks = Vec::<Vec<Vec<u8>>>::new();
    let mut current = Vec::new();
    let mut used = header;
    for cell in cells {
        if cell.len() + header + 2 > SQLITE_PAGE_SIZE {
            bail!("artifact index row exceeds one SQLite page");
        }
        if !current.is_empty() && used + cell.len() + 2 > SQLITE_PAGE_SIZE {
            chunks.push(current);
            current = Vec::new();
            used = header;
        }
        used += cell.len() + 2;
        current.push(cell.clone());
    }
    chunks.push(current);
    Ok(chunks)
}

pub(super) fn write_database_header(page: &mut [u8], pages: u32) {
    page[..16].copy_from_slice(b"SQLite format 3\0");
    page[16..18].copy_from_slice(&1u16.to_be_bytes()); // 1 means 65536 bytes.
    page[18] = 1;
    page[19] = 1;
    page[20] = 0;
    page[21] = 64;
    page[22] = 32;
    page[23] = 32;
    page[24..28].copy_from_slice(&1u32.to_be_bytes());
    page[28..32].copy_from_slice(&pages.to_be_bytes());
    page[40..44].copy_from_slice(&1u32.to_be_bytes());
    page[44..48].copy_from_slice(&4u32.to_be_bytes());
    page[56..60].copy_from_slice(&1u32.to_be_bytes());
    page[68..72].copy_from_slice(&0x4252_5752u32.to_be_bytes()); // BRWR
    page[92..96].copy_from_slice(&1u32.to_be_bytes());
    page[96..100].copy_from_slice(&3_045_000u32.to_be_bytes());
}

pub(super) fn write_leaf_page(page: &mut [u8], offset: usize, cells: &[Vec<u8>]) -> Result<()> {
    page[offset] = 0x0d;
    page[offset + 1..offset + 3].copy_from_slice(&0u16.to_be_bytes());
    page[offset + 3..offset + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
    let mut content = SQLITE_PAGE_SIZE;
    for (index, cell) in cells.iter().enumerate() {
        content = content
            .checked_sub(cell.len())
            .context("SQLite leaf page overflow")?;
        let pointer = offset + 8 + index * 2;
        if pointer + 2 > content {
            bail!("SQLite leaf page pointer overflow");
        }
        page[content..content + cell.len()].copy_from_slice(cell);
        page[pointer..pointer + 2].copy_from_slice(&(content as u16).to_be_bytes());
    }
    let encoded = if content == SQLITE_PAGE_SIZE {
        0
    } else {
        content as u16
    };
    page[offset + 5..offset + 7].copy_from_slice(&encoded.to_be_bytes());
    page[offset + 7] = 0;
    Ok(())
}

pub(super) fn write_interior_page(page: &mut [u8], children: &[(u32, u64)]) -> Result<()> {
    if children.len() < 2 {
        bail!("SQLite interior page needs at least two children");
    }
    page[0] = 0x05;
    page[1..3].copy_from_slice(&0u16.to_be_bytes());
    page[3..5].copy_from_slice(&((children.len() - 1) as u16).to_be_bytes());
    page[8..12].copy_from_slice(&children.last().expect("children checked").0.to_be_bytes());
    let mut content = SQLITE_PAGE_SIZE;
    for (index, (child, max_rowid)) in children[..children.len() - 1].iter().enumerate() {
        let mut cell = child.to_be_bytes().to_vec();
        cell.extend(sqlite_varint(*max_rowid));
        content = content
            .checked_sub(cell.len())
            .context("SQLite interior page overflow")?;
        let pointer = 12 + index * 2;
        if pointer + 2 > content {
            bail!("SQLite interior page pointer overflow");
        }
        page[content..content + cell.len()].copy_from_slice(&cell);
        page[pointer..pointer + 2].copy_from_slice(&(content as u16).to_be_bytes());
    }
    page[5..7].copy_from_slice(&(content as u16).to_be_bytes());
    page[7] = 0;
    Ok(())
}

/// One table-leaf cell, or `None` when its payload is too large to keep
/// local (`SQLITE_MAX_LOCAL_PAYLOAD`).
pub(super) fn table_leaf_cell(rowid: u64, values: &[SqlValue]) -> Result<Option<Vec<u8>>> {
    let payload = sqlite_record(values)?;
    if payload.len() > SQLITE_MAX_LOCAL_PAYLOAD {
        return Ok(None);
    }
    let mut cell = sqlite_varint(payload.len() as u64);
    cell.extend(sqlite_varint(rowid));
    cell.extend(payload);
    Ok(Some(cell))
}

pub(super) fn decode_cell_rowid(cell: &[u8]) -> Result<u64> {
    let (_, first) = read_sqlite_varint(cell)?;
    let (rowid, _) = read_sqlite_varint(&cell[first..])?;
    Ok(rowid)
}

pub(super) fn sqlite_record(values: &[SqlValue]) -> Result<Vec<u8>> {
    let mut serial = Vec::new();
    let mut body = Vec::new();
    for value in values {
        match value {
            SqlValue::Null => serial.extend(sqlite_varint(0)),
            SqlValue::Integer(value) => {
                serial.extend(sqlite_varint(6));
                body.extend(value.to_be_bytes());
            }
            SqlValue::Text(value) => {
                let bytes = value.as_bytes();
                serial.extend(sqlite_varint(13 + 2 * bytes.len() as u64));
                body.extend(bytes);
            }
        }
    }
    let mut header_len = serial.len() + 1;
    loop {
        let encoded = sqlite_varint(header_len as u64);
        let next = serial.len() + encoded.len();
        if next == header_len {
            let mut record = encoded;
            record.extend(serial);
            record.extend(body);
            return Ok(record);
        }
        header_len = next;
    }
}

pub(super) fn sqlite_varint(value: u64) -> Vec<u8> {
    if value <= 0x7f {
        return vec![value as u8];
    }
    if value > 0x00ff_ffff_ffff_ffff {
        let mut out = vec![0u8; 9];
        out[8] = value as u8;
        let mut shifted = value >> 8;
        for index in (0..8).rev() {
            out[index] = ((shifted & 0x7f) as u8) | 0x80;
            shifted >>= 7;
        }
        return out;
    }
    let bits = 64 - value.leading_zeros() as usize;
    let count = bits.div_ceil(7);
    let mut out = vec![0u8; count];
    for (index, byte) in out.iter_mut().enumerate() {
        let shift = (count - index - 1) * 7;
        *byte = ((value >> shift) & 0x7f) as u8;
        if index + 1 != count {
            *byte |= 0x80;
        }
    }
    out
}

pub(super) fn read_sqlite_varint(bytes: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().copied().take(9).enumerate() {
        if index == 8 {
            return Ok(((value << 8) | u64::from(byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    bail!("invalid SQLite varint")
}
