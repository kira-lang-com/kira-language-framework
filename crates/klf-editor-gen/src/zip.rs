//! A zip archive with stored entries, enough for an OPC package such as a
//! `.vsix`.

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn u16_le(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&u16::try_from(value).unwrap_or(u16::MAX).to_le_bytes());
}

fn u32_le(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes());
}

/// An archive of `entries`, in order, with fixed timestamps so the same
/// input gives the same bytes.
pub(crate) fn archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    const DOS_DATE: usize = (2020 - 1980) << 9 | 1 << 5 | 1;
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (name, contents) in entries {
        let offset = out.len();
        let crc = crc32(contents) as usize;
        let header = |out: &mut Vec<u8>, central: bool| {
            if central {
                u32_le(out, 0x0201_4b50);
                u16_le(out, 20);
            } else {
                u32_le(out, 0x0403_4b50);
            }
            u16_le(out, 20);
            u16_le(out, 1 << 11);
            u16_le(out, 0);
            u16_le(out, 0);
            u16_le(out, DOS_DATE);
            u32_le(out, crc);
            u32_le(out, contents.len());
            u32_le(out, contents.len());
            u16_le(out, name.len());
            u16_le(out, 0);
            if central {
                u16_le(out, 0);
                u16_le(out, 0);
                u16_le(out, 0);
                u32_le(out, 0);
                u32_le(out, offset);
            }
            out.extend_from_slice(name.as_bytes());
        };
        header(&mut out, false);
        out.extend_from_slice(contents);
        header(&mut directory, true);
    }
    let start = out.len();
    out.extend_from_slice(&directory);
    u32_le(&mut out, 0x0605_4b50);
    u16_le(&mut out, 0);
    u16_le(&mut out, 0);
    u16_le(&mut out, entries.len());
    u16_le(&mut out, entries.len());
    u32_le(&mut out, directory.len());
    u32_le(&mut out, start);
    u16_le(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}
