use crate::Result;
use crate::bundle::Bundle;
use std::path::Path;

/// Write a `.sqrj.json` bundle.
pub fn write(bundle: &Bundle, path: impl AsRef<Path>) -> Result<()> {
    crate::formats::write_sqrj(bundle, path)
}

/// Read a `.sqrj.json` bundle.
pub fn read(path: impl AsRef<Path>) -> Result<Bundle> {
    crate::formats::read_sqrj(path)
}
