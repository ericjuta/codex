use std::hash::Hasher;

pub(super) fn hash_hex(input: &str, width: usize) -> String {
    let mut hasher = Fnv1a64::default();
    hasher.write(input.as_bytes());
    let mask = if width >= 16 {
        u64::MAX
    } else {
        (1_u64 << (width * 4)) - 1
    };
    format!("{:0width$x}", hasher.finish() & mask)
}

pub(super) fn line_hash(input: &str) -> String {
    hash_hex(input, 2)
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}
