//! Two-pass integrity validation without a retained compressed archive.
//! Contracts and inventories are data; they grant no device/NAND authority.
use super::{Inspection, Sink};
use crate::{Cancellation, Error, manifest::MAX_ASSET_BYTES};
use sha2::{Digest, Sha256};
use std::io::{self, Read};

/// Bounded compressed rootfs length/hash contract, not publisher or plan trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    size: u64,
    sha256: [u8; 32],
}
impl SourceSpec {
    /// Validate an integrity contract. Callers must separately bind it to their
    /// verified assets, approved plan and authenticated device/session.
    /// # Errors
    /// Rejects empty/oversized sources or a zero digest.
    pub fn new(size: u64, sha256: [u8; 32]) -> Result<Self, Error> {
        if size == 0 || size > MAX_ASSET_BYTES {
            return Err(Error::Length);
        }
        if sha256 == [0; 32] {
            return Err(Error::Hash);
        }
        Ok(Self { size, sha256 })
    }
    #[must_use]
    pub const fn size(self) -> u64 {
        self.size
    }
    #[must_use]
    pub const fn sha256(self) -> [u8; 32] {
        self.sha256
    }

    /// Inspect and hash a complete first pass without filesystem mutation.
    /// The reader must supply explicit pass EOF and separately bounded blocking
    /// reads, as an authenticated framed transport would. Only bounded content
    /// buffers and the already-bounded metadata inventory are retained.
    /// # Errors
    /// Rejects malformed gzip/tar, length/hash changes, unsupported installation
    /// metadata, reader failure or cancellation.
    pub fn inspect(
        self,
        reader: impl Read,
        cancel: &Cancellation,
    ) -> Result<InspectedStream, Error> {
        let mut source = CheckedSource::new(reader, self, cancel);
        let expected = super::inspect_gzip(&mut source, cancel);
        cancel.check()?;
        let expected = expected?;
        source.finish()?;
        expected.installation_preflight(cancel)?;
        Ok(InspectedStream {
            spec: self,
            expected,
        })
    }
}

/// Complete first-pass source integrity and immutable metadata inventory.
/// This is not a `VerifiedAssets` token or physical installation authorization.
#[derive(Debug)]
pub struct InspectedStream {
    spec: SourceSpec,
    expected: Inspection,
}
impl InspectedStream {
    #[must_use]
    pub const fn spec(&self) -> SourceSpec {
        self.spec
    }
    #[must_use]
    pub const fn inspection(&self) -> &Inspection {
        &self.expected
    }

    /// Replay a separately supplied second pass against the first inventory.
    /// Each pass must match the exact compressed length/hash, including gzip
    /// header bytes outside the content CRC. Sink callbacks may precede a late
    /// source error; no retry/resume or completion is inferred from callbacks.
    /// # Errors
    /// Rejects archive/source changes, consumer failure or cancellation.
    pub fn replay(
        &self,
        reader: impl Read,
        sink: &mut impl Sink,
        cancel: &Cancellation,
    ) -> Result<Inspection, Error> {
        let mut source = CheckedSource::new(reader, self.spec, cancel);
        let result = super::replay_gzip(&mut source, &self.expected, sink, cancel);
        cancel.check()?;
        let result = result?;
        source.finish()?;
        Ok(result)
    }

    /// Install a second pass beneath a retained empty Linux filesystem root.
    /// Source hash/EOF must pass before directory finalization, and contained
    /// semantic readback must pass before success. Partial output is preserved
    /// on failure. No NAND/UBI authorization or automatic retry is provided.
    /// # Errors
    /// Rejects source/metadata/content changes, cancellation or filesystem errors.
    #[cfg(target_os = "linux")]
    pub fn install(
        &self,
        reader: impl Read,
        root: super::contained::Root,
        cancel: &Cancellation,
    ) -> Result<Inspection, Error> {
        cancel.check()?;
        let mut installer = super::contained::Installer::new(root, &self.expected, cancel)?;
        let result = self.replay(reader, &mut installer, cancel)?;
        let root = installer.complete()?;
        root.verify_rootfs(&result, cancel)?;
        cancel.check()?;
        Ok(result)
    }
}

struct CheckedSource<'a, R> {
    reader: R,
    spec: SourceSpec,
    cancel: &'a Cancellation,
    hash: Sha256,
    bytes: u64,
    eof: bool,
}
impl<'a, R: Read> CheckedSource<'a, R> {
    fn new(reader: R, spec: SourceSpec, cancel: &'a Cancellation) -> Self {
        Self {
            reader,
            spec,
            cancel,
            hash: Sha256::new(),
            bytes: 0,
            eof: false,
        }
    }
    fn finish(self) -> Result<(), Error> {
        self.cancel.check()?;
        if !self.eof || self.bytes != self.spec.size {
            return Err(Error::Length);
        }
        if <[u8; 32]>::from(self.hash.finalize()) != self.spec.sha256 {
            return Err(Error::Hash);
        }
        self.cancel.check()
    }
}
impl<R: Read> Read for CheckedSource<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        if buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self
            .spec
            .size
            .checked_sub(self.bytes)
            .ok_or_else(|| io::Error::other(Error::Length))?;
        let limit = usize::try_from((remaining + 1).min(8192).min(buffer.len() as u64))
            .map_err(|_| io::Error::other(Error::Length))?;
        let count = self.reader.read(&mut buffer[..limit]);
        self.cancel.check().map_err(io::Error::other)?;
        let count = count?;
        if count > limit {
            return Err(io::Error::other(Error::Length));
        }
        if count == 0 {
            self.eof = true;
            return Ok(0);
        }
        self.bytes = self
            .bytes
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other(Error::Length))?;
        if self.bytes > self.spec.size || self.eof {
            return Err(io::Error::other(Error::Length));
        }
        self.hash.update(&buffer[..count]);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rootfs::{
        Entry,
        tests::{archive, compressed, member},
    };

    fn fixture() -> (SourceSpec, Vec<u8>) {
        let data: Vec<_> = (0_u32..20_000).map(|n| n.to_le_bytes()[0]).collect();
        let bytes = compressed(&archive(&[
            member("./", b'5', b"", ""),
            member("./file", b'0', &data, ""),
        ]));
        (
            SourceSpec::new(bytes.len() as u64, Sha256::digest(&bytes).into()).unwrap(),
            bytes,
        )
    }
    #[derive(Default)]
    struct Consumer {
        begun: usize,
        finished: usize,
        bytes: u64,
    }
    impl Sink for Consumer {
        fn begin(&mut self, _: &Entry) -> Result<(), Error> {
            self.begun += 1;
            Ok(())
        }
        fn data(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
            assert_eq!(offset, self.bytes);
            assert!(!bytes.is_empty() && bytes.len() <= 8192);
            self.bytes += bytes.len() as u64;
            Ok(())
        }
        fn finish(&mut self, _: &Entry) -> Result<(), Error> {
            self.finished += 1;
            Ok(())
        }
    }
    #[test]
    fn complete_two_pass_delivery_preserves_inventory_with_bounded_chunks() {
        let (spec, bytes) = fixture();
        let cancel = Cancellation::default();
        let inspected = spec.inspect(bytes.as_slice(), &cancel).unwrap();
        assert_eq!(inspected.spec(), spec);
        assert_eq!(inspected.inspection().entries().len(), 2);
        let mut consumer = Consumer::default();
        let result = inspected
            .replay(bytes.as_slice(), &mut consumer, &cancel)
            .unwrap();
        assert_eq!(
            result.semantic_sha256,
            inspected.inspection().semantic_sha256
        );
        assert_eq!(
            (consumer.begun, consumer.finished, consumer.bytes),
            (2, 2, 20_000)
        );
    }
    #[test]
    fn changed_gzip_header_rejected_in_each_pass_even_with_identical_decoded_contents() {
        let (spec, bytes) = fixture();
        let cancel = Cancellation::default();
        let mut changed = bytes.clone();
        changed[4] ^= 1;
        assert!(super::super::inspect_gzip(changed.as_slice(), &cancel).is_ok());
        assert!(matches!(
            spec.inspect(changed.as_slice(), &cancel),
            Err(Error::Hash)
        ));
        let inspected = spec.inspect(bytes.as_slice(), &cancel).unwrap();
        let mut consumer = Consumer::default();
        assert!(matches!(
            inspected.replay(changed.as_slice(), &mut consumer, &cancel),
            Err(Error::Hash)
        ));
        // Entry completion is delivery acknowledgement, never global completion.
        assert_eq!(
            (consumer.begun, consumer.finished, consumer.bytes),
            (2, 2, 20_000)
        );
    }
    #[test]
    fn source_contract_bounds_lengths_truncation_and_extra_bytes_fail_closed() {
        let (spec, bytes) = fixture();
        assert!(matches!(
            SourceSpec::new(0, spec.sha256()),
            Err(Error::Length)
        ));
        assert!(matches!(
            SourceSpec::new(MAX_ASSET_BYTES + 1, spec.sha256()),
            Err(Error::Length)
        ));
        assert!(matches!(
            SourceSpec::new(spec.size(), [0; 32]),
            Err(Error::Hash)
        ));
        for size in [spec.size() - 1, spec.size() + 1] {
            assert!(
                SourceSpec::new(size, spec.sha256())
                    .unwrap()
                    .inspect(bytes.as_slice(), &Cancellation::default())
                    .is_err()
            );
        }
        assert!(
            spec.inspect(&bytes[..bytes.len() - 1], &Cancellation::default())
                .is_err()
        );
        let mut extra = bytes;
        extra.push(0);
        assert!(
            spec.inspect(extra.as_slice(), &Cancellation::default())
                .is_err()
        );
    }
    #[test]
    fn fragmented_interrupted_and_cancelled_sources_remain_bounded() {
        struct Fragmented<'a> {
            bytes: &'a [u8],
            cancel: &'a Cancellation,
            calls: usize,
            cancel_at: usize,
        }
        impl Read for Fragmented<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                assert!(buffer.len() <= 8192);
                self.calls += 1;
                if self.calls == self.cancel_at {
                    self.cancel.cancel();
                    return Err(io::ErrorKind::Interrupted.into());
                }
                if self.calls == 1 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let count = buffer.len().min(17);
                self.bytes.read(&mut buffer[..count])
            }
        }
        let (spec, bytes) = fixture();
        for cancel_at in [0, 3] {
            let cancel = Cancellation::default();
            let source = Fragmented {
                bytes: &bytes,
                cancel: &cancel,
                calls: 0,
                cancel_at,
            };
            let result = spec.inspect(source, &cancel);
            if cancel_at == 0 {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(Error::Cancelled)));
            }
        }
    }
    #[test]
    fn invalid_reader_counts_and_replay_consumer_failure_never_complete() {
        struct Invalid;
        impl Read for Invalid {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                Ok(buffer.len() + 1)
            }
        }
        struct Failing {
            calls: usize,
        }
        impl Sink for Failing {
            fn begin(&mut self, _: &Entry) -> Result<(), Error> {
                Ok(())
            }
            fn data(&mut self, _: u64, _: &[u8]) -> Result<(), Error> {
                self.calls += 1;
                Err(Error::Timeout)
            }
            fn finish(&mut self, entry: &Entry) -> Result<(), Error> {
                assert_eq!(entry.path, ".");
                Ok(())
            }
        }
        let (spec, bytes) = fixture();
        assert!(spec.inspect(Invalid, &Cancellation::default()).is_err());
        let cancel = Cancellation::default();
        let inspected = spec.inspect(bytes.as_slice(), &cancel).unwrap();
        let mut consumer = Failing { calls: 0 };
        assert!(matches!(
            inspected.replay(bytes.as_slice(), &mut consumer, &cancel),
            Err(Error::Timeout)
        ));
        assert_eq!(consumer.calls, 1);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn streamed_installation_checks_each_pass_and_defers_directory_completion() {
        use super::super::contained::Root;
        use std::{fs::File, os::unix::fs::PermissionsExt};
        let (spec, bytes) = fixture();
        let cancel = Cancellation::default();
        let inspected = spec.inspect(bytes.as_slice(), &cancel).unwrap();
        for changed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let root = Root::new(File::open(directory.path()).unwrap(), &cancel).unwrap();
            let mut source = bytes.clone();
            if changed {
                source[4] ^= 1;
            }
            let result = inspected.install(source.as_slice(), root, &cancel);
            if rustix::process::geteuid().as_raw() != 0 {
                assert!(result.is_err());
                continue;
            }
            if changed {
                assert!(matches!(result, Err(Error::Hash)));
            } else {
                assert_eq!(result.unwrap().entries().len(), 2);
            }
            assert_eq!(
                std::fs::metadata(directory.path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                if changed { 0o700 } else { 0o755 }
            );
            assert_eq!(
                std::fs::metadata(directory.path().join("file"))
                    .unwrap()
                    .len(),
                20_000
            );
        }
    }
}
