//! Bounded delivery of private verified snapshots. This grants no NAND authority.
use super::VerifiedAsset;
use crate::{Cancellation, Error};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};

/// Leaves room for JSON byte-array encoding within a 64 KiB recovery frame.
pub const TRANSFER_CHUNK_BYTES: usize = 8192;

impl VerifiedAsset {
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
}
