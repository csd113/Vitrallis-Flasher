//! Bounded BCH-64 decoding for the reviewed R8 boot0 codeword format.
//!
//! The pinned image builder uses GF(2^14), polynomial 0x5803, 1024 data
//! bytes, four FF metadata bytes and 112 parity bytes. Its bit reversal and
//! boot0 LFSR are accounted for before syndrome/locator/root calculations.
//! The decoder performs no I/O. The saved-region entry point reads only a bounded,
//! regular diagnostic capture. No method writes NAND or grants execution approval.
use crate::{Cancellation, Error};

pub const CODEWORD_BYTES: usize = 1140;
const DATA_BYTES: usize = 1024;
const ORDER: usize = 16_383;
const STRENGTH: usize = 64;
const BITS: usize = CODEWORD_BYTES * 8;

/// Corrected data and the number of flipped bits in data, metadata or parity.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedPage {
    pub data: [u8; DATA_BYTES],
    pub corrected_bits: usize,
}

/// One decoded 16 KiB SPL copy. Its digest must still match the trusted artifact.
pub struct DecodedSpl {
    pub data: [u8; 16_384],
    pub corrected_bits: [usize; 16],
}

/// Public evidence from a saved raw region; no decoded executable bytes are serialized.
#[derive(Debug, serde::Serialize)]
pub struct SplCopyReport {
    pub copy: u8,
    pub data_sha256: String,
    pub corrected_bits: [usize; 16],
    pub header_bytes: Vec<u8>,
    pub checksum: u32,
}

/// Decodes all four SPL copies from an original separated-data capture.
/// # Errors
/// Rejects unsafe paths, wrong size, malformed SPL, ECC failure or cancellation.
pub fn read_saved_region(
    path: &std::path::Path,
    cancel: &Cancellation,
) -> Result<Vec<SplCopyReport>, Error> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    cancel.check()?;
    crate::assets::regular_components(path)?;
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != 4_194_304 {
        return Err(Error::Length);
    }
    let mut raw = Vec::new();
    file.take(4_194_305).read_to_end(&mut raw)?;
    let decoder = Boot0Decoder::new()?;
    (0..4)
        .map(|copy| {
            let result = decoder.decode_spl(&raw, copy, cancel)?;
            Ok(SplCopyReport {
                copy,
                data_sha256: format!("{:x}", Sha256::digest(result.data)),
                corrected_bits: result.corrected_bits,
                header_bytes: result.data[..32].to_vec(),
                checksum: u32::from_le_bytes(
                    result.data[12..16].try_into().map_err(|_| Error::Length)?,
                ),
            })
        })
        .collect()
}

/// Reusable bounded field tables; one decoder can process all boot0 pages.
pub struct Boot0Decoder {
    powers: Vec<u16>,
    logs: Vec<u16>,
}
impl Boot0Decoder {
    /// Builds the reviewed primitive-field tables.
    /// # Errors
    /// Rejects an internal field construction error.
    pub fn new() -> Result<Self, Error> {
        let mut powers = Vec::with_capacity(ORDER * 2);
        let mut logs = vec![0; ORDER + 1];
        let mut value = 1_u16;
        for exponent in 0..ORDER {
            if exponent != 0 && value == 1 {
                return Err(Error::State);
            }
            powers.push(value);
            logs[usize::from(value)] = u16::try_from(exponent).map_err(|_| Error::State)?;
            value <<= 1;
            if value & 0x4000 != 0 {
                value ^= 0x5803;
            }
        }
        if value != 1 {
            return Err(Error::State);
        }
        powers.extend_from_within(..ORDER);
        Ok(Self { powers, logs })
    }

    fn multiply(&self, a: u16, b: u16) -> u16 {
        if a == 0 || b == 0 {
            0
        } else {
            self.powers
                [usize::from(self.logs[usize::from(a)]) + usize::from(self.logs[usize::from(b)])]
        }
    }

    fn divide(&self, a: u16, b: u16) -> Result<u16, Error> {
        if b == 0 {
            return Err(Error::State);
        }
        if a == 0 {
            return Ok(0);
        }
        let exponent =
            usize::from(self.logs[usize::from(a)]) + ORDER - usize::from(self.logs[usize::from(b)]);
        Ok(self.powers[exponent])
    }

    fn syndromes(
        &self,
        bytes: &[u8; CODEWORD_BYTES],
        cancel: &Cancellation,
    ) -> Result<[u16; STRENGTH * 2], Error> {
        let mut syndromes = [0; STRENGTH * 2];
        for (index, byte) in bytes.iter().enumerate() {
            if index % 32 == 0 {
                cancel.check()?;
            }
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    // The builder reverses every byte: its first wire LSB is
                    // the highest polynomial coefficient of this shortened code.
                    let position = BITS - 1 - index * 8 - bit;
                    for (i, syndrome) in syndromes.iter_mut().enumerate().step_by(2) {
                        *syndrome ^= self.powers[((i + 1) * position) % ORDER];
                    }
                }
            }
        }
        for i in (1..STRENGTH * 2).step_by(2) {
            let half = syndromes[i / 2];
            syndromes[i] = self.multiply(half, half);
        }
        Ok(syndromes)
    }

    fn locator(&self, syndromes: &[u16; STRENGTH * 2]) -> Result<(usize, [u16; 129]), Error> {
        // Berlekamp-Massey over the 128 syndromes, with fixed-size polynomials.
        let mut current = [0; 129];
        let mut previous = [0; 129];
        current[0] = 1;
        previous[0] = 1;
        let mut degree = 0;
        let mut shift = 1;
        let mut last_discrepancy = 1;
        for (index, syndrome) in syndromes.iter().enumerate() {
            let mut discrepancy = *syndrome;
            for i in 1..=degree {
                discrepancy ^= self.multiply(current[i], syndromes[index - i]);
            }
            if discrepancy == 0 {
                shift += 1;
                continue;
            }
            let saved = current;
            let scale = self.divide(discrepancy, last_discrepancy)?;
            for i in 0..129 - shift {
                current[i + shift] ^= self.multiply(scale, previous[i]);
            }
            if 2 * degree <= index {
                degree = index + 1 - degree;
                previous = saved;
                last_discrepancy = discrepancy;
                shift = 1;
            } else {
                shift += 1;
            }
        }
        if degree == 0 || degree > STRENGTH {
            return Err(Error::Boot0Ecc);
        }
        Ok((degree, current))
    }

    fn correct(
        &self,
        bytes: &mut [u8; CODEWORD_BYTES],
        degree: usize,
        locator: &[u16; 129],
        cancel: &Cancellation,
    ) -> Result<usize, Error> {
        // Chien search is restricted to this shortened 9120-bit codeword.
        let mut terms = *locator;
        let mut corrected = 0;
        for position in 0..BITS {
            if position % 256 == 0 {
                cancel.check()?;
            }
            if terms[..=degree].iter().fold(0, |a, b| a ^ b) == 0 {
                let bit = BITS - 1 - position;
                bytes[bit / 8] ^= 1 << (bit % 8);
                corrected += 1;
            }
            for (i, term) in terms.iter_mut().enumerate().take(degree + 1).skip(1) {
                *term = self.multiply(*term, self.powers[ORDER - i]);
            }
        }
        if corrected != degree {
            return Err(Error::Boot0Ecc);
        }
        Ok(corrected)
    }

    /// Decodes one of the four 64-page-spaced SPL copies in a 4 MiB raw-data block.
    /// # Errors
    /// Rejects wrong region length, copy index, ECC or header/checksum errors.
    pub fn decode_spl(
        &self,
        raw: &[u8],
        copy: u8,
        cancel: &Cancellation,
    ) -> Result<DecodedSpl, Error> {
        if raw.len() != 4_194_304 || copy > 3 {
            return Err(Error::Length);
        }
        let mut data = [0; 16_384];
        let mut corrected_bits = [0; 16];
        for (page, count) in corrected_bits.iter_mut().enumerate() {
            let offset = (usize::from(copy) * 64 + page) * 16_384;
            let result = self.decode(&raw[offset..offset + CODEWORD_BYTES], cancel)?;
            data[page * DATA_BYTES..(page + 1) * DATA_BYTES].copy_from_slice(&result.data);
            *count = result.corrected_bits;
        }
        if &data[4..12] != b"eGON.BT0" || &data[20..24] != b"SPL\x02" {
            return Err(Error::Boot0Ecc);
        }
        let length = u32::from_le_bytes(data[16..20].try_into().map_err(|_| Error::Length)?);
        let checksum = u32::from_le_bytes(data[12..16].try_into().map_err(|_| Error::Length)?);
        let computed = data
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .fold(0_u32, |sum, (i, word)| {
                sum.wrapping_add(if i == 3 {
                    0x5f0a_6c39
                } else {
                    u32::from_le_bytes(*word)
                })
            });
        if length != 16_384 || computed != checksum {
            return Err(Error::Boot0Ecc);
        }
        Ok(DecodedSpl {
            data,
            corrected_bits,
        })
    }

    /// Decodes one complete scrambled boot0 codeword, including its parity.
    /// # Errors
    /// Rejects wrong lengths, cancellation, uncorrectable syndromes or metadata.
    /// A caller must still compare the decoded artifact digest: bounded-distance
    /// decoding alone cannot identify the intended codeword beyond 64 errors.
    pub fn decode(&self, raw: &[u8], cancel: &Cancellation) -> Result<DecodedPage, Error> {
        cancel.check()?;
        let mut bytes: [u8; CODEWORD_BYTES] = raw.try_into().map_err(|_| Error::Length)?;
        descramble(&mut bytes);
        let syndromes = self.syndromes(&bytes, cancel)?;
        let corrected_bits = if syndromes.iter().all(|value| *value == 0) {
            0
        } else {
            let (degree, locator) = self.locator(&syndromes)?;
            let corrected = self.correct(&mut bytes, degree, &locator, cancel)?;
            if self
                .syndromes(&bytes, cancel)?
                .iter()
                .any(|value| *value != 0)
            {
                return Err(Error::Boot0Ecc);
            }
            corrected
        };
        if bytes[DATA_BYTES..DATA_BYTES + 4] != [0xff; 4] {
            return Err(Error::Boot0Ecc);
        }
        Ok(DecodedPage {
            data: bytes[..DATA_BYTES].try_into().map_err(|_| Error::Length)?,
            corrected_bits,
        })
    }
}

fn descramble(bytes: &mut [u8; CODEWORD_BYTES]) {
    let mut state = 0x4a80_u16;
    for _ in 0..15 {
        state = (state >> 1) | (((state ^ (state >> 1)) & 1) << 14);
    }
    for byte in bytes {
        *byte ^= state.to_le_bytes()[0];
        for _ in 0..8 {
            state = (state >> 1) | (((state ^ (state >> 1)) & 1) << 14);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const GOLDEN: &[u8] = include_bytes!("../../../fixtures/nand/boot0-bch64-codeword.bin");

    #[test]
    fn pinned_encoder_known_answer_and_full_correction_radius() {
        let decoder = Boot0Decoder::new().unwrap();
        let pristine = decoder.decode(GOLDEN, &Cancellation::default()).unwrap();
        assert_eq!(pristine.corrected_bits, 0);
        assert_eq!(
            format!("{:x}", Sha256::digest(pristine.data)),
            "4608af7a94879fbad2f208ee76912b309111e9d0c928e8ae78f736fc130feffb"
        );
        for count in [1, 8, 32, 64] {
            let mut raw = GOLDEN.to_vec();
            for i in 0..count {
                let position = (i * 137 + 11) % BITS;
                raw[position / 8] ^= 1 << (position % 8);
            }
            let result = decoder.decode(&raw, &Cancellation::default()).unwrap();
            assert_eq!(result.data, pristine.data);
            assert_eq!(result.corrected_bits, count);
        }
    }

    #[test]
    fn lengths_erased_data_excess_errors_and_cancellation_fail_closed() {
        let decoder = Boot0Decoder::new().unwrap();
        for raw in [
            &GOLDEN[..GOLDEN.len() - 1],
            &[0xff; CODEWORD_BYTES],
            &[0; CODEWORD_BYTES],
        ] {
            assert!(decoder.decode(raw, &Cancellation::default()).is_err());
        }
        let mut raw = GOLDEN.to_vec();
        for i in 0..65 {
            let position = (i * 137 + 11) % BITS;
            raw[position / 8] ^= 1 << (position % 8);
        }
        assert!(decoder.decode(&raw, &Cancellation::default()).is_err());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            decoder.decode(GOLDEN, &cancel),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn metadata_errors_are_corrected_and_copy_bounds_are_rejected() {
        let decoder = Boot0Decoder::new().unwrap();
        let pristine = decoder.decode(GOLDEN, &Cancellation::default()).unwrap();
        let mut raw = GOLDEN.to_vec();
        for byte in &mut raw[DATA_BYTES..DATA_BYTES + 4] {
            *byte ^= 0xff;
        }
        let corrected = decoder.decode(&raw, &Cancellation::default()).unwrap();
        assert_eq!(corrected.data, pristine.data);
        assert_eq!(corrected.corrected_bits, 32);
        assert!(matches!(
            decoder.decode_spl(&vec![0xff; 4_194_304], 4, &Cancellation::default()),
            Err(Error::Length)
        ));
        assert!(matches!(
            decoder.decode_spl(&[], 0, &Cancellation::default()),
            Err(Error::Length)
        ));
    }
}
