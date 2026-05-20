use std::io;

use tiny_keccak::{Hasher, Keccak};

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
