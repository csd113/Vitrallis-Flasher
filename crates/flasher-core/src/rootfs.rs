//! Bounded rootfs tar inspection. No extraction, installation or NAND authority.
use crate::{Cancellation, Error};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{BufReader, Read},
};

const BLOCK: usize = 512;
const MAX_ENTRIES: usize = 200_000;
const MAX_DATA: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_METADATA: usize = 32 * 1024 * 1024;
const MAX_PATH: usize = 4096;
const MAX_EXTENSION: usize = 8192;

/// Metadata classes supported by the locked stock archive and reviewed repacker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Symlink,
    Hardlink,
    Character,
}
impl Kind {
    const fn code(self) -> u8 {
        match self {
            Self::Directory => 1,
            Self::File => 2,
            Self::Symlink => 3,
            Self::Hardlink => 4,
            Self::Character => 5,
        }
    }
}

/// Inspected data only. Paths are archive-relative; callers must never follow
/// symlinks when installing beneath a root. This value grants no write authority.
#[derive(Debug)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub link: String,
    pub major: u32,
    pub minor: u32,
    pub size: u64,
    pub content_sha256: [u8; 32],
}

/// Complete inspection result; success requires headers, contents and terminal
/// padding to pass. It is not a verified asset or installation capability.
#[derive(Debug)]
pub struct Inspection {
    entries: Vec<Entry>,
    pub file_bytes: u64,
    pub semantic_sha256: [u8; 32],
}
impl Inspection {
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

const fn reject(reason: &'static str) -> Error {
    Error::RootfsArchive(reason)
}

struct Input<R> {
    reader: R,
    bytes: u64,
}
impl<R: Read> Input<R> {
    fn read_chunk(&mut self, bytes: &mut [u8], cancel: &Cancellation) -> Result<usize, Error> {
        loop {
            cancel.check()?;
            let count = match self.reader.read(bytes) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    cancel.check()?;
                    return Err(error.into());
                }
            };
            cancel.check()?;
            if count > bytes.len() {
                return Err(Error::Length);
            }
            self.bytes = self.bytes.checked_add(count as u64).ok_or(Error::Length)?;
            // Expanded file data plus bounded headers/extensions and end padding.
            if self.bytes > MAX_DATA + 2 * MAX_METADATA as u64 {
                return Err(reject("expanded archive bound"));
            }
            return Ok(count);
        }
    }
    fn exact(&mut self, bytes: &mut [u8], cancel: &Cancellation) -> Result<(), Error> {
        cancel.check()?;
        let mut offset = 0;
        while offset < bytes.len() {
            let count = self.read_chunk(&mut bytes[offset..], cancel)?;
            if count == 0 {
                return Err(reject("truncated tar stream"));
            }
            offset += count;
        }
        cancel.check()
    }
    fn padding(&mut self, size: u64, cancel: &Cancellation) -> Result<(), Error> {
        let count = usize::try_from((BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64)
            .map_err(|_| Error::Length)?;
        let mut padding = [0; BLOCK];
        self.exact(&mut padding[..count], cancel)?;
        if padding[..count].iter().any(|b| *b != 0) {
            return Err(reject("nonzero entry padding"));
        }
        Ok(())
    }
    fn finish(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        let mut second = [0; BLOCK];
        self.exact(&mut second, cancel)?;
        if second.iter().any(|b| *b != 0) {
            return Err(reject("missing second terminal block"));
        }
        // Standard tar block-factor padding is permitted, concatenated payloads are not.
        let mut padding = [0; BLOCK];
        let mut trailing = 0;
        loop {
            let count = self.read_chunk(&mut padding, cancel)?;
            if count == 0 {
                if trailing % BLOCK != 0 {
                    return Err(reject("partial terminal padding block"));
                }
                return Ok(());
            }
            trailing += count;
            if trailing > 10240 || padding[..count].iter().any(|b| *b != 0) {
                return Err(reject("trailing archive payload or padding bound"));
            }
        }
    }
}

/// Inspect one gzip-compressed rootfs without buffering the archive or starting
/// a process.
///
/// Only the fixed ten-byte, no-optional-fields gzip header emitted by
/// the locked image pipeline is supported. The compressed source is bounded to
/// the manifest asset limit (2 GiB); expanded data/metadata bounds are enforced
/// by the tar inspector.
/// Providers must separately bound blocking reads. This grants no asset trust
/// or installation authority.
/// # Errors
/// Rejects unsupported headers, invalid CRC/size trailers, truncation, trailing
/// bytes/concatenated members, malformed tar, source bounds and cancellation.
pub fn inspect_gzip(reader: impl Read, cancel: &Cancellation) -> Result<Inspection, Error> {
    cancel.check()?;
    let mut input = Input { reader, bytes: 0 };
    let mut header = [0; 10];
    input.exact(&mut header, cancel)?;
    if header[..4] != [0x1f, 0x8b, 8, 0] {
        return Err(reject("unsupported gzip header"));
    }
    // Check flags before constructing the decoder: optional name/comment/extra
    // fields otherwise permit allocations unrelated to the tar metadata bound.
    let source = Compressed {
        reader: input.reader,
        bytes: 10,
        cancel,
    };
    let source = header.as_slice().chain(source);
    let mut decoder = flate2::bufread::GzDecoder::new(BufReader::with_capacity(8192, source));
    let inspected = inspect(&mut decoder, cancel);
    cancel.check()?;
    let inspected = inspected?;
    let mut source = decoder.into_inner();
    let mut trailing = Input {
        reader: &mut source,
        bytes: 0,
    };
    if trailing.read_chunk(&mut [0; 1], cancel)? != 0 {
        return Err(reject("trailing compressed payload"));
    }
    cancel.check()?;
    Ok(inspected)
}

struct Compressed<'a, R> {
    reader: R,
    bytes: u64,
    cancel: &'a Cancellation,
}
impl<R: Read> Read for Compressed<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.cancel.check().map_err(std::io::Error::other)?;
        let count = self.reader.read(buffer)?;
        self.cancel.check().map_err(std::io::Error::other)?;
        if count > buffer.len() {
            return Err(std::io::Error::other("invalid compressed reader count"));
        }
        self.bytes = self
            .bytes
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("compressed source overflow"))?;
        if self.bytes > crate::manifest::MAX_ASSET_BYTES {
            return Err(std::io::Error::other("compressed source bound"));
        }
        Ok(count)
    }
}

/// Inspect an already decompressed tar stream with bounded content/metadata memory.
/// Cancellation is checked between reads; the provider must bound blocking reads.
/// # Errors
/// Rejects truncation, malformed headers/extensions, unsafe topology, unsupported
/// metadata/device nodes, expansion bounds and cancellation. No filesystem mutation occurs.
pub fn inspect(reader: impl Read, cancel: &Cancellation) -> Result<Inspection, Error> {
    let mut input = Input { reader, bytes: 0 };
    let mut extensions = Extensions::default();
    let mut entries = Vec::new();
    let mut paths = BTreeMap::new();
    let mut metadata = 0_usize;
    let mut file_bytes = 0_u64;
    let mut semantic = Sha256::new();
    loop {
        let mut header = [0; BLOCK];
        input.exact(&mut header, cancel)?;
        if header.iter().all(|b| *b == 0) {
            if entries.is_empty() || extensions.name.is_some() || extensions.link.is_some() {
                return Err(reject("empty archive or orphan extension"));
            }
            input.finish(cancel)?;
            validate_ancestors(&paths, cancel)?;
            cancel.check()?;
            return Ok(Inspection {
                entries,
                file_bytes,
                semantic_sha256: semantic.finalize().into(),
            });
        }
        validate_header(&header)?;
        if matches!(header[156], b'L' | b'K' | b'x') {
            extensions.read(&header, &mut input, cancel)?;
            continue;
        }
        let (name, mut entry) = parse_entry(&header, &mut extensions)?;
        if paths.contains_key(&entry.path) || entries.len() >= MAX_ENTRIES {
            return Err(reject("duplicate path or entry bound"));
        }
        metadata = metadata
            .checked_add(std::mem::size_of::<Entry>() + 2 * entry.path.len() + entry.link.len())
            .ok_or(Error::Length)?;
        if metadata > MAX_METADATA {
            return Err(reject("metadata memory bound"));
        }
        validate_link(&entry, &paths)?;
        file_bytes = file_bytes.checked_add(entry.size).ok_or(Error::Length)?;
        if file_bytes > MAX_DATA {
            return Err(reject("expanded file bound"));
        }
        entry.content_sha256 = content(&mut input, entry.size, cancel)?;
        if entry.kind != Kind::File {
            entry.content_sha256 = [0; 32];
        }
        canonical(&mut semantic, &name, &entry)?;
        paths.insert(entry.path.clone(), entry.kind);
        entries.push(entry);
    }
}

fn content<R: Read>(
    input: &mut Input<R>,
    size: u64,
    cancel: &Cancellation,
) -> Result<[u8; 32], Error> {
    let mut bytes = [0; 8192];
    let mut left = size;
    let mut hash = Sha256::new();
    while left > 0 {
        let count = usize::try_from(left.min(bytes.len() as u64)).map_err(|_| Error::Length)?;
        input.exact(&mut bytes[..count], cancel)?;
        hash.update(&bytes[..count]);
        left -= count as u64;
    }
    input.padding(size, cancel)?;
    Ok(hash.finalize().into())
}

fn number(bytes: &[u8]) -> Result<u64, Error> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| reject("non-octal numeric field"))?
        .trim_matches(['\0', ' ']);
    if value.is_empty() {
        return Ok(0);
    }
    if !value.bytes().all(|b| matches!(b, b'0'..=b'7')) {
        return Err(reject("non-octal numeric field"));
    }
    u64::from_str_radix(value, 8).map_err(|_| reject("numeric overflow"))
}
fn integer(bytes: &[u8]) -> Result<u32, Error> {
    u32::try_from(number(bytes)?).map_err(|_| reject("numeric field bound"))
}
fn text(bytes: &[u8]) -> Result<String, Error> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|b| *b != 0) {
        return Err(reject("nonzero bytes after string terminator"));
    }
    std::str::from_utf8(&bytes[..end])
        .map(str::to_owned)
        .map_err(|_| reject("non-UTF8 metadata"))
}
fn validate_header(header: &[u8; BLOCK]) -> Result<(), Error> {
    if &header[257..265] != b"ustar  \0" && &header[257..265] != b"ustar\0\x30\x30" {
        return Err(reject("unsupported tar header format"));
    }
    let checksum: u64 = header
        .iter()
        .enumerate()
        .map(|(i, b)| u64::from(if (148..156).contains(&i) { b' ' } else { *b }))
        .sum();
    if number(&header[148..156])? != checksum {
        return Err(reject("header checksum"));
    }
    Ok(())
}
fn path(name: &str) -> Result<String, Error> {
    let stripped = name.strip_prefix("./").unwrap_or(name);
    if stripped == "." {
        return Ok(stripped.into());
    }
    if stripped.is_empty()
        || stripped.len() > MAX_PATH
        || stripped.contains(['\0', '\\'])
        || stripped.split('/').any(|p| matches!(p, "" | "." | ".."))
    {
        return Err(reject("unsafe archive-relative path"));
    }
    Ok(stripped.into())
}
fn parse_entry(
    header: &[u8; BLOCK],
    extensions: &mut Extensions,
) -> Result<(String, Entry), Error> {
    let kind = match header[156] {
        b'0' => Kind::File,
        b'5' => Kind::Directory,
        b'2' => Kind::Symlink,
        b'1' => Kind::Hardlink,
        b'3' => Kind::Character,
        _ => return Err(reject("unsupported entry type")),
    };
    // A GNU/PAX name supersedes the short field, which can end mid UTF-8 codepoint.
    let mut name = match extensions.name.take() {
        Some(name) => name,
        None => header_name(header)?,
    };
    if kind == Kind::Directory {
        name = name.trim_end_matches('/').into();
    }
    let normalized = path(&name)?;
    if normalized == "." && kind != Kind::Directory {
        return Err(reject("root must be a directory"));
    }
    let link = match extensions.link.take() {
        Some(link) => link,
        None => text(&header[157..257])?,
    };
    let size = number(&header[124..136])?;
    let mode = integer(&header[100..108])?;
    if size > MAX_FILE || (kind != Kind::File && size != 0) || mode > 0o7777 {
        return Err(reject("entry size or mode bound"));
    }
    let entry = Entry {
        path: normalized,
        kind,
        mode,
        uid: integer(&header[108..116])?,
        gid: integer(&header[116..124])?,
        link,
        major: integer(&header[329..337])?,
        minor: integer(&header[337..345])?,
        size,
        content_sha256: [0; 32],
    };
    if kind == Kind::Character {
        validate_device(&entry)?;
    } else if entry.major != 0 || entry.minor != 0 {
        return Err(reject("unexpected device numbers"));
    }
    Ok((name, entry))
}

fn header_name(header: &[u8; BLOCK]) -> Result<String, Error> {
    let name = text(&header[..100])?;
    if &header[257..263] == b"ustar\0" {
        let prefix = text(&header[345..500])?;
        if !prefix.is_empty() {
            return Ok(format!("{prefix}/{name}"));
        }
    }
    Ok(name)
}

fn validate_device(entry: &Entry) -> Result<(), Error> {
    let expected = match entry.path.as_str() {
        "dev/random" => (1, 8),
        "dev/console" => (5, 1),
        "dev/ptmx" => (5, 2),
        "dev/full" => (1, 7),
        "dev/urandom" => (1, 9),
        "dev/null" => (1, 3),
        "dev/tty" => (5, 0),
        "dev/zero" => (1, 5),
        _ => return Err(reject("unreviewed character device")),
    };
    if (entry.major, entry.minor) != expected || entry.uid != 0 || entry.gid != 0 {
        return Err(reject("changed character device identity"));
    }
    Ok(())
}
fn validate_link(entry: &Entry, paths: &BTreeMap<String, Kind>) -> Result<(), Error> {
    match entry.kind {
        Kind::Hardlink => {
            if paths.get(&path(&entry.link)?) != Some(&Kind::File) {
                return Err(reject("hardlink must target an earlier regular file"));
            }
        }
        Kind::Symlink => {
            if entry.link.is_empty()
                || entry.link.len() > MAX_PATH
                || entry.link.contains(['\0', '\\'])
            {
                return Err(reject("unsafe symlink target"));
            }
            let mut depth = if entry.link.starts_with('/') {
                0
            } else {
                entry.path.split('/').count() - 1
            };
            for component in entry.link.split('/') {
                match component {
                    "" | "." => {}
                    ".." => {
                        depth = depth
                            .checked_sub(1)
                            .ok_or_else(|| reject("symlink escapes root"))?;
                    }
                    _ => depth += 1,
                }
            }
        }
        _ if !entry.link.is_empty() => return Err(reject("unexpected link target")),
        _ => {}
    }
    Ok(())
}
fn validate_ancestors(paths: &BTreeMap<String, Kind>, cancel: &Cancellation) -> Result<(), Error> {
    for name in paths.keys() {
        cancel.check()?;
        for (offset, _) in name.match_indices('/') {
            if paths.get(&name[..offset]) != Some(&Kind::Directory) {
                return Err(reject("missing or non-directory ancestor"));
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Extensions {
    name: Option<String>,
    link: Option<String>,
}
impl Extensions {
    fn read<R: Read>(
        &mut self,
        header: &[u8; BLOCK],
        input: &mut Input<R>,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        let size = usize::try_from(number(&header[124..136])?).map_err(|_| Error::Length)?;
        if size == 0 || size > MAX_EXTENSION {
            return Err(reject("extension bound"));
        }
        let mut bytes = vec![0; size];
        input.exact(&mut bytes, cancel)?;
        input.padding(size as u64, cancel)?;
        if matches!(header[156], b'L' | b'K') && bytes.last() != Some(&0) {
            return Err(reject("unterminated GNU extension"));
        }
        match header[156] {
            b'L' => Self::assign(&mut self.name, text(&bytes)?)?,
            b'K' => Self::assign(&mut self.link, text(&bytes)?)?,
            b'x' => self.pax(&bytes)?,
            _ => return Err(reject("unsupported extension")),
        }
        Ok(())
    }
    fn assign(slot: &mut Option<String>, value: String) -> Result<(), Error> {
        if slot.is_some() || value.is_empty() || value.len() > MAX_PATH {
            return Err(reject("duplicate or oversized extension field"));
        }
        *slot = Some(value);
        Ok(())
    }
    fn pax(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            let separator = bytes
                .iter()
                .position(|b| *b == b' ')
                .ok_or_else(|| reject("PAX record length"))?;
            let number = std::str::from_utf8(&bytes[..separator])
                .map_err(|_| reject("PAX record length"))?;
            if !number.bytes().all(|b| b.is_ascii_digit()) {
                return Err(reject("PAX record length"));
            }
            let length: usize = number.parse().map_err(|_| reject("PAX record length"))?;
            if length <= separator + 2 || length > bytes.len() || bytes[length - 1] != b'\n' {
                return Err(reject("PAX truncated record"));
            }
            let record = std::str::from_utf8(&bytes[separator + 1..length - 1])
                .map_err(|_| reject("PAX non-UTF8 record"))?;
            let (key, value) = record.split_once('=').ok_or_else(|| reject("PAX field"))?;
            match key {
                "path" => Self::assign(&mut self.name, value.into())?,
                "linkpath" => Self::assign(&mut self.link, value.into())?,
                _ => return Err(reject("unsupported PAX metadata")),
            }
            bytes = &bytes[length..];
        }
        Ok(())
    }
}
fn canonical(hash: &mut Sha256, name: &str, entry: &Entry) -> Result<(), Error> {
    // Same length-prefixed metadata/content encoding as images/repack.py.
    for bytes in [
        name.as_bytes(),
        &[entry.kind.code()],
        &entry.mode.to_be_bytes(),
        &entry.uid.to_be_bytes(),
        &entry.gid.to_be_bytes(),
        entry.link.as_bytes(),
        &entry.major.to_be_bytes(),
        &entry.minor.to_be_bytes(),
        &entry.size.to_be_bytes(),
        &entry.content_sha256,
    ] {
        hash.update(
            u32::try_from(bytes.len())
                .map_err(|_| Error::Length)?
                .to_be_bytes(),
        );
        hash.update(bytes);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, kind: u8, size: u64, link: &str) -> [u8; BLOCK] {
        let mut h = [0; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000755\0");
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[156] = kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..265].copy_from_slice(b"ustar  \0");
        checksum(&mut h);
        h
    }
    fn checksum(h: &mut [u8; BLOCK]) {
        h[148..156].fill(b' ');
        let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    }
    fn member(name: &str, kind: u8, data: &[u8], link: &str) -> Vec<u8> {
        let mut result = header(name, kind, data.len() as u64, link).to_vec();
        result.extend_from_slice(data);
        result.resize(result.len().next_multiple_of(BLOCK), 0);
        result
    }
    fn archive(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut result = entries.concat();
        result.extend_from_slice(&[0; 2 * BLOCK]);
        result
    }
    fn scan(bytes: &[u8]) -> Result<Inspection, Error> {
        inspect(bytes, &Cancellation::default())
    }

    #[test]
    fn complete_tree_preserves_links_numeric_ownership_modes_and_content_hash() {
        let mut data = member("./file", b'0', b"contents", "");
        data[108..116].copy_from_slice(b"0001750\0"); // uid 1000
        data[116..124].copy_from_slice(b"0001750\0");
        let mut h: [u8; BLOCK] = data[..BLOCK].try_into().unwrap();
        checksum(&mut h);
        data[..BLOCK].copy_from_slice(&h);
        let tree = scan(&archive(&[
            member("./", b'5', b"", ""),
            data,
            member("./alias", b'1', b"", "./file"),
            member("./link", b'2', b"", "/file"),
        ]))
        .unwrap();
        assert_eq!(tree.file_bytes, 8);
        assert_eq!(tree.entries().len(), 4);
        let file = &tree.entries()[1];
        assert_eq!((file.uid, file.gid, file.mode), (1000, 1000, 0o755));
        assert_eq!(
            file.content_sha256,
            <[u8; 32]>::from(Sha256::digest(b"contents"))
        );
        assert_eq!(tree.entries()[2].kind, Kind::Hardlink);
        assert_eq!(tree.entries()[3].link, "/file");
    }

    #[test]
    fn gnu_long_name_and_pax_path_are_supported_without_extracting_extensions() {
        let name = format!("./usr/{}", "n".repeat(130));
        let mut long = name.as_bytes().to_vec();
        long.push(0);
        let tree = scan(&archive(&[
            member("./usr", b'5', b"", ""),
            member("././@LongLink", b'L', &long, ""),
            member("placeholder", b'0', b"x", ""),
        ]))
        .unwrap();
        assert_eq!(tree.entries().len(), 2);
        assert_eq!(tree.entries()[1].path, name.strip_prefix("./").unwrap());
        let pax = b"15 path=./file\n";
        assert_eq!(pax.len(), 15);
        let tree = scan(&archive(&[
            member("PaxHeader", b'x', pax, ""),
            member("placeholder", b'0', b"x", ""),
        ]))
        .unwrap();
        assert_eq!(tree.entries()[0].path, "file");
    }

    #[test]
    fn overridden_gnu_short_name_can_end_inside_utf8_and_posix_prefix_is_preserved() {
        let name = format!("./{}", "é".repeat(60));
        let mut long = name.as_bytes().to_vec();
        long.push(0);
        let mut short = header("placeholder", b'0', 0, "");
        short[..100].fill(b'n');
        short[99] = 0xc3; // incomplete UTF-8, superseded by the complete long name
        checksum(&mut short);
        let tree = scan(&archive(&[
            member("extension", b'L', &long, ""),
            short.to_vec(),
        ]))
        .unwrap();
        assert_eq!(tree.entries()[0].path, name.strip_prefix("./").unwrap());
        let mut posix = header("file", b'0', 0, "");
        posix[257..265].copy_from_slice(b"ustar\0\x30\x30");
        posix[345..348].copy_from_slice(b"dir");
        checksum(&mut posix);
        let tree = scan(&archive(&[member("dir", b'5', b"", ""), posix.to_vec()])).unwrap();
        assert_eq!(tree.entries()[1].path, "dir/file");
    }

    #[test]
    fn duplicate_escaping_paths_missing_parents_and_symlink_ancestors_fail() {
        for entries in [
            vec![
                member("./file", b'0', b"", ""),
                member("file", b'0', b"", ""),
            ],
            vec![member("../outside", b'0', b"", "")],
            vec![member("/outside", b'0', b"", "")],
            vec![member("dir/file", b'0', b"", "")],
            vec![
                member("dir", b'2', b"", "/tmp"),
                member("dir/file", b'0', b"", ""),
            ],
            vec![
                member("dir/file", b'0', b"", ""),
                member("dir", b'2', b"", "/tmp"),
            ],
            vec![member("link", b'2', b"", "../outside")],
            vec![member("hard", b'1', b"", "../outside")],
            vec![
                member("hard", b'1', b"", "later"),
                member("later", b'0', b"", ""),
            ],
        ] {
            assert!(scan(&archive(&entries)).is_err());
        }
    }

    #[test]
    fn malformed_extensions_checksums_truncation_padding_and_appended_payload_fail() {
        let good = archive(&[member("file", b'0', b"abc", "")]);
        let mut bad_checksum = good.clone();
        bad_checksum[0] ^= 1;
        let mut bad_padding = good.clone();
        bad_padding[BLOCK + 3] = 1;
        let mut partial_padding = good.clone();
        partial_padding.extend_from_slice(&[0; BLOCK - 1]);
        let mut appended = good.clone();
        appended.extend_from_slice(b"another archive");
        for bytes in [
            bad_checksum,
            bad_padding,
            partial_padding,
            appended,
            good[..BLOCK + 2].to_vec(),
            good[..good.len() - 1].to_vec(),
            archive(&[member("extension", b'L', b"orphan\0", "")]),
            archive(&[
                member("extension", b'L', b"unterminated", ""),
                member("file", b'0', b"", ""),
            ]),
            archive(&[
                member("extension", b'x', b"15 path=./file\n15 path=./file\n", ""),
                member("file", b'0', b"", ""),
            ]),
            archive(&[member("extension", b'x', b"999 path=file\n", "")]),
            archive(&[
                member("extension", b'x', b"12 uid=1000\n", ""),
                member("file", b'0', b"", ""),
            ]),
            archive(&[member("extension", b'g', b"", "")]),
        ] {
            assert!(scan(&bytes).is_err());
        }
    }

    #[test]
    fn unsupported_device_nodes_numeric_modes_and_file_bounds_fail_closed() {
        let mut device = header("dev/null", b'3', 0, "");
        device[329..337].copy_from_slice(b"0000001\0");
        device[337..345].copy_from_slice(b"0000003\0");
        checksum(&mut device);
        scan(&archive(&[member("dev", b'5', b"", ""), device.to_vec()])).unwrap();
        device[337..345].copy_from_slice(b"0000004\0");
        checksum(&mut device);
        assert!(scan(&archive(&[member("dev", b'5', b"", ""), device.to_vec()])).is_err());
        assert!(scan(&archive(&[header("file", b'0', MAX_FILE + 1, "").to_vec()])).is_err());
        assert!(scan(&archive(&[member("device", b'4', b"", "")])).is_err());
        let mut mode = header("file", b'0', 0, "");
        mode[100..108].copy_from_slice(b"0010000\0");
        checksum(&mut mode);
        assert!(scan(&archive(&[mode.to_vec()])).is_err());
    }

    #[test]
    fn fragmented_interrupted_and_invalid_reads_are_bounded_and_cancellable() {
        struct Fragments<'a> {
            bytes: &'a [u8],
            calls: usize,
            cancel: Option<&'a Cancellation>,
        }
        impl Read for Fragments<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if self.calls == 2
                    && let Some(cancel) = self.cancel
                {
                    cancel.cancel();
                }
                if self.calls == 3 {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let count = buffer.len().min(7);
                self.bytes.read(&mut buffer[..count])
            }
        }
        struct Invalid;
        impl Read for Invalid {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Ok(usize::MAX)
            }
        }
        let bytes = archive(&[member("file", b'0', b"contents", "")]);
        let cancel = Cancellation::default();
        let mut source = Fragments {
            bytes: &bytes,
            calls: 0,
            cancel: None,
        };
        assert_eq!(inspect(&mut source, &cancel).unwrap().file_bytes, 8);
        assert!(source.calls > 10);
        let mut source = Fragments {
            bytes: &bytes,
            calls: 0,
            cancel: Some(&cancel),
        };
        assert!(matches!(
            inspect(&mut source, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(source.calls, 2);
        assert!(matches!(
            inspect(Invalid, &Cancellation::default()),
            Err(Error::Length)
        ));
    }

    #[test]
    fn cancellation_stops_between_bounded_content_reads() {
        struct Cancelling<'a> {
            bytes: &'a [u8],
            cancel: &'a Cancellation,
            calls: usize,
        }
        impl Read for Cancelling<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(buffer.len() <= 8192);
                self.calls += 1;
                if self.calls == 2 {
                    self.cancel.cancel();
                }
                self.bytes.read(buffer)
            }
        }
        let bytes = archive(&[member("file", b'0', &vec![17; 20_000], "")]);
        let cancel = Cancellation::default();
        let source = Cancelling {
            bytes: &bytes,
            cancel: &cancel,
            calls: 0,
        };
        assert!(matches!(inspect(source, &cancel), Err(Error::Cancelled)));
    }
    fn compressed(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn gzip_checks_tar_trailer_and_rejects_concatenated_or_trailing_data() {
        let tar = archive(&[
            member("./", b'5', b"", ""),
            member("./file", b'0', b"data", ""),
        ]);
        let gzip = compressed(&tar);
        let inspection = inspect_gzip(gzip.as_slice(), &Cancellation::default()).unwrap();
        assert_eq!(
            inspection.semantic_sha256,
            scan(&tar).unwrap().semantic_sha256
        );
        assert_eq!(inspection.file_bytes, 4);
        for trailer_index in [gzip.len() - 8, gzip.len() - 4] {
            let mut changed = gzip.clone();
            changed[trailer_index] ^= 1;
            assert!(inspect_gzip(changed.as_slice(), &Cancellation::default()).is_err());
        }
        for length in [0, 9, gzip.len() - 1, gzip.len() - 8] {
            assert!(inspect_gzip(&gzip[..length], &Cancellation::default()).is_err());
        }
        for tail in [vec![0], vec![37], gzip.clone()] {
            let mut appended = gzip.clone();
            appended.extend_from_slice(&tail);
            assert!(inspect_gzip(appended.as_slice(), &Cancellation::default()).is_err());
        }
        let bad_tar = compressed(b"not a complete tar");
        assert!(inspect_gzip(bad_tar.as_slice(), &Cancellation::default()).is_err());
    }

    #[test]
    fn gzip_header_rejects_optional_allocations_and_unknown_flags_before_decode() {
        let gzip = compressed(&archive(&[member("./", b'5', b"", "")]));
        for flags in [1, 2, 4, 8, 16, 32, 64, 128, 255] {
            let mut changed = gzip.clone();
            changed[3] = flags;
            assert!(matches!(
                inspect_gzip(changed.as_slice(), &Cancellation::default()),
                Err(Error::RootfsArchive("unsupported gzip header"))
            ));
        }
        for index in 0..3 {
            let mut changed = gzip.clone();
            changed[index] ^= 1;
            assert!(inspect_gzip(changed.as_slice(), &Cancellation::default()).is_err());
        }
    }

    #[test]
    fn compressed_reader_bounds_counts_and_checks_cancellation_after_each_fragment() {
        struct Fragment<'a> {
            bytes: &'a [u8],
            cancel: &'a Cancellation,
            remaining: usize,
        }
        impl Read for Fragment<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    self.cancel.cancel();
                }
                self.remaining = self.remaining.saturating_sub(1);
                let count = buffer.len().min(self.bytes.len()).min(1);
                buffer[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes = &self.bytes[count..];
                Ok(count)
            }
        }
        let gzip = compressed(&archive(&[member("./", b'5', b"", "")]));
        for remaining in [0, 9, 10, 15, gzip.len() - 1] {
            let cancel = Cancellation::default();
            let reader = Fragment {
                bytes: &gzip,
                cancel: &cancel,
                remaining,
            };
            assert!(matches!(
                inspect_gzip(reader, &cancel),
                Err(Error::Cancelled)
            ));
        }
        let cancel = Cancellation::default();
        let reader = Fragment {
            bytes: &gzip,
            cancel: &cancel,
            remaining: usize::MAX,
        };
        assert!(inspect_gzip(reader, &cancel).is_ok());
        let mut source = Compressed {
            reader: &[1][..],
            bytes: crate::manifest::MAX_ASSET_BYTES,
            cancel: &cancel,
        };
        assert!(source.read(&mut [0; 1]).is_err());
    }
}
