use std::io;

use tiny_keccak::{Hasher, Keccak};

/// Streams Borsh output straight into a keccak hasher.
///
/// Mirrors portal's private `keccak_writer` for the same reason it exists there:
/// `borsh::to_vec(&order)` would allocate a full second copy of the order — route
/// segments included — on a heap that never frees. On the default 32 KB heap that
/// copy is a material fraction of the program's entire budget, and it is pure
/// waste when the bytes are only ever fed to a hasher.
///
/// `order_hash_does_not_allocate` pins the property.
pub struct KeccakWriter<'a>(&'a mut Keccak);

impl<'a> KeccakWriter<'a> {
    pub fn new(hasher: &'a mut Keccak) -> Self {
        Self(hasher)
    }
}

impl io::Write for KeccakWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
