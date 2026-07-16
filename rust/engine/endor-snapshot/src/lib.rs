#![forbid(unsafe_code)]
//! endor-snapshot: the endor XS_M atom-container writer/reader and
//! index-arena heap serializer (design `designs/xs2rust-endor-engine.md`
//! § Snapshots, requirement 1c; the callback-signature discipline from
//! `designs/daemon-xs-worker-snapshot.md`).
//!
//! Endor writes and reads the length-prefixed big-endian FourCC atom
//! grammar — `XS_M` over `VERS`/`SIGN`/`CREA`/`BLOC`/`HEAP`/`STAC`/
//! `KEYS`/`NAME`/`SYMB` — with an **endor `VERS` discriminator** so an
//! endor snapshot is never mistaken for a C-XS one and vice versa. The
//! C-XS importer is out of scope (resolved question 3): this crate is the
//! Rust-native writer and reader only.
//!
//! Because the heap is index arenas, the writer is a **serializer, not a
//! relocator**: a `SlotIndex`/`ChunkOffset` is already position-
//! independent, so the `HEAP`/`BLOC` atoms are the flat arena images and a
//! read reconstructs identical arenas ([`endor_vm::SlotArena::from_image`]
//! / [`endor_vm::ChunkArena::from_image`]).
//!
//! # Surface (what child 3 builds on)
//!
//! - [`atom`] — the raw FourCC atom container ([`atom::AtomWriter`] /
//!   [`atom::AtomReader`]).
//! - [`format`] — the tags, the endor [`format::Version`] discriminator,
//!   and the host callback-table [`format::Signature`] scheme.
//! - [`slot_codec`] — [`endor_vm::Slot`] ↔ fixed-width record.
//! - [`image`] — [`image::MachineImage`] plus [`image::write_machine`] /
//!   [`image::read_machine`], the narrow API the `Machine`-level
//!   `write_snapshot_to_file`/`from_snapshot_file`/`suspend_to_cas`
//!   (child 3) call. This crate deliberately does **not** wire that
//!   `Machine` surface.
//!
//! # Side-table completeness (the bug class this crate designs against)
//!
//! A machine's reachable state is not wholly in the arenas: dozens of
//! `Interp` side tables hold per-instance and per-activation state. An
//! atom grammar that misses one is the snapshot-shaped version of a
//! missing GC root. [`sidetable::SideTable`] enumerates them explicitly,
//! one compiler-forced variant per table, each with its current
//! [`sidetable::Coverage`] — so the remaining atoms are a compile-checked
//! ledger, never a silent omission.
//!
//! The whole crate is `#![forbid(unsafe_code)]` (the engine unsafe budget
//! is zero, design § Minimizing `unsafe`).

pub mod atom;
pub mod format;
pub mod image;
pub mod sidetable;
pub mod slot_codec;

pub use atom::{Atom, AtomError, AtomReader, AtomWriter};
pub use format::{
    FourCc, Signature, SignatureError, SnapshotError, Version, VersionError, BLOC, CREA, HEAP,
    KEYS, NAME, SIGN, STAC, SYMB, VERS, XS_M,
};
pub use image::{read_machine, write_machine, CreationParams, MachineImage};
pub use sidetable::{Coverage, Descriptor, SideTable};
pub use slot_codec::{decode_slot, decode_slots, encode_slot, encode_slots, SLOT_RECORD_BYTES};
