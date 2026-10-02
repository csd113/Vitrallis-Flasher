//! Bounded delivery, inspection and Linux installation of private snapshots.
//! These operations grant no NAND authority.
use super::VerifiedAsset;
use crate::{Cancellation, Error};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};

/// Leaves room for JSON byte-array encoding within a 64 KiB recovery frame.
pub const TRANSFER_CHUNK_BYTES: usize = 8192;

impl VerifiedAsset {
    /// Install this rootfs beneath an owned, empty Linux root capability.
    ///
    /// Complete source inspection and metadata preflight precede creation. All
    /// directories are created privately before replay; final directory metadata
    /// is deferred until replay, gzip EOF and the retained asset recheck pass.
    /// Failure can leave partial installation and never retries or rolls back.
    /// Completion additionally requires contained installed-filesystem readback.
    /// This grants no NAND authority or physical UBI/volume authorization.
    /// # Errors
    /// Rejects source changes, unsupported metadata, non-root execution,
    /// cancellation, filesystem errors or incomplete replay.
    #[cfg(target_os = "linux")]
    pub fn install_rootfs(
        &mut self,
        root: crate::rootfs::contained::Root,
        cancel: &Cancellation,
    ) -> Result<crate::rootfs::Inspection, Error> {
        let expected = self.inspect_rootfs(cancel)?;
        let mut installer = crate::rootfs::contained::Installer::new(root, &expected, cancel)?;
        let result = crate::rootfs::replay_gzip(
            self.snapshot.as_file_mut(),
            &expected,
            &mut installer,
            cancel,
        )?;
        self.recheck(cancel)?;
        cancel.check()?;
        let root = installer.complete()?;
        root.verify_rootfs(&result, cancel)?;
        cancel.check()?;
        Ok(result)
    }

    /// Inspect the compressed rootfs from this retained private snapshot.
    ///
    /// The exact asset length/hash is rechecked before decoding and again before
    /// returning the semantic inventory. No cache path is reopened, filesystem
    /// content is extracted, or installation authorization is granted.
    /// # Errors
    /// Rejects other roles, snapshot changes, malformed gzip/tar and cancellation.
    pub fn inspect_rootfs(
        &mut self,
        cancel: &Cancellation,
    ) -> Result<crate::rootfs::Inspection, Error> {
        if self.role() != crate::manifest::Role::Rootfs {
            return Err(Error::Manifest(
                "rootfs inspection requires the rootfs role",
            ));
        }
        self.recheck(cancel)?;
        let inspection = crate::rootfs::inspect_gzip(self.snapshot.as_file_mut(), cancel)?;
        self.recheck(cancel)?;
        cancel.check()?;
        Ok(inspection)
    }

    /// Inspect and replay this rootfs into a bounded consumer.
    ///
    /// The complete archive is inspected before delivery. Replay validates each
    /// entry against that inventory, and the retained compressed snapshot is
    /// rechecked before and after replay. Consumers receive no NAND authority;
    /// a later error can follow partial delivery and never causes a retry.
    /// # Errors
    /// Rejects wrong roles, snapshot/archive changes, cancellation or sink failure.
    pub fn replay_rootfs(
        &mut self,
        sink: &mut impl crate::rootfs::Sink,
        cancel: &Cancellation,
    ) -> Result<crate::rootfs::Inspection, Error> {
        let expected = self.inspect_rootfs(cancel)?;
        expected.installation_preflight(cancel)?;
        let result =
            crate::rootfs::replay_gzip(self.snapshot.as_file_mut(), &expected, sink, cancel)?;
        self.recheck(cancel)?;
        cancel.check()?;
        Ok(result)
    }

    /// Revalidate before delivery, then deliver ordered bounded chunks from the
    /// same open snapshot. Offsets describe artifact bytes, never NAND addresses.
    /// A successful callback acknowledges delivery only; the caller must retain
    /// interruption state and separately verify installation. No retry/resume is
    /// performed. Completion progress follows final length/hash validation.
    /// # Errors
    /// Returns corruption, cancellation, delivery or I/O failure. Some chunks
    /// may already have been delivered when a later failure is detected.
    pub fn transfer(
        &mut self,
        cancel: &Cancellation,
        mut deliver: impl FnMut(u64, &[u8]) -> Result<(), Error>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), Error> {
        self.recheck(cancel)?;
        let size = self.size();
        let expected = self.spec.sha256();
        let source = self.snapshot.as_file_mut();
        source.rewind()?;
        let mut bytes = [0; TRANSFER_CHUNK_BYTES];
        let mut digest = Sha256::new();
        let mut offset = 0_u64;
        loop {
            cancel.check()?;
            let limit = usize::try_from((size - offset + 1).min(bytes.len() as u64))
                .map_err(|_| Error::Length)?;
            let count = source.read(&mut bytes[..limit])?;
            cancel.check()?;
            if count == 0 {
                break;
            }
            let end = offset.checked_add(count as u64).ok_or(Error::Length)?;
            if end > size {
                return Err(Error::Length);
            }
            digest.update(&bytes[..count]);
            deliver(offset, &bytes[..count])?;
            cancel.check()?;
            offset = end;
            // Even the last accepted chunk is not completion before EOF/hash.
            progress(offset.min(size.saturating_sub(1)), size);
        }
        if offset != size {
            return Err(Error::Length);
        }
        if format!("{:x}", digest.finalize()) != expected {
            return Err(Error::Hash);
        }
        cancel.check()?;
        progress(size, size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assets::Cache,
        manifest::{Manifest, Role},
        simulation,
    };
    use std::io::{SeekFrom, Write};

    fn snapshot(bytes: &[u8]) -> (VerifiedAsset, tempfile::TempDir) {
        let directory = crate::assets::temporary_directory().unwrap();
        let mut raw: serde_json::Value = serde_json::from_str(simulation::MANIFEST).unwrap();
        let rootfs = raw["assets"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|asset| asset["role"] == "rootfs")
            .unwrap();
        rootfs["size"] = (bytes.len() as u64).into();
        rootfs["sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
        let manifest = Manifest::read(serde_json::to_vec(&raw).unwrap().as_slice()).unwrap();
        let spec = manifest
            .assets()
            .iter()
            .find(|asset| asset.role() == Role::Rootfs)
            .unwrap();
        let asset = Cache::open(directory.path())
            .unwrap()
            .import(spec, bytes, &Cancellation::default(), |_, _| {})
            .unwrap();
        (asset, directory)
    }

    #[test]
    fn bounded_chunks_and_offsets_cover_snapshot_and_complete_after_final_check() {
        let bytes = vec![37; TRANSFER_CHUNK_BYTES * 3 + 11];
        let (mut asset, _directory) = snapshot(&bytes);
        let mut delivered = Vec::new();
        let mut events = Vec::new();
        asset
            .transfer(
                &Cancellation::default(),
                |offset, chunk| {
                    assert_eq!(offset, delivered.len() as u64);
                    assert!(!chunk.is_empty() && chunk.len() <= TRANSFER_CHUNK_BYTES);
                    delivered.extend_from_slice(chunk);
                    Ok(())
                },
                |done, total| events.push((done, total)),
            )
            .unwrap();
        assert_eq!(delivered, bytes);
        assert_eq!(events.len(), 5);
        assert!(events[..4].iter().all(|(done, total)| done < total));
        assert!(events.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(events[4], (bytes.len() as u64, bytes.len() as u64));
    }

    #[test]
    fn changed_snapshot_is_rejected_before_any_delivery() {
        let (mut asset, _directory) = snapshot(b"verified bytes");
        asset
            .snapshot
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .unwrap();
        asset.snapshot.as_file_mut().write_all(b"X").unwrap();
        let mut called = false;
        assert!(matches!(
            asset.transfer(
                &Cancellation::default(),
                |_, _| {
                    called = true;
                    Ok(())
                },
                |_, _| panic!("no progress on corrupt snapshot")
            ),
            Err(Error::Hash)
        ));
        assert!(!called);
    }

    #[test]
    fn delivery_failure_or_cancellation_never_completes_or_retries() {
        for cancel_delivery in [false, true] {
            let (mut asset, _directory) = snapshot(&vec![9; TRANSFER_CHUNK_BYTES * 3]);
            let cancel = Cancellation::default();
            let mut calls = 0;
            let mut complete = false;
            let result = asset.transfer(
                &cancel,
                |_, _| {
                    calls += 1;
                    if calls == 2 {
                        if cancel_delivery {
                            cancel.cancel();
                        } else {
                            return Err(Error::Timeout);
                        }
                    }
                    Ok(())
                },
                |done, total| complete |= done == total,
            );
            assert!(if cancel_delivery {
                matches!(result, Err(Error::Cancelled))
            } else {
                matches!(result, Err(Error::Timeout))
            });
            assert_eq!(calls, 2);
            assert!(!complete);
        }
    }

    #[test]
    fn corruption_or_append_during_delivery_fails_final_validation() {
        for append in [false, true] {
            let (mut asset, _directory) = snapshot(&vec![9; TRANSFER_CHUNK_BYTES * 2]);
            let mut other = asset.snapshot.as_file().try_clone().unwrap();
            let mut calls = 0;
            let mut complete = false;
            let result = asset.transfer(
                &Cancellation::default(),
                |_, _| {
                    calls += 1;
                    if calls == 1 {
                        // Clone shares the open-file position; restore it after mutation.
                        let position = other.stream_position()?;
                        other.seek(if append {
                            SeekFrom::End(0)
                        } else {
                            SeekFrom::Start(TRANSFER_CHUNK_BYTES as u64)
                        })?;
                        other.write_all(b"X")?;
                        other.seek(SeekFrom::Start(position))?;
                    }
                    Ok(())
                },
                |done, total| complete |= done == total,
            );
            assert!(if append {
                matches!(result, Err(Error::Length))
            } else {
                matches!(result, Err(Error::Hash))
            });
            assert!(!complete);
        }
    }
    fn rootfs_gzip() -> Vec<u8> {
        let mut header = [0_u8; 512];
        header[..2].copy_from_slice(b"./");
        header[100..108].copy_from_slice(b"0000755\0");
        header[156] = b'5';
        header[257..265].copy_from_slice(b"ustar  \0");
        header[148..156].fill(b' ');
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&header).unwrap();
        encoder.write_all(&[0; 1024]).unwrap();
        encoder.finish().unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rootfs_installation_finishes_only_a_complete_verified_snapshot() {
        use crate::rootfs::contained::Root;
        use std::fs::File;
        use std::os::unix::fs::PermissionsExt;
        for malformed in [false, true] {
            let mut bytes = rootfs_gzip();
            if malformed {
                let crc = bytes.len() - 8;
                bytes[crc] ^= 1;
            }
            let (mut asset, _cache) = snapshot(&bytes);
            let directory = tempfile::tempdir().unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let cancel = Cancellation::default();
            let root = Root::new(File::open(directory.path()).unwrap(), &cancel).unwrap();
            let result = asset.install_rootfs(root, &cancel);
            if !malformed && rustix::process::geteuid().as_raw() == 0 {
                assert_eq!(result.unwrap().entries().len(), 1);
                assert_eq!(
                    std::fs::metadata(directory.path())
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o7777,
                    0o755
                );
            } else {
                assert!(result.is_err());
                assert_eq!(
                    std::fs::metadata(directory.path())
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o7777,
                    0o700
                );
            }
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires root and the exact locked 516 MB stock archive; isolated filesystem workload"]
    fn rootfs_installation_complete_stock_archive_with_semantic_readback() {
        use crate::rootfs::contained::Root;
        use std::fmt::Write as _;
        use std::{fs::File, os::unix::fs::PermissionsExt, time::Instant};
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let started = Instant::now();
        let path = std::env::var_os("VITRALLIS_STOCK_ROOTFS_ARCHIVE").unwrap();
        let source = File::open(path).unwrap();
        assert!(source.metadata().unwrap().is_file());
        let mut raw: serde_json::Value = serde_json::from_str(simulation::MANIFEST).unwrap();
        let asset = raw["assets"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|asset| asset["role"] == "rootfs")
            .unwrap();
        asset["size"] = 516_563_033_u64.into();
        asset["sha256"] = "1e516cade3085633f61697d69a5d95cb84a501d8b606247987db5837a53e19ef".into();
        let manifest = Manifest::read(serde_json::to_vec(&raw).unwrap().as_slice()).unwrap();
        let spec = manifest
            .assets()
            .iter()
            .find(|asset| asset.role() == Role::Rootfs)
            .unwrap();
        // Only this role is acquired; the other simulation roles grant no trust.
        let private = tempfile::tempdir().unwrap();
        let cache_path = private.path().join("cache");
        std::fs::create_dir(&cache_path).unwrap();
        std::fs::set_permissions(&cache_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = Cancellation::default();
        let mut asset = Cache::open(&cache_path)
            .unwrap()
            .import(spec, source, &cancel, |_, _| {})
            .unwrap();
        let root_path = private.path().join("root");
        std::fs::create_dir(&root_path).unwrap();
        std::fs::set_permissions(&root_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = Root::new(File::open(&root_path).unwrap(), &cancel).unwrap();
        let result = asset.install_rootfs(root, &cancel).unwrap();
        assert_eq!(result.entries().len(), 50_950);
        assert_eq!(result.file_bytes, 1_264_012_666);
        let mut semantic = String::with_capacity(64);
        for byte in result.semantic_sha256 {
            write!(semantic, "{byte:02x}").unwrap();
        }
        assert_eq!(
            semantic,
            "50deb4906c1cfc5bf30b766cdb0e711713b8564022b65a686878ef6bab500c35"
        );
        println!(
            "{}",
            serde_json::json!({"stock_archive_installed":true,"installed_filesystem_semantic_readback":true,"device_accessed":false,"nand_mutated":false,"entries":result.entries().len(),"file_bytes":result.file_bytes,"semantic_sha256":semantic,"elapsed_seconds":started.elapsed().as_secs_f64()})
        );
    }

    #[test]
    fn rootfs_inspection_uses_the_retained_snapshot_after_cache_path_replacement() {
        let bytes = rootfs_gzip();
        let (mut asset, directory) = snapshot(&bytes);
        std::fs::write(
            directory.path().join(asset.sha256()),
            b"replaced cache entry",
        )
        .unwrap();
        let inventory = asset.inspect_rootfs(&Cancellation::default()).unwrap();
        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].path, ".");
        assert_eq!(inventory.entries()[0].kind, crate::rootfs::Kind::Directory);
        assert_eq!(inventory.file_bytes, 0);
        assert_eq!(asset.snapshot.as_file_mut().stream_position().unwrap(), 0);
        asset.recheck(&Cancellation::default()).unwrap();
    }

    #[test]
    fn rootfs_inspection_rejects_changed_snapshot_even_when_gzip_remains_valid() {
        let bytes = rootfs_gzip();
        let (mut asset, _directory) = snapshot(&bytes);
        // Gzip mtime is outside the content CRC; decoding alone would accept it.
        asset
            .snapshot
            .as_file_mut()
            .seek(SeekFrom::Start(4))
            .unwrap();
        asset.snapshot.as_file_mut().write_all(&[1]).unwrap();
        let mut changed = bytes;
        changed[4] = 1;
        assert!(crate::rootfs::inspect_gzip(changed.as_slice(), &Cancellation::default()).is_ok());
        assert!(matches!(
            asset.inspect_rootfs(&Cancellation::default()),
            Err(Error::Hash)
        ));
    }

    #[test]
    fn rootfs_inspection_rejects_wrong_role_invalid_archive_and_cancellation() {
        let (mut asset, _directory) = snapshot(&rootfs_gzip());
        asset.spec = Manifest::read(simulation::MANIFEST.as_bytes())
            .unwrap()
            .assets()
            .iter()
            .find(|spec| spec.role() == Role::UbootNand)
            .unwrap()
            .clone();
        assert!(matches!(
            asset.inspect_rootfs(&Cancellation::default()),
            Err(Error::Manifest(_))
        ));
        let (mut invalid, _invalid_directory) = snapshot(b"hash-verified but not an archive");
        assert!(invalid.inspect_rootfs(&Cancellation::default()).is_err());
        let cancel = Cancellation::default();
        cancel.cancel();
        let (mut valid, _valid_directory) = snapshot(&rootfs_gzip());
        assert!(matches!(
            valid.inspect_rootfs(&cancel),
            Err(Error::Cancelled)
        ));
    }
    #[test]
    fn rootfs_replay_rechecks_snapshot_after_delivery_without_retry() {
        struct Consumer {
            file: std::fs::File,
            mutate: bool,
            begun: usize,
            finished: usize,
        }
        impl crate::rootfs::Sink for Consumer {
            fn begin(&mut self, _entry: &crate::rootfs::Entry) -> Result<(), Error> {
                self.begun += 1;
                if self.mutate {
                    let position = self.file.stream_position()?;
                    self.file.seek(SeekFrom::Start(4))?;
                    self.file.write_all(&[1])?;
                    self.file.seek(SeekFrom::Start(position))?;
                }
                Ok(())
            }
            fn data(&mut self, _offset: u64, _bytes: &[u8]) -> Result<(), Error> {
                Err(Error::State) // The fixture contains only the root directory.
            }
            fn finish(&mut self, _entry: &crate::rootfs::Entry) -> Result<(), Error> {
                self.finished += 1;
                Ok(())
            }
        }
        for mutate in [false, true] {
            let (mut asset, _directory) = snapshot(&rootfs_gzip());
            let mut consumer = Consumer {
                file: asset.snapshot.as_file().try_clone().unwrap(),
                mutate,
                begun: 0,
                finished: 0,
            };
            let result = asset.replay_rootfs(&mut consumer, &Cancellation::default());
            assert!(if mutate {
                matches!(result, Err(Error::Hash))
            } else {
                result.is_ok()
            });
            assert_eq!(consumer.begun, 1);
            assert_eq!(consumer.finished, 1);
        }
    }
}
