use crate::Result;
use crate::bundle::Bundle;
use std::path::Path;

/// Write a `.sqrb` bundle.
pub fn write(bundle: &Bundle, path: impl AsRef<Path>) -> Result<()> {
    crate::formats::write_sqrb(bundle, path)
}

/// Read a `.sqrb` bundle.
pub fn read(path: impl AsRef<Path>) -> Result<Bundle> {
    crate::formats::read_sqrb(path)
}

/// Expand a `.sqrb` bundle into JSON files.
pub fn unpack(path: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> Result<()> {
    crate::formats::unpack_sqrb(path, output_dir)
}
