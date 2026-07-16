//! The `MachineImage` — the plain-data snapshot of an endor machine's
//! serializable state — and the [`write_machine`]/[`read_machine`] codec
//! that maps it to and from the `XS_M` atom container.
//!
//! This is the **narrow, documented surface child 3 calls** (job spec):
//! child 3's `Machine`-level `write_snapshot_to_file`/`from_snapshot_file`/
//! `suspend_to_cas` build a `MachineImage` from a live `Interp` (reading
//! its private fields) and stream [`write_machine`]'s bytes; on restore it
//! [`read_machine`]s the bytes into a `MachineImage` and rebuilds the
//! arenas with [`endor_vm::SlotArena::from_image`] /
//! [`endor_vm::ChunkArena::from_image`]. This crate owns the *format*; the
//! `Interp`↔image conversion stays in the engine.
//!
//! Coverage today: the index arenas (`HEAP`/`BLOC`), the interpreter stack
//! (`STAC`), and the symbol/key tables (`NAME`/`KEYS`/`SYMB`), plus the
//! `VERS`/`SIGN`/`CREA` headers. The rich per-instance side tables are
//! enumerated in [`crate::sidetable`] with their coverage; the ones marked
//! `Pending` there are the remaining atoms.

use crate::atom::{AtomReader, AtomWriter};
use crate::format::{
    Signature, SnapshotError, Version, BLOC, CREA, HEAP, KEYS, NAME, SIGN, STAC, SYMB, VERS,
};
use crate::slot_codec::{decode_slots, encode_slots, SLOT_RECORD_BYTES};
use endor_vm::{ChunkArena, Slot, SlotArena};

/// Machine creation parameters (`CREA`). The heap-sizing hints XS records
/// so a restore can pre-size the arenas; endor's arenas grow on demand, so
/// these are advisory (recorded for fidelity and future pre-sizing).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CreationParams {
    pub initial_slot_count: u32,
    pub initial_chunk_bytes: u32,
}

impl CreationParams {
    fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(8);
        v.extend_from_slice(&self.initial_slot_count.to_be_bytes());
        v.extend_from_slice(&self.initial_chunk_bytes.to_be_bytes());
        v
    }
    fn decode(p: &[u8]) -> Result<CreationParams, SnapshotError> {
        if p.len() < 8 {
            return Err(SnapshotError::Corrupt("CREA payload too short"));
        }
        Ok(CreationParams {
            initial_slot_count: u32::from_be_bytes([p[0], p[1], p[2], p[3]]),
            initial_chunk_bytes: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        })
    }
}

/// The serializable image of an endor machine.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineImage {
    pub version: Version,
    pub signature: Signature,
    pub creation: CreationParams,
    /// `BLOC`: the chunk arena bytes, header discipline included.
    pub chunks: Vec<u8>,
    /// `HEAP`: every slot record (live and free alike), index-ordered.
    pub slots: Vec<Slot>,
    /// `HEAP`: the slot arena's free list.
    pub slot_free: Vec<u32>,
    /// `HEAP`: the live slot count (`currentHeapCount`).
    pub slot_live: u32,
    /// `STAC`: the interpreter's live stack slots.
    pub stack: Vec<Slot>,
    /// `KEYS`: runtime-interned property key names.
    pub keys: Vec<String>,
    /// `NAME`: the program symbol names, id-ordered (`symbol_names`).
    pub names: Vec<String>,
    /// `SYMB`: well-known / registered symbol identity slot indices.
    pub symbols: Vec<u32>,
}

impl MachineImage {
    /// Build an image straight from a pair of arenas plus the stack and
    /// symbol tables — the arena-(de)serialization surface. The caller
    /// supplies the machine signature (its callback-table version).
    pub fn from_arenas(
        signature: Signature,
        slots: &SlotArena,
        chunks: &ChunkArena,
        stack: &[Slot],
        names: Vec<String>,
        keys: Vec<String>,
        symbols: Vec<u32>,
    ) -> MachineImage {
        MachineImage {
            version: Version::current(),
            signature,
            creation: CreationParams {
                initial_slot_count: slots.capacity(),
                initial_chunk_bytes: chunks.byte_size() as u32,
            },
            chunks: chunks.raw().to_vec(),
            slots: slots.records().to_vec(),
            slot_free: slots.free_list().to_vec(),
            slot_live: slots.live_count(),
            stack: stack.to_vec(),
            keys,
            names,
            symbols,
        }
    }

    /// Rebuild the slot and chunk arenas from this image. Round-trips the
    /// index arenas exactly (indices preserved, free list preserved).
    pub fn to_arenas(&self) -> (SlotArena, ChunkArena) {
        let slots = SlotArena::from_image(self.slots.clone(), self.slot_free.clone(), self.slot_live);
        let chunks = ChunkArena::from_image(self.chunks.clone());
        (slots, chunks)
    }
}

// --- string-list and slot-list atom payload helpers ---

fn encode_strings(list: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(list.len() as u32).to_be_bytes());
    for s in list {
        let b = s.as_bytes();
        v.extend_from_slice(&(b.len() as u32).to_be_bytes());
        v.extend_from_slice(b);
    }
    v
}

fn decode_strings(p: &[u8]) -> Result<Vec<String>, SnapshotError> {
    if p.len() < 4 {
        return Err(SnapshotError::Corrupt("string list header"));
    }
    let count = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut i = 4;
    for _ in 0..count {
        if i + 4 > p.len() {
            return Err(SnapshotError::Corrupt("string list entry header"));
        }
        let len = u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]) as usize;
        i += 4;
        if i + len > p.len() {
            return Err(SnapshotError::Corrupt("string list entry body"));
        }
        let s = std::str::from_utf8(&p[i..i + len])
            .map_err(|_| SnapshotError::Corrupt("string list entry not utf8"))?;
        out.push(s.to_string());
        i += len;
    }
    Ok(out)
}

fn encode_u32s(list: &[u32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + list.len() * 4);
    v.extend_from_slice(&(list.len() as u32).to_be_bytes());
    for &x in list {
        v.extend_from_slice(&x.to_be_bytes());
    }
    v
}

fn decode_u32s(p: &[u8]) -> Result<Vec<u32>, SnapshotError> {
    if p.len() < 4 {
        return Err(SnapshotError::Corrupt("u32 list header"));
    }
    let count = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut i = 4;
    for _ in 0..count {
        if i + 4 > p.len() {
            return Err(SnapshotError::Corrupt("u32 list entry"));
        }
        out.push(u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]));
        i += 4;
    }
    Ok(out)
}

/// HEAP payload: `[slot_count][free_count][live][free…][records…]`.
fn encode_heap(image: &MachineImage) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(image.slots.len() as u32).to_be_bytes());
    v.extend_from_slice(&(image.slot_free.len() as u32).to_be_bytes());
    v.extend_from_slice(&image.slot_live.to_be_bytes());
    for &f in &image.slot_free {
        v.extend_from_slice(&f.to_be_bytes());
    }
    v.extend_from_slice(&encode_slots(&image.slots));
    v
}

fn decode_heap(p: &[u8]) -> Result<(Vec<Slot>, Vec<u32>, u32), SnapshotError> {
    if p.len() < 12 {
        return Err(SnapshotError::Corrupt("HEAP header"));
    }
    let slot_count = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
    let free_count = u32::from_be_bytes([p[4], p[5], p[6], p[7]]) as usize;
    let live = u32::from_be_bytes([p[8], p[9], p[10], p[11]]);
    let mut i = 12;
    let mut free = Vec::with_capacity(free_count);
    for _ in 0..free_count {
        if i + 4 > p.len() {
            return Err(SnapshotError::Corrupt("HEAP free list"));
        }
        free.push(u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]));
        i += 4;
    }
    let want = slot_count * SLOT_RECORD_BYTES;
    if p.len() - i < want {
        return Err(SnapshotError::Corrupt("HEAP records truncated"));
    }
    let slots = decode_slots(&p[i..i + want]).map_err(|_| SnapshotError::Corrupt("HEAP slot record"))?;
    Ok((slots, free, live))
}

/// STAC payload: `[count][records…]`.
fn encode_stack(stack: &[Slot]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(stack.len() as u32).to_be_bytes());
    v.extend_from_slice(&encode_slots(stack));
    v
}

fn decode_stack(p: &[u8]) -> Result<Vec<Slot>, SnapshotError> {
    if p.len() < 4 {
        return Err(SnapshotError::Corrupt("STAC header"));
    }
    let count = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
    let want = count * SLOT_RECORD_BYTES;
    if p.len() - 4 < want {
        return Err(SnapshotError::Corrupt("STAC records truncated"));
    }
    decode_slots(&p[4..4 + want]).map_err(|_| SnapshotError::Corrupt("STAC slot record"))
}

/// Serialize a machine image into an `XS_M` atom container. Atoms are
/// written in the canonical order `VERS SIGN CREA BLOC HEAP STAC KEYS NAME
/// SYMB` (the order `xsSnapshot.c` emits), so two writes of the same image
/// are byte-identical.
pub fn write_machine(image: &MachineImage) -> Vec<u8> {
    let mut w = AtomWriter::new();
    w.atom(VERS, &image.version.encode());
    w.atom(SIGN, &image.signature.encode());
    w.atom(CREA, &image.creation.encode());
    w.atom(BLOC, &image.chunks);
    w.atom(HEAP, &encode_heap(image));
    w.atom(STAC, &encode_stack(&image.stack));
    w.atom(KEYS, &encode_strings(&image.keys));
    w.atom(NAME, &encode_strings(&image.names));
    w.atom(SYMB, &encode_u32s(&image.symbols));
    w.finish()
}

/// Parse an `XS_M` atom container into a machine image, enforcing the
/// endor `VERS` discriminator and checking the host callback-table
/// `SIGN` against `expected_sig` — a mismatch fails closed exactly as
/// `fxReadSnapshot` does (a callback index would bind the wrong host
/// function). Pass the machine's current signature.
pub fn read_machine(buf: &[u8], expected_sig: &Signature) -> Result<MachineImage, SnapshotError> {
    let r = AtomReader::parse(buf)?;

    let vers = r.find(VERS).ok_or(SnapshotError::MissingAtom(VERS))?;
    let version = Version::decode(vers.payload)?;

    let sign = r.find(SIGN).ok_or(SnapshotError::MissingAtom(SIGN))?;
    let signature = Signature::decode(sign.payload)?;
    if !signature.is_compatible_with(expected_sig) {
        return Err(SnapshotError::SignatureMismatch {
            expected: expected_sig.clone(),
            found: signature,
        });
    }

    let creation = match r.find(CREA) {
        Some(a) => CreationParams::decode(a.payload)?,
        None => CreationParams::default(),
    };
    let chunks = r.find(BLOC).map(|a| a.payload.to_vec()).unwrap_or_default();

    let heap = r.find(HEAP).ok_or(SnapshotError::MissingAtom(HEAP))?;
    let (slots, slot_free, slot_live) = decode_heap(heap.payload)?;

    let stack = match r.find(STAC) {
        Some(a) => decode_stack(a.payload)?,
        None => Vec::new(),
    };
    let keys = match r.find(KEYS) {
        Some(a) => decode_strings(a.payload)?,
        None => Vec::new(),
    };
    let names = match r.find(NAME) {
        Some(a) => decode_strings(a.payload)?,
        None => Vec::new(),
    };
    let symbols = match r.find(SYMB) {
        Some(a) => decode_u32s(a.payload)?,
        None => Vec::new(),
    };

    Ok(MachineImage {
        version,
        signature,
        creation,
        chunks,
        slots,
        slot_free,
        slot_live,
        stack,
        keys,
        names,
        symbols,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use endor_vm::{Kind, Payload, SlotIndex};

    fn sig() -> Signature {
        Signature::new("endor-test-sig-v1")
    }

    #[test]
    fn empty_machine_round_trips_byte_equal() {
        let img = MachineImage {
            version: Version::current(),
            signature: sig(),
            creation: CreationParams::default(),
            chunks: Vec::new(),
            slots: Vec::new(),
            slot_free: Vec::new(),
            slot_live: 0,
            stack: Vec::new(),
            keys: Vec::new(),
            names: Vec::new(),
            symbols: Vec::new(),
        };
        let bytes = write_machine(&img);
        let back = read_machine(&bytes, &sig()).unwrap();
        assert_eq!(back, img);
        // Second write byte-equals the first.
        assert_eq!(write_machine(&back), bytes);
    }

    #[test]
    fn signature_mismatch_fails_closed() {
        let img = MachineImage {
            version: Version::current(),
            signature: Signature::new("written-under-v1"),
            creation: CreationParams::default(),
            chunks: Vec::new(),
            slots: Vec::new(),
            slot_free: Vec::new(),
            slot_live: 0,
            stack: Vec::new(),
            keys: Vec::new(),
            names: Vec::new(),
            symbols: Vec::new(),
        };
        let bytes = write_machine(&img);
        match read_machine(&bytes, &Signature::new("host-is-now-v2")) {
            Err(SnapshotError::SignatureMismatch { .. }) => {}
            other => panic!("expected signature mismatch, got {:?}", other),
        }
    }

    /// Build an arena with an object graph (an instance whose two property
    /// slots hold an integer and a heap string), a closure cell, and a
    /// stack, and round-trip it through the atom container — the arena
    /// (de)serialization surface, write → read → write byte-equal.
    #[test]
    fn arena_graph_round_trips() {
        let mut slots = SlotArena::new();
        let mut chunks = ChunkArena::new();

        // A heap string "hi" (UTF-16BE).
        let hi = chunks.alloc(&[0x00, 0x68, 0x00, 0x69]);
        // The shared closure cell holding integer 7.
        let cell = slots.alloc(Slot::integer(7));
        // A property list: {a: 5, b: "hi"} on an instance.
        let prop_b = slots.alloc(Slot::property(2, Payload::String(hi)));
        let mut prop_a = Slot::property(1, Payload::Integer(5));
        prop_a.next = prop_b;
        let prop_a_i = slots.alloc(prop_a);
        let mut inst = Slot::instance(SlotIndex::NULL);
        inst.next = prop_a_i;
        let inst_i = slots.alloc(inst);
        // A closure scope slot indirecting to the cell.
        let closure = slots.alloc(Slot::of(Kind::Closure, Payload::Reference(cell)));

        // Free one slot to exercise the free-list round-trip.
        let scratch = slots.alloc(Slot::integer(0));
        slots.free(scratch);

        let stack = vec![
            Slot::of(Kind::Reference, Payload::Reference(inst_i)),
            Slot::of(Kind::Closure, Payload::Reference(cell)),
            Slot::of(Kind::String, Payload::String(hi)),
        ];
        let _ = closure;

        let img = MachineImage::from_arenas(
            sig(),
            &slots,
            &chunks,
            &stack,
            vec!["length".to_string(), "name".to_string()],
            vec!["dynKey".to_string()],
            vec![101, 202],
        );

        let bytes = write_machine(&img);
        let back = read_machine(&bytes, &sig()).unwrap();
        assert_eq!(back, img);
        // Byte-equality of the second write.
        assert_eq!(write_machine(&back), bytes);

        // Structural: the rebuilt arenas reproduce the graph.
        let (slots2, chunks2) = back.to_arenas();
        assert_eq!(slots2.capacity(), slots.capacity());
        assert_eq!(slots2.live_count(), slots.live_count());
        // The instance's first property is the integer 5.
        let inst2 = slots2.get(inst_i);
        let pa = slots2.get(inst2.next);
        assert_eq!(pa.value, Payload::Integer(5));
        // Its successor property references the "hi" chunk; decode it back.
        let pb = slots2.get(pa.next);
        if let Payload::String(o) = pb.value {
            assert_eq!(chunks2.payload(o), &[0x00, 0x68, 0x00, 0x69]);
        } else {
            panic!("second property should be a string");
        }
        // The closure cell survived with its value.
        assert_eq!(slots2.get(cell).value, Payload::Integer(7));
    }

    #[test]
    fn string_and_symbol_tables_round_trip() {
        let img = MachineImage {
            version: Version::current(),
            signature: sig(),
            creation: CreationParams {
                initial_slot_count: 3,
                initial_chunk_bytes: 16,
            },
            chunks: vec![1, 2, 3, 4],
            slots: vec![Slot::integer(9)],
            slot_free: vec![],
            slot_live: 1,
            stack: vec![Slot::boolean(true)],
            keys: vec!["k1".to_string(), "k2".to_string(), "".to_string()],
            names: vec!["Object".to_string(), "length".to_string()],
            symbols: vec![7, 8, 9],
        };
        let bytes = write_machine(&img);
        let back = read_machine(&bytes, &sig()).unwrap();
        assert_eq!(back, img);
    }

    #[test]
    fn missing_heap_atom_is_rejected() {
        // A hand-built container with VERS+SIGN but no HEAP.
        use crate::atom::AtomWriter;
        let mut w = AtomWriter::new();
        w.atom(VERS, &Version::current().encode());
        w.atom(SIGN, &sig().encode());
        let bytes = w.finish();
        assert_eq!(
            read_machine(&bytes, &sig()),
            Err(SnapshotError::MissingAtom(HEAP))
        );
    }

    #[test]
    fn bigint_chunk_survives() {
        // A BigInt slot referencing a chunk of little-endian digits.
        let mut chunks = ChunkArena::new();
        let digits = chunks.alloc(&[0x00, 0x01, 0x00, 0x00, 0x00]); // sign + LE u32
        let mut slots = SlotArena::new();
        let bi = slots.alloc(Slot::of(Kind::BigInt, Payload::BigInt(digits)));
        let _ = bi;
        let img = MachineImage::from_arenas(
            sig(),
            &slots,
            &chunks,
            &[],
            vec![],
            vec![],
            vec![],
        );
        let bytes = write_machine(&img);
        let back = read_machine(&bytes, &sig()).unwrap();
        assert_eq!(write_machine(&back), bytes);
        let (slots2, chunks2) = back.to_arenas();
        if let Payload::BigInt(o) = slots2.get(bi).value {
            assert_eq!(chunks2.payload(o), &[0x00, 0x01, 0x00, 0x00, 0x00]);
        } else {
            panic!("bigint payload");
        }
    }
}
