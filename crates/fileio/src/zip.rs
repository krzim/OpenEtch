//! A small zip reader and writer (APPNOTE.TXT, PKWARE): enough for 3MF models and OpenEtch user bundles. Read:
//! stored and deflated entries through the central directory (no zip64, no encryption), with size limits against zip
//! bombs. Written: deflated entries (stored when deflate does not help), UTF-8 names.

use crate::{FileError, Result, crc32, invalid};

/// Largest total uncompressed size read from one archive.
pub const MAX_TOTAL: u64 = 512 * 1024 * 1024;
/// Most entries read.
pub const MAX_ENTRIES: usize = 10_000;

/// One file of an archive.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn u32_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?)))
}

fn bad<T>() -> Result<T> {
    invalid("the zip archive is damaged")
}

/// Whether `bytes` look like a zip archive.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

/// The names of the entries, without reading their data.
pub fn names(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(directory(bytes)?.into_iter().map(|d| d.name).collect())
}

struct Dir {
    name: String,
    method: usize,
    crc: u32,
    compressed: u64,
    size: u64,
    local: u64,
}

fn directory(bytes: &[u8]) -> Result<Vec<Dir>> {
    // End of central directory: the last "PK\5\6" within the final 64 KiB + 22 bytes.
    let from = bytes.len().saturating_sub(65_535 + 22);
    let eocd = (from..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))
        .ok_or_else(|| FileError::Invalid("not a zip archive".into()))?;
    let count = u16_at(bytes, eocd + 10).ok_or(()).or_else(|_| bad())?;
    let mut at = u32_at(bytes, eocd + 16).ok_or(()).or_else(|_| bad())? as usize;
    if count > MAX_ENTRIES {
        return Err(FileError::TooLarge(format!("the archive has more than {MAX_ENTRIES} entries")));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if !bytes.get(at..).is_some_and(|b| b.starts_with(b"PK\x01\x02")) {
            return bad();
        }
        let g = |o: usize| u16_at(bytes, at + o).ok_or(());
        let w = |o: usize| u32_at(bytes, at + o).ok_or(());
        let (Ok(method), Ok(crc), Ok(compressed), Ok(size), Ok(nlen), Ok(xlen), Ok(clen), Ok(local)) =
            (g(10), w(16), w(20), w(24), g(28), g(30), g(32), w(42))
        else {
            return bad();
        };
        let name_at = at.checked_add(46).ok_or(()).or_else(|_| bad())?;
        let name = bytes.get(name_at..name_at.saturating_add(nlen)).ok_or(()).or_else(|_| bad())?;
        out.push(Dir { name: String::from_utf8_lossy(name).replace('\\', "/"), method, crc: crc as u32, compressed, size, local });
        at = name_at.saturating_add(nlen + xlen + clen);
    }
    Ok(out)
}

/// Read every file entry (directories are skipped).
pub fn read(bytes: &[u8]) -> Result<Vec<Entry>> {
    let mut total = 0u64;
    // Every file entry with its local header's offset and its data's range, checked before anything is unpacked.
    let mut located: Vec<(Dir, usize, usize, usize)> = Vec::new();
    for d in directory(bytes)? {
        if d.name.ends_with('/') {
            continue;
        }
        total = total.saturating_add(d.size);
        if total > MAX_TOTAL {
            return Err(FileError::TooLarge("the archive unpacks to more than 512 MB".into()));
        }
        let at = usize::try_from(d.local).ok().filter(|&a| bytes.get(a..).is_some_and(|b| b.starts_with(b"PK\x03\x04")));
        let Some(at) = at else { return bad() };
        let (Some(nlen), Some(xlen)) = (u16_at(bytes, at + 26), u16_at(bytes, at + 28)) else { return bad() };
        let start = at + 30 + nlen + xlen;
        let end = usize::try_from(d.compressed).ok().and_then(|c| start.checked_add(c)).filter(|&e| e <= bytes.len());
        let Some(end) = end else { return bad() };
        located.push((d, at, start, end));
    }
    // An "overlapping" zip bomb points many entries at one compressed block: each entry's header and data must have
    // bytes of their own.
    let mut spans: Vec<(usize, usize)> = located.iter().map(|(_, at, _, end)| (*at, *end)).collect();
    spans.sort_unstable();
    if spans.windows(2).any(|w| w[1].0 < w[0].1) {
        return invalid("the zip archive's entries overlap (a zip bomb?)");
    }
    let mut out = Vec::new();
    for (d, _, start, end) in located {
        let raw = &bytes[start..end];
        let data = match d.method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, d.size as usize)
                .map_err(|_| FileError::Invalid(format!("{} cannot be unpacked", d.name)))?,
            m => return Err(FileError::Unsupported(format!("{}: compression method {m} is not supported", d.name))),
        };
        if data.len() as u64 != d.size || crc32(&data) != d.crc {
            return invalid(format!("{} is damaged (size or checksum)", d.name));
        }
        out.push(Entry { name: d.name, data });
    }
    Ok(out)
}

/// One entry by name (exact, else ignoring case).
pub fn read_one(bytes: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    let all = read(bytes)?;
    Ok(all
        .iter()
        .find(|e| e.name == name)
        .or_else(|| all.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
        .map(|e| e.data.clone()))
}

/// Write entries as a zip archive.
pub fn write(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        if e.name.is_empty() || e.name.len() > 0xFFFF || e.data.len() > u32::MAX as usize {
            return invalid(format!("cannot store {:?} in a zip archive", e.name));
        }
        let crc = crc32(&e.data);
        let packed = miniz_oxide::deflate::compress_to_vec(&e.data, 6);
        let (method, body): (u16, &[u8]) = if packed.len() < e.data.len() { (8, &packed) } else { (0, &e.data) };
        let offset = u32::try_from(out.len()).map_err(|_| FileError::TooLarge("the archive would exceed 4 GB".into()))?;
        let name = e.name.as_bytes();
        let header = |sig: &[u8], central: bool| {
            let mut h = sig.to_vec();
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
                h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name);
            h
        };
        out.extend(header(b"PK\x03\x04", false));
        out.extend_from_slice(body);
        central.extend(header(b"PK\x01\x02", true));
    }
    let dir_at = out.len() as u32;
    let n = entries.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_stored_and_deflated() {
        let entries = vec![
            Entry { name: "a.txt".into(), data: b"hi".to_vec() },
            Entry { name: "dir/b.json".into(), data: vec![b'x'; 10_000] },
        ];
        let z = write(&entries).unwrap();
        assert!(is_zip(&z));
        assert_eq!(read(&z).unwrap(), entries);
        assert_eq!(names(&z).unwrap(), ["a.txt", "dir/b.json"]);
        assert_eq!(read_one(&z, "DIR/B.JSON").unwrap().unwrap().len(), 10_000);
        assert!(read_one(&z, "c").unwrap().is_none());
    }

    /// Many directory records pointing at one compressed block (the "overlapping" zip bomb: a few kilobytes that
    /// unpack to the 512 MB limit) are refused before anything is unpacked.
    #[test]
    fn overlapping_entries_are_refused() {
        let one = write(&[Entry { name: "a".into(), data: vec![b'x'; 1 << 20] }]).unwrap();
        let eocd = one.len() - 22;
        let dir_at = u32::from_le_bytes(one[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let record = &one[dir_at..eocd];
        let n = 400u16;
        let mut bomb = one[..dir_at].to_vec();
        for _ in 0..n {
            bomb.extend_from_slice(record);
        }
        bomb.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        bomb.extend_from_slice(&n.to_le_bytes());
        bomb.extend_from_slice(&n.to_le_bytes());
        bomb.extend_from_slice(&((record.len() * usize::from(n)) as u32).to_le_bytes());
        bomb.extend_from_slice(&(dir_at as u32).to_le_bytes());
        bomb.extend_from_slice(&0u16.to_le_bytes());
        assert!(bomb.len() < 40_000, "{}", bomb.len());
        assert_eq!(names(&bomb).unwrap().len(), 400);
        let e = read(&bomb).unwrap_err();
        assert!(e.to_string().contains("overlap"), "{e}");
        // A directory record pointing past the end, or at an offset that overflows, is damage, not a panic.
        let mut far = one.clone();
        far[dir_at + 42..dir_at + 46].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&far).is_err());
        let mut long = one.clone();
        long[dir_at + 20..dir_at + 24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&long).is_err());
    }

    #[test]
    fn damage_is_detected() {
        let mut z = write(&[Entry { name: "a".into(), data: b"hello world".to_vec() }]).unwrap();
        assert!(read(b"not a zip").is_err());
        // Flip a data byte (stored: "hello world" after the 31-byte header).
        z[31] ^= 0xFF;
        assert!(read(&z).is_err());
    }
}
