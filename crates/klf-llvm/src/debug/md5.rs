//! MD5 of a virtual file's text, for the DWARF 5 line table's checksum
//! column. DWARF 5 defines it as MD5, so a debugger can tell embedded text
//! from the file on disk.

// The round constants and shift amounts of the published algorithm.
#![allow(clippy::unreadable_literal)]

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// The MD5 of `text` as the 32 lowercase hex digits DWARF writes.
#[must_use]
pub(crate) fn hex(text: &str) -> String {
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let mut message = text.as_bytes().to_vec();
    let length = u64::try_from(message.len()).expect("a file under 2^61 bytes");
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&(length * 8).to_le_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut words = [0_u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let at = index * 4;
            *word = u32::from_le_bytes([chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]]);
        }
        let [mut first, mut second, mut third, mut fourth] = state;
        for round in 0..64 {
            let (mix, word) = match round / 16 {
                0 => ((second & third) | (!second & fourth), round),
                1 => ((fourth & second) | (!fourth & third), (5 * round + 1) % 16),
                2 => (second ^ third ^ fourth, (3 * round + 5) % 16),
                _ => (third ^ (second | !fourth), (7 * round) % 16),
            };
            let sum = first
                .wrapping_add(mix)
                .wrapping_add(K[round])
                .wrapping_add(words[word])
                .rotate_left(S[round]);
            first = fourth;
            fourth = third;
            third = second;
            second = second.wrapping_add(sum);
        }
        for (slot, value) in state.iter_mut().zip([first, second, third, fourth]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = String::with_capacity(32);
    for byte in state.iter().flat_map(|word| word.to_le_bytes()) {
        digest.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble"));
        digest.push(char::from_digit(u32::from(byte & 0xf), 16).expect("a nibble"));
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn matches_the_published_digests() {
        assert_eq!(hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex("The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }
}
