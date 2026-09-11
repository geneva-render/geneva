//! H.264 bitstream helpers for stitching copied packets and freshly
//! encoded ones into one stream: the parameter sets in and out of the
//! `avcC` record, their ids, and the NAL units of length-prefixed packets.

/// The parameter sets of an `avcC` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterSets {
    /// `AVCProfileIndication`, `profile_compatibility`, `AVCLevelIndication`.
    pub profile: [u8; 3],
    /// Bytes of each NAL length prefix in the packets (1, 2 or 4).
    pub length_size: usize,
    /// Sequence parameter sets, NAL header included.
    pub sps: Vec<Vec<u8>>,
    /// Picture parameter sets, NAL header included.
    pub pps: Vec<Vec<u8>>,
    /// Whatever follows the PPS list (the High profile extension), kept
    /// as it was.
    pub tail: Vec<u8>,
}

/// Reads an `avcC` record.
pub fn parse_avcc(data: &[u8]) -> Option<ParameterSets> {
    if data.len() < 7 || data[0] != 1 {
        return None;
    }
    let profile = [data[1], data[2], data[3]];
    let length_size = usize::from(data[4] & 3) + 1;
    let mut pos = 5;
    let list = |count: usize, pos: &mut usize| -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*data.get(*pos)?, *data.get(*pos + 1)?]));
            *pos += 2;
            out.push(data.get(*pos..*pos + len)?.to_vec());
            *pos += len;
        }
        Some(out)
    };
    let sps_count = usize::from(data[pos] & 0x1f);
    pos += 1;
    let sps = list(sps_count, &mut pos)?;
    let pps_count = usize::from(*data.get(pos)?);
    pos += 1;
    let pps = list(pps_count, &mut pos)?;
    Some(ParameterSets {
        profile,
        length_size,
        sps,
        pps,
        tail: data[pos..].to_vec(),
    })
}

/// Writes an `avcC` record.
pub fn build_avcc(sets: &ParameterSets) -> Vec<u8> {
    let mut out = vec![
        1,
        sets.profile[0],
        sets.profile[1],
        sets.profile[2],
        0xfc | ((sets.length_size as u8).saturating_sub(1) & 3),
        0xe0 | (sets.sps.len() as u8 & 0x1f),
    ];
    for sps in &sets.sps {
        out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        out.extend_from_slice(sps);
    }
    out.push(sets.pps.len() as u8);
    for pps in &sets.pps {
        out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        out.extend_from_slice(pps);
    }
    out.extend_from_slice(&sets.tail);
    out
}

/// The NAL units of a packet whose units carry `length_size`-byte
/// big-endian length prefixes (the form MP4 and Matroska store).
pub fn nal_units(packet: &[u8], length_size: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + length_size <= packet.len() {
        let mut len = 0usize;
        for b in &packet[pos..pos + length_size] {
            len = (len << 8) | usize::from(*b);
        }
        pos += length_size;
        let Some(nal) = packet.get(pos..pos + len) else {
            break;
        };
        if !nal.is_empty() {
            out.push(nal);
        }
        pos += len;
    }
    out
}

/// NAL unit type of a unit (its first byte's low five bits).
pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1f)
}

/// Whether a packet holds an IDR picture.
pub fn is_idr(packet: &[u8], length_size: usize) -> bool {
    nal_units(packet, length_size)
        .iter()
        .any(|nal| nal_type(nal) == 5)
}

/// The payload of a NAL unit without its emulation prevention bytes.
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0usize;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    /// An unsigned Exp-Golomb code.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1 << zeros) - 1 + self.bits(zeros)?)
    }
}

/// The id of a sequence parameter set.
pub fn sps_id(sps: &[u8]) -> Option<u8> {
    let body = rbsp(sps.get(1..)?);
    let mut bits = Bits {
        data: &body,
        pos: 0,
    };
    bits.bits(24)?; // profile_idc, constraint flags, level_idc
    u8::try_from(bits.ue()?).ok()
}

/// The id of a picture parameter set and the id of the SPS it refers to.
pub fn pps_ids(pps: &[u8]) -> Option<(u8, u8)> {
    let body = rbsp(pps.get(1..)?);
    let mut bits = Bits {
        data: &body,
        pos: 0,
    };
    let pps = u8::try_from(bits.ue()?).ok()?;
    let sps = u8::try_from(bits.ue()?).ok()?;
    Some((pps, sps))
}

/// The smallest id in 0..32 that none of the parameter sets use.
pub fn free_id(sets: &ParameterSets) -> Option<u8> {
    let mut used = [false; 32];
    for sps in &sets.sps {
        if let Some(id) = sps_id(sps) {
            used[usize::from(id) & 31] = true;
        }
    }
    for pps in &sets.pps {
        if let Some((p, s)) = pps_ids(pps) {
            used[usize::from(p) & 31] = true;
            used[usize::from(s) & 31] = true;
        }
    }
    (0..32u8).find(|id| !used[usize::from(*id)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_round_trips_and_reads_ids() {
        // An SPS (id 0, High 4.0) and a PPS (id 0 → sps 0) from x264.
        let sps = vec![
            0x67, 0x64, 0x00, 0x28, 0xac, 0xd9, 0x40, 0x78, 0x02, 0x27, 0xe5, 0x84,
        ];
        let pps = vec![0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];
        let sets = ParameterSets {
            profile: [0x64, 0, 0x28],
            length_size: 4,
            sps: vec![sps.clone()],
            pps: vec![pps.clone()],
            tail: vec![0xfd, 0xf8, 0xf8, 0x00],
        };
        let bytes = build_avcc(&sets);
        assert_eq!(parse_avcc(&bytes).unwrap(), sets);
        assert_eq!(sps_id(&sps), Some(0));
        assert_eq!(pps_ids(&pps), Some((0, 0)));
        assert_eq!(free_id(&sets), Some(1));
        let packet = [0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 1, 0x06];
        assert!(is_idr(&packet, 4));
        assert_eq!(nal_units(&packet, 4).len(), 2);
    }

    #[test]
    fn emulation_prevention_bytes_are_removed() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
    }
}
