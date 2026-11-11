//! Just enough ISO BMFF for fragmented-MP4 DASH: the init segment's track (codec setup and CENC defaults) and each
//! fragment's samples with their encryption data (`senc`, or `saiz`/`saio` aux info).

use anyhow::{Context, Result, bail};

use crate::Subsample;

/// Microsoft PIFF's `senc`, as a `uuid` box.
const PIFF_SENC: [u8; 16] = [0xa2, 0x39, 0x4f, 0x52, 0x5a, 0x9b, 0x4f, 0x14, 0xa2, 0x44, 0x6c, 0x42, 0x7c, 0x64, 0x8d, 0xf4];

/// Widevine's DRM system id in `pssh` boxes.
pub const WIDEVINE: [u8; 16] = [0xed, 0xef, 0x8b, 0xa9, 0x79, 0xd6, 0x4a, 0xce, 0xa3, 0xc8, 0x27, 0xdc, 0xd5, 0x1d, 0x21, 0xed];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Other,
}

#[derive(Clone, Debug)]
pub enum Codec {
    /// H.264: NAL length size and the parameter sets from `avcC`
    Avc { nal_length: u8, sps: Vec<Vec<u8>>, pps: Vec<Vec<u8>> },
    Aac(AudioConfig),
    Unknown([u8; 4]),
}

/// The parts of an AAC AudioSpecificConfig an ADTS header repeats.
#[derive(Clone, Copy, Debug)]
pub struct AudioConfig {
    pub object_type: u8,
    pub frequency_index: u8,
    pub channels: u8,
}

/// The track's CENC defaults (`tenc`).
#[derive(Clone, Debug)]
pub struct Protection {
    /// `cenc` or `cbcs`
    pub scheme: [u8; 4],
    pub protected: bool,
    pub iv_size: u8,
    pub key_id: [u8; 16],
    /// when samples carry no IV of their own
    pub constant_iv: Vec<u8>,
    pub crypt_blocks: u8,
    pub skip_blocks: u8,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub kind: Kind,
    pub timescale: u32,
    pub width: u16,
    pub height: u16,
    pub sample_rate: u32,
    pub codec: Codec,
    pub protection: Option<Protection>,
    defaults: Defaults,
}

#[derive(Clone, Copy, Debug, Default)]
struct Defaults {
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Clone, Debug)]
pub struct Init {
    pub tracks: Vec<Track>,
    /// the `pssh` boxes, whole
    pub pssh: Vec<Vec<u8>>,
}

impl Init {
    pub fn parse(data: &[u8]) -> Result<Init> {
        let moov = atoms(data).find(|atom| &atom.kind == b"moov").context("no moov")?;
        let mut tracks = Vec::new();
        let mut pssh = Vec::new();
        let mut defaults = Vec::new();
        for atom in atoms(moov.body) {
            match &atom.kind {
                b"trak" => tracks.push(parse_track(atom.body)?),
                b"pssh" => pssh.push(atom.whole.to_vec()),
                b"mvex" => {
                    for trex in atoms(atom.body).filter(|atom| &atom.kind == b"trex") {
                        let mut bytes = Bytes::new(trex.body);
                        bytes.skip(4)?;
                        let id = bytes.u32()?;
                        bytes.skip(4)?;
                        defaults.push((id, Defaults { duration: bytes.u32()?, size: bytes.u32()?, flags: bytes.u32()? }));
                    }
                }
                _ => {}
            }
        }
        for track in &mut tracks {
            if let Some((_, found)) = defaults.iter().find(|(id, _)| *id == track.id) {
                track.defaults = *found;
            }
        }
        Ok(Init { tracks, pssh })
    }

    /// The first video or audio track.
    pub fn track(&self, kind: Kind) -> Option<&Track> {
        self.tracks.iter().find(|track| track.kind == kind)
    }

    /// The `pssh` boxes of one DRM system.
    pub fn pssh_for(&self, system: &[u8; 16]) -> Vec<u8> {
        self.pssh.iter().filter(|pssh| pssh.len() >= 28 && pssh[12..28] == system[..]).flatten().copied().collect()
    }
}

fn parse_track(trak: &[u8]) -> Result<Track> {
    let mut track = Track {
        id: 0,
        kind: Kind::Other,
        timescale: 1,
        width: 0,
        height: 0,
        sample_rate: 0,
        codec: Codec::Unknown(*b"none"),
        protection: None,
        defaults: Defaults::default(),
    };
    for atom in atoms(trak) {
        match &atom.kind {
            b"tkhd" => {
                let mut bytes = Bytes::new(atom.body);
                let version = bytes.u8()?;
                bytes.skip(3 + if version == 1 { 16 } else { 8 })?;
                track.id = bytes.u32()?;
            }
            b"mdia" => {
                for atom in atoms(atom.body) {
                    match &atom.kind {
                        b"mdhd" => {
                            let mut bytes = Bytes::new(atom.body);
                            let version = bytes.u8()?;
                            bytes.skip(3 + if version == 1 { 16 } else { 8 })?;
                            track.timescale = bytes.u32()?.max(1);
                        }
                        b"hdlr" => {
                            let mut bytes = Bytes::new(atom.body);
                            bytes.skip(8)?;
                            track.kind = match bytes.take(4)? {
                                b"vide" => Kind::Video,
                                b"soun" => Kind::Audio,
                                _ => Kind::Other,
                            };
                        }
                        b"minf" => {
                            let stbl = atoms(atom.body).find(|atom| &atom.kind == b"stbl").context("no stbl")?;
                            let stsd = atoms(stbl.body).find(|atom| &atom.kind == b"stsd").context("no stsd")?;
                            parse_sample_entry(stsd.body.get(8..).context("short stsd")?, &mut track)?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(track)
}

fn parse_sample_entry(entries: &[u8], track: &mut Track) -> Result<()> {
    let entry = atoms(entries).next().context("no sample entry")?;
    let mut format = entry.kind;
    let children = match &entry.kind {
        b"avc1" | b"avc3" | b"encv" | b"hvc1" | b"hev1" => {
            let mut bytes = Bytes::new(entry.body);
            bytes.skip(24)?;
            track.width = bytes.u16()?;
            track.height = bytes.u16()?;
            entry.body.get(78..).context("short visual sample entry")?
        }
        b"mp4a" | b"enca" | b"ec-3" | b"ac-3" => {
            let mut bytes = Bytes::new(entry.body);
            bytes.skip(8)?;
            let version = bytes.u16()?;
            bytes.skip(14)?;
            track.sample_rate = bytes.u32()? >> 16;
            let skip = match version {
                1 => 44,
                2 => 64,
                _ => 28,
            };
            entry.body.get(skip..).context("short audio sample entry")?
        }
        _ => {
            track.codec = Codec::Unknown(entry.kind);
            return Ok(());
        }
    };
    for atom in atoms(children) {
        match &atom.kind {
            b"sinf" => {
                let mut protection = Protection {
                    scheme: *b"cenc",
                    protected: true,
                    iv_size: 8,
                    key_id: [0; 16],
                    constant_iv: Vec::new(),
                    crypt_blocks: 0,
                    skip_blocks: 0,
                };
                for atom in atoms(atom.body) {
                    match &atom.kind {
                        b"frma" => format = atom.body.get(..4).context("short frma")?.try_into()?,
                        b"schm" => protection.scheme = atom.body.get(4..8).context("short schm")?.try_into()?,
                        b"schi" => {
                            if let Some(tenc) = atoms(atom.body).find(|atom| &atom.kind == b"tenc") {
                                parse_tenc(tenc.body, &mut protection)?;
                            }
                        }
                        _ => {}
                    }
                }
                track.protection = Some(protection);
            }
            b"avcC" => track.codec = parse_avcc(atom.body)?,
            b"esds" => track.codec = Codec::Aac(parse_esds(atom.body)?),
            _ => {}
        }
    }
    if let Codec::Unknown(_) = track.codec {
        track.codec = Codec::Unknown(format);
    }
    Ok(())
}

fn parse_tenc(body: &[u8], protection: &mut Protection) -> Result<()> {
    let mut bytes = Bytes::new(body);
    let version = bytes.u8()?;
    // flags, a reserved byte, and one that holds the pattern from version 1 on
    bytes.skip(5)?;
    if version > 0 {
        protection.crypt_blocks = body[5] >> 4;
        protection.skip_blocks = body[5] & 0x0f;
    }
    protection.protected = bytes.u8()? != 0;
    protection.iv_size = bytes.u8()?;
    protection.key_id = bytes.take(16)?.try_into()?;
    if protection.protected && protection.iv_size == 0 {
        let size = bytes.u8()? as usize;
        protection.constant_iv = bytes.take(size)?.to_vec();
    }
    Ok(())
}

fn parse_avcc(body: &[u8]) -> Result<Codec> {
    let mut bytes = Bytes::new(body);
    bytes.skip(4)?;
    let nal_length = (bytes.u8()? & 3) + 1;
    let mut sps = Vec::new();
    for _ in 0..(bytes.u8()? & 0x1f) {
        let size = bytes.u16()? as usize;
        sps.push(bytes.take(size)?.to_vec());
    }
    let mut pps = Vec::new();
    for _ in 0..bytes.u8()? {
        let size = bytes.u16()? as usize;
        pps.push(bytes.take(size)?.to_vec());
    }
    Ok(Codec::Avc { nal_length, sps, pps })
}

/// The AudioSpecificConfig inside `esds`' descriptors (ES → DecoderConfig → DecoderSpecificInfo).
fn parse_esds(body: &[u8]) -> Result<AudioConfig> {
    let mut bytes = Bytes::new(body);
    bytes.skip(4)?;
    while bytes.remaining() > 0 {
        let tag = bytes.u8()?;
        let mut size = 0usize;
        for _ in 0..4 {
            let byte = bytes.u8()?;
            size = size << 7 | (byte & 0x7f) as usize;
            if byte & 0x80 == 0 {
                break;
            }
        }
        match tag {
            3 => {
                bytes.skip(2)?;
                let flags = bytes.u8()?;
                if flags & 0x80 != 0 {
                    bytes.skip(2)?;
                }
                if flags & 0x40 != 0 {
                    let length = bytes.u8()? as usize;
                    bytes.skip(length)?;
                }
                if flags & 0x20 != 0 {
                    bytes.skip(2)?;
                }
            }
            4 => bytes.skip(13)?,
            5 => {
                let config = bytes.take(size)?;
                let bits = u32::from_be_bytes([
                    *config.first().unwrap_or(&0),
                    *config.get(1).unwrap_or(&0),
                    *config.get(2).unwrap_or(&0),
                    *config.get(3).unwrap_or(&0),
                ]);
                let mut object_type = (bits >> 27) as u8;
                let mut rest = bits << 5;
                if object_type == 31 {
                    object_type = 32 + (rest >> 26) as u8;
                    rest <<= 6;
                }
                let frequency_index = (rest >> 28) as u8;
                // an explicit 24-bit frequency (index 15) never comes with AAC on Crunchyroll: assume stereo
                let channels = if frequency_index == 15 { 2 } else { (rest << 4 >> 28) as u8 };
                return Ok(AudioConfig { object_type, frequency_index, channels });
            }
            _ => bytes.skip(size)?,
        }
    }
    bail!("no AudioSpecificConfig in esds")
}

/// One access unit of a fragment, located in the segment it was parsed from.
#[derive(Clone, Debug)]
pub struct Sample {
    pub track: u32,
    /// in the track's timescale
    pub decode_time: u64,
    pub composition_offset: i64,
    pub duration: u32,
    pub keyframe: bool,
    pub offset: usize,
    pub size: usize,
    /// empty when the sample is in the clear
    pub iv: Vec<u8>,
    pub subsamples: Vec<Subsample>,
}

impl Sample {
    /// Presentation time in nanoseconds.
    pub fn pts_ns(&self, timescale: u32) -> u64 {
        let ticks = (self.decode_time as i64 + self.composition_offset).max(0) as u128;
        (ticks * 1_000_000_000 / timescale as u128) as u64
    }

    pub fn data<'a>(&self, segment: &'a [u8]) -> &'a [u8] {
        &segment[self.offset..self.offset + self.size]
    }
}

/// The samples of every `moof`/`mdat` pair in a media segment, for `track`.
pub fn parse_fragments(segment: &[u8], track: &Track) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    for moof in atoms(segment).filter(|atom| &atom.kind == b"moof") {
        for traf in atoms(moof.body).filter(|atom| &atom.kind == b"traf") {
            parse_traf(segment, moof.start, traf.body, track, &mut samples)?;
        }
    }
    Ok(samples)
}

fn parse_traf(segment: &[u8], moof_start: usize, traf: &[u8], track: &Track, samples: &mut Vec<Sample>) -> Result<()> {
    let tfhd = atoms(traf).find(|atom| &atom.kind == b"tfhd").context("no tfhd")?;
    let mut bytes = Bytes::new(tfhd.body);
    let flags = bytes.u32()? & 0xff_ffff;
    if bytes.u32()? != track.id {
        return Ok(());
    }
    if flags & 0x01 != 0 {
        bail!("absolute base data offsets aren't supported");
    }
    if flags & 0x02 != 0 {
        bytes.skip(4)?;
    }
    let mut defaults = track.defaults;
    if flags & 0x08 != 0 {
        defaults.duration = bytes.u32()?;
    }
    if flags & 0x10 != 0 {
        defaults.size = bytes.u32()?;
    }
    if flags & 0x20 != 0 {
        defaults.flags = bytes.u32()?;
    }

    let mut decode_time = 0;
    if let Some(tfdt) = atoms(traf).find(|atom| &atom.kind == b"tfdt") {
        let mut bytes = Bytes::new(tfdt.body);
        decode_time = if bytes.u8()? == 1 {
            bytes.skip(3)?;
            bytes.u64()?
        } else {
            bytes.skip(3)?;
            bytes.u32()? as u64
        };
    }

    let first = samples.len();
    let mut data_offset = moof_start;
    for trun in atoms(traf).filter(|atom| &atom.kind == b"trun") {
        let mut bytes = Bytes::new(trun.body);
        let version = bytes.u8()?;
        let flags = bytes.u24()?;
        let count = bytes.u32()?;
        if flags & 0x01 != 0 {
            data_offset = (moof_start as i64 + bytes.u32()? as i32 as i64) as usize;
        }
        let first_flags = if flags & 0x04 != 0 { Some(bytes.u32()?) } else { None };
        for index in 0..count {
            let duration = if flags & 0x100 != 0 { bytes.u32()? } else { defaults.duration };
            let size = if flags & 0x200 != 0 { bytes.u32()? } else { defaults.size } as usize;
            let sample_flags = if flags & 0x400 != 0 {
                bytes.u32()?
            } else if index == 0 && let Some(first_flags) = first_flags {
                first_flags
            } else {
                defaults.flags
            };
            let composition_offset = if flags & 0x800 != 0 {
                let raw = bytes.u32()?;
                if version == 0 { raw as i64 } else { raw as i32 as i64 }
            } else {
                0
            };
            if data_offset + size > segment.len() {
                bail!("sample beyond the segment's end");
            }
            samples.push(Sample {
                track: track.id,
                decode_time,
                composition_offset,
                duration,
                keyframe: track.kind != Kind::Video || sample_flags & 0x1_0000 == 0,
                offset: data_offset,
                size,
                iv: Vec::new(),
                subsamples: Vec::new(),
            });
            decode_time += duration as u64;
            data_offset += size;
        }
    }

    let Some(protection) = track.protection.as_ref().filter(|protection| protection.protected) else {
        return Ok(());
    };
    let samples = &mut samples[first..];
    let senc = atoms(traf).find_map(|atom| match &atom.kind {
        b"senc" => Some((atom.body, false)),
        b"uuid" if atom.body.get(..16) == Some(&PIFF_SENC[..]) => Some((&atom.body[16..], true)),
        _ => None,
    });
    if let Some((body, piff)) = senc {
        let mut bytes = Bytes::new(body);
        let flags = bytes.u32()? & 0xff_ffff;
        let mut iv_size = protection.iv_size as usize;
        if piff && flags & 0x01 != 0 {
            bytes.skip(3)?;
            iv_size = bytes.u8()? as usize;
            bytes.skip(16)?;
        }
        let count = bytes.u32()? as usize;
        for sample in samples.iter_mut().take(count) {
            read_aux_info(&mut bytes, iv_size, flags & 0x02 != 0, protection, sample)?;
        }
        return Ok(());
    }

    // the same records, located by saiz (sizes) and saio (offset from the moof)
    let saiz = atoms(traf).find(|atom| &atom.kind == b"saiz");
    let saio = atoms(traf).find(|atom| &atom.kind == b"saio");
    let (Some(saiz), Some(saio)) = (saiz, saio) else {
        bail!("encrypted samples without senc or saiz/saio");
    };
    let mut bytes = Bytes::new(saiz.body);
    let flags = bytes.u32()? & 0xff_ffff;
    if flags & 1 != 0 {
        bytes.skip(8)?;
    }
    let default_size = bytes.u8()? as usize;
    let count = bytes.u32()? as usize;
    let sizes: Vec<usize> = if default_size == 0 {
        bytes.take(count)?.iter().map(|&size| size as usize).collect()
    } else {
        vec![default_size; count]
    };
    let mut bytes = Bytes::new(saio.body);
    let version = bytes.u8()?;
    let flags = bytes.u24()?;
    if flags & 1 != 0 {
        bytes.skip(8)?;
    }
    bytes.skip(4)?;
    let offset = if version == 1 { bytes.u64()? as usize } else { bytes.u32()? as usize };
    let mut position = moof_start + offset;
    for (sample, size) in samples.iter_mut().zip(sizes) {
        let record = segment.get(position..position + size).context("aux info beyond the segment's end")?;
        let mut bytes = Bytes::new(record);
        read_aux_info(&mut bytes, protection.iv_size as usize, size > protection.iv_size as usize, protection, sample)?;
        position += size;
    }
    Ok(())
}

fn read_aux_info(
    bytes: &mut Bytes,
    iv_size: usize,
    has_subsamples: bool,
    protection: &Protection,
    sample: &mut Sample,
) -> Result<()> {
    sample.iv = if iv_size == 0 { protection.constant_iv.clone() } else { bytes.take(iv_size)?.to_vec() };
    if has_subsamples {
        for _ in 0..bytes.u16()? {
            sample.subsamples.push(Subsample { clear: bytes.u16()? as u32, cipher: bytes.u32()? });
        }
    }
    Ok(())
}

struct Atom<'a> {
    kind: [u8; 4],
    body: &'a [u8],
    /// header included
    whole: &'a [u8],
    /// where the box starts in the slice it was read from
    start: usize,
}

/// The boxes laid one after another in `data` (stopping at the first malformed one).
fn atoms(data: &[u8]) -> impl Iterator<Item = Atom<'_>> {
    let mut position = 0;
    std::iter::from_fn(move || {
        let header = data.get(position..position + 8)?;
        let mut size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let mut header_size = 8;
        if size == 1 {
            size = u64::from_be_bytes(data.get(position + 8..position + 16)?.try_into().unwrap()) as usize;
            header_size = 16;
        } else if size == 0 {
            size = data.len() - position;
        }
        if size < header_size || position + size > data.len() {
            return None;
        }
        let atom = Atom {
            kind,
            body: &data[position + header_size..position + size],
            whole: &data[position..position + size],
            start: position,
        };
        position += size;
        Some(atom)
    })
}

struct Bytes<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Bytes<'a> {
        Bytes { data, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let bytes = self.data.get(self.position..self.position + count).context("box too short")?;
        self.position += count;
        Ok(bytes)
    }

    fn skip(&mut self, count: usize) -> Result<()> {
        self.take(count).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }

    fn u24(&mut self) -> Result<u32> {
        let bytes = self.take(3)?;
        Ok(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }
}
