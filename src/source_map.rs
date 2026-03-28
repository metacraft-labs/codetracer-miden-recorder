//! Source mapping for Miden VM programs.
//!
//! Maps byte offsets from Miden's debug `Location` back to line numbers
//! by building a line-offset table from the original source text.

use std::path::{Path, PathBuf};

/// Maps byte offsets in a MASM source file to 1-based line numbers.
pub struct SourceMap {
    path: PathBuf,
    /// The original source code.
    source_code: String,
    /// Byte offset of the start of each line (0-indexed).
    /// `line_offsets[0]` is always 0.
    line_offsets: Vec<u32>,
}

impl SourceMap {
    /// Build a `SourceMap` from a source file path and its contents.
    pub fn from_source(path: &Path, source: &str) -> Self {
        let mut line_offsets = vec![0u32];
        for (i, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_offsets.push((i + 1) as u32);
            }
        }
        Self {
            path: path.to_path_buf(),
            source_code: source.to_string(),
            line_offsets,
        }
    }

    /// Convert a byte offset to a 1-based line number.
    ///
    /// Uses binary search over the line-start offsets.
    pub fn byte_to_line(&self, byte_offset: u32) -> u32 {
        // Binary search: find the last line_offset <= byte_offset
        match self.line_offsets.binary_search(&byte_offset) {
            Ok(idx) => (idx + 1) as u32,
            Err(idx) => idx as u32, // idx is the insertion point; the line is idx (1-based)
        }
    }

    /// The path of the source file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The original source code.
    pub fn source_code(&self) -> &str {
        &self.source_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_to_line_simple() {
        let source = "line1\nline2\nline3\n";
        let sm = SourceMap::from_source(Path::new("test.masm"), source);
        // "line1\n" occupies bytes 0..6
        assert_eq!(sm.byte_to_line(0), 1); // start of line 1
        assert_eq!(sm.byte_to_line(3), 1); // middle of line 1
        assert_eq!(sm.byte_to_line(6), 2); // start of line 2
        assert_eq!(sm.byte_to_line(12), 3); // start of line 3
    }

    #[test]
    fn test_byte_to_line_compute_masm() {
        let source = "proc.compute.5\n    push.10 loc_store.0\n    push.32 loc_store.1\n    loc_load.0 loc_load.1 add loc_store.2\n    loc_load.2 dup add loc_store.3\n    loc_load.3 loc_load.0 add loc_store.4\nend\n\nbegin\n    exec.compute\nend\n";
        let sm = SourceMap::from_source(Path::new("compute.masm"), source);
        assert_eq!(sm.byte_to_line(0), 1); // "proc.compute.5"
        assert_eq!(sm.byte_to_line(15), 2); // "    push.10 ..."
    }
}
