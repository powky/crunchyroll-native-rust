//! Samples as the decoder takes them, since the pipeline gets no codec setup besides the codec's name: H.264 in
//! Annex B with the parameter sets before each keyframe, AAC behind ADTS headers. What's added or resized is clear
//! data, so it only grows the clear part of the subsample it lands in and the encrypted runs keep their keystream.

use anyhow::{Result, bail};

use crate::Subsample;
use crate::mp4::AudioConfig;

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const ADTS_HEADER: usize = 7;

/// SPS and PPS with start codes, to go before keyframes.
pub fn parameter_sets(sps: &[Vec<u8>], pps: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for set in sps.iter().chain(pps) {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(set);
    }
    out
}

/// A length-prefixed (avcC) H.264 sample in Annex B, after `prefix`. `subsamples` (empty for clear samples) are
/// adjusted to match.
pub fn annex_b(sample: &[u8], nal_length: u8, prefix: &[u8], subsamples: &mut [Subsample]) -> Result<Vec<u8>> {
    let length = nal_length as usize;
    if !(1..=4).contains(&length) {
        bail!("bad NAL length size {length}");
    }
    let mut out = Vec::with_capacity(prefix.len() + sample.len() + 64);
    out.extend_from_slice(prefix);
    let mut position = 0;
    while position < sample.len() {
        let Some(header) = sample.get(position..position + length) else {
            bail!("truncated NAL length");
        };
        let size = header.iter().fold(0usize, |size, &byte| size << 8 | byte as usize);
        let Some(nal) = sample.get(position + length..position + length + size) else {
            bail!("NAL length past the sample's end (is the whole sample encrypted?)");
        };
        if length != 4 && !subsamples.is_empty() {
            grow_clear(subsamples, position, 4 - length)?;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
        position += length + size;
    }
    if let Some(first) = subsamples.first_mut() {
        first.clear += prefix.len() as u32;
    }
    Ok(out)
}

/// An AAC frame behind an ADTS header. An encrypted frame without subsamples (whole-sample encryption) gets one, so
/// the header stays clear.
pub fn adts(frame: &[u8], config: AudioConfig, encrypted: bool, subsamples: &mut Vec<Subsample>) -> Vec<u8> {
    let length = frame.len() + ADTS_HEADER;
    // ADTS profiles stop at LTP; HE-AAC is signalled as its LC core
    let profile = if (1..=4).contains(&config.object_type) { config.object_type - 1 } else { 1 };
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&[
        0xff,
        0xf1,
        profile << 6 | (config.frequency_index & 0xf) << 2 | (config.channels >> 2 & 1),
        (config.channels & 3) << 6 | (length >> 11 & 3) as u8,
        (length >> 3) as u8,
        ((length & 7) << 5) as u8 | 0x1f,
        0xfc,
    ]);
    out.extend_from_slice(frame);
    if let Some(first) = subsamples.first_mut() {
        first.clear += ADTS_HEADER as u32;
    } else if encrypted {
        subsamples.push(Subsample { clear: ADTS_HEADER as u32, cipher: frame.len() as u32 });
    }
    out
}

/// Adds `extra` clear bytes to the subsample whose clear part holds offset `at` of the original sample.
fn grow_clear(subsamples: &mut [Subsample], at: usize, extra: usize) -> Result<()> {
    let mut start = 0;
    for subsample in subsamples.iter_mut() {
        if at < start + subsample.clear as usize {
            subsample.clear += extra as u32;
            return Ok(());
        }
        start += (subsample.clear + subsample.cipher) as usize;
    }
    bail!("a NAL length is inside encrypted data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_keeps_encrypted_runs() {
        // two NALs with 2-byte lengths: [00 03 aa bb cc] [00 02 dd ee]
        let sample = [0, 3, 0xaa, 0xbb, 0xcc, 0, 2, 0xdd, 0xee];
        let mut subsamples = vec![Subsample { clear: 3, cipher: 2 }, Subsample { clear: 3, cipher: 1 }];
        let out = annex_b(&sample, 2, &[9, 9], &mut subsamples).unwrap();
        assert_eq!(out, [9, 9, 0, 0, 0, 1, 0xaa, 0xbb, 0xcc, 0, 0, 0, 1, 0xdd, 0xee]);
        assert_eq!(subsamples, [Subsample { clear: 7, cipher: 2 }, Subsample { clear: 5, cipher: 1 }]);
        let total: u32 = subsamples.iter().map(|s| s.clear + s.cipher).sum();
        assert_eq!(total as usize, out.len());
    }

    #[test]
    fn adts_header() {
        let config = AudioConfig { object_type: 2, frequency_index: 3, channels: 2 };
        let mut subsamples = Vec::new();
        let out = adts(&[1, 2, 3], config, true, &mut subsamples);
        assert_eq!(&out[..7], &[0xff, 0xf1, 0x4c, 0x80, 0x01, 0x5f, 0xfc]);
        assert_eq!(subsamples, [Subsample { clear: 7, cipher: 3 }]);
    }
}
