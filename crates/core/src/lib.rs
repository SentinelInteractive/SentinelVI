//! `sentinel-rpf9-core` — parsing, carving and decoding core for GTA VI
//! container formats (RPF9 archives, RDS5/ETPF texture databases and RAGE9
//! resource pages).
//!
//! Design rules for this crate:
//!
//! * Every decoder works on `&[u8]` slices. Nothing is copied unless the format
//!   requires it (swizzle, block decompression, zlib).
//! * No decoder assumes it can hold an archive in RAM. All file access goes
//!   through [`Source`], which is either an mmap window or a positioned reader.
//! * Decoders are pure: same bytes in, same bytes out. That makes every layer
//!   testable against a captured fixture.
//!
//! The knowledge in here was derived from the file formats themselves. It is an
//! independent implementation and contains no code from any third-party viewer.

pub mod awc;
pub mod bc;
pub mod cache;
pub mod img;
pub mod mhiu;
pub mod psin;
pub mod read;
pub mod rpf9;
pub mod source;
pub mod swizzle;
pub mod tpfd;
pub mod mesh;
pub mod zlib;

pub use read::{Be, Le};
pub use rpf9::{Header, Rpf9Error, ScanHit};
pub use source::{BytesSource, MmapSource, Source, StreamSource};

/// Version string reported by the CLI and the library.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
