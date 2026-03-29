//! Tests for Rust-via-midenc support (M4).
//!
//! These tests use synthetic data to verify the multi-file source mapping,
//! source language detection, synthetic location filtering, and Rust source
//! file resolution — without requiring the midenc toolchain.

use std::path::Path;

use codetracer_miden_recorder::rust_support::{
    RustSourceResolver, SourceLanguage, is_synthetic_location,
};
use codetracer_miden_recorder::source_map::MultiSourceMap;

// ---------------------------------------------------------------------------
// Test 1: MultiSourceMap — add two files, resolve locations from both
// ---------------------------------------------------------------------------

#[test]
fn test_multi_source_map_basic() {
    let mut msm = MultiSourceMap::new();

    // Simulate two Rust source files that midenc would reference.
    let rust_src_a = "fn main() {\n    let x = 10;\n    let y = 20;\n}\n";
    let rust_src_b = "fn helper() {\n    let a = 1;\n}\n";

    msm.add_file(
        "/project/src/main.rs",
        Path::new("/project/src/main.rs"),
        rust_src_a,
    );
    msm.add_file(
        "/project/src/helper.rs",
        Path::new("/project/src/helper.rs"),
        rust_src_b,
    );

    // Resolve from main.rs: byte 0 = line 1, byte 12 (start of "    let x") = line 2
    let (path_a, line_a) = msm
        .resolve("/project/src/main.rs", 0)
        .expect("should resolve main.rs offset 0");
    assert_eq!(path_a, Path::new("/project/src/main.rs"));
    assert_eq!(line_a, 1);

    let (path_a2, line_a2) = msm
        .resolve("/project/src/main.rs", 12)
        .expect("should resolve main.rs offset 12");
    assert_eq!(path_a2, Path::new("/project/src/main.rs"));
    assert_eq!(line_a2, 2);

    // Resolve from helper.rs: byte 0 = line 1, byte 14 (start of "    let a") = line 2
    let (path_b, line_b) = msm
        .resolve("/project/src/helper.rs", 0)
        .expect("should resolve helper.rs offset 0");
    assert_eq!(path_b, Path::new("/project/src/helper.rs"));
    assert_eq!(line_b, 1);

    let (path_b2, line_b2) = msm
        .resolve("/project/src/helper.rs", 14)
        .expect("should resolve helper.rs offset 14");
    assert_eq!(path_b2, Path::new("/project/src/helper.rs"));
    assert_eq!(line_b2, 2);
}

// ---------------------------------------------------------------------------
// Test 2: MultiSourceMap — unknown URI returns None
// ---------------------------------------------------------------------------

#[test]
fn test_multi_source_map_missing_file() {
    let msm = MultiSourceMap::new();

    assert!(
        msm.resolve("/nonexistent/file.rs", 0).is_none(),
        "resolve should return None for unknown URI"
    );
    assert!(
        !msm.contains("/nonexistent/file.rs"),
        "contains should return false for unknown URI"
    );
}

// ---------------------------------------------------------------------------
// Test 3: SourceLanguage detection
// ---------------------------------------------------------------------------

#[test]
fn test_source_language_detection() {
    assert_eq!(
        SourceLanguage::from_path("compute.masm"),
        SourceLanguage::Masm
    );
    assert_eq!(
        SourceLanguage::from_path("/path/to/lib.rs"),
        SourceLanguage::Rust
    );
    assert_eq!(
        SourceLanguage::from_path("program.move"),
        SourceLanguage::Unknown
    );
    assert_eq!(SourceLanguage::from_path(""), SourceLanguage::Unknown);
    assert_eq!(
        SourceLanguage::from_path("/home/user/project/src/main.rs"),
        SourceLanguage::Rust
    );
    assert_eq!(
        SourceLanguage::from_path("test-programs/masm/compute.masm"),
        SourceLanguage::Masm
    );
    // Edge cases
    assert_eq!(SourceLanguage::from_path("file.RS"), SourceLanguage::Unknown);
    assert_eq!(SourceLanguage::from_path("file.MASM"), SourceLanguage::Unknown);
}

// ---------------------------------------------------------------------------
// Test 4: Synthetic location filtering
// ---------------------------------------------------------------------------

#[test]
fn test_synthetic_location_filtering() {
    use std::sync::Arc;
    use miden_core::debuginfo::{ByteIndex, Location};

    // A normal location with a real path and nonzero offsets is not synthetic.
    let normal_loc = Location::new(
        Arc::from("/project/src/main.rs"),
        ByteIndex::new(10),
        ByteIndex::new(25),
    );
    assert!(
        !is_synthetic_location(&normal_loc),
        "normal location should not be synthetic"
    );

    // A location with an empty path is synthetic.
    let empty_path_loc = Location::new(
        Arc::from(""),
        ByteIndex::new(0),
        ByteIndex::new(0),
    );
    assert!(
        is_synthetic_location(&empty_path_loc),
        "empty path location should be synthetic"
    );

    // A location with <synthetic> marker and zero offsets is synthetic.
    let synthetic_loc = Location::new(
        Arc::from("<synthetic>"),
        ByteIndex::new(0),
        ByteIndex::new(0),
    );
    assert!(
        is_synthetic_location(&synthetic_loc),
        "<synthetic> location should be synthetic"
    );

    // A normal location at offset 0 with a real path is not synthetic.
    let zero_offset_loc = Location::new(
        Arc::from("/project/src/lib.rs"),
        ByteIndex::new(0),
        ByteIndex::new(0),
    );
    assert!(
        !is_synthetic_location(&zero_offset_loc),
        "zero offset with real path should not be synthetic"
    );
}

// ---------------------------------------------------------------------------
// Test 5: RustSourceResolver finds file in search dir
// ---------------------------------------------------------------------------

#[test]
fn test_rust_source_resolver_finds_file() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let src_dir = tmp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("failed to create src dir");

    // Create a .rs file in the search directory.
    let rs_path = src_dir.join("lib.rs");
    std::fs::write(&rs_path, "fn main() {}").expect("failed to write file");

    let resolver = RustSourceResolver::new(vec![src_dir.clone()]);

    // Should find the file by filename when given a non-existent absolute path.
    let result = resolver.resolve("/original/build/path/lib.rs");
    assert!(result.is_some(), "resolver should find lib.rs in search dir");
    assert_eq!(result.unwrap(), rs_path);
}

// ---------------------------------------------------------------------------
// Test 6: RustSourceResolver returns None for missing files
// ---------------------------------------------------------------------------

#[test]
fn test_rust_source_resolver_missing() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let resolver = RustSourceResolver::new(vec![tmp_dir.path().to_path_buf()]);

    let result = resolver.resolve("/nonexistent/path/missing.rs");
    assert!(
        result.is_none(),
        "resolver should return None for missing files"
    );
}
