//! The index-arena value and heap model (design § Value and heap
//! model). XS's pointer-linked slot graph becomes index arenas:
//!
//! - `SlotIndex(u32)` replaces `txSlot*`; the slot heap is an arena of
//!   32-byte slot records with a free list (XS's "slots never move").
//! - `ChunkOffset(u32)` replaces chunk pointers; the chunk heap is a
//!   growable byte arena with the same header discipline, ready for the
//!   slide-compaction GC that lands in stage 2.
//!
//! The 32-byte record layout is held exactly (resolved question 5) so
//! `currentHeapCount` semantics and snapshot slot images stay aligned
//! with the oracle: kind + flag + 16-bit id + next-index + 16-byte
//! payload. Stage 1 exercises the immediate value kinds (undefined,
//! null, boolean, integer, number); reference/string kinds carry their
//! arena handles and are filled in as later stages land the object
//! model and GC.

/// Handle into the slot arena. `u32::MAX` is the null sentinel
/// (XS's `C_NULL`), never a live index.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotIndex(pub u32);

impl SlotIndex {
    pub const NULL: SlotIndex = SlotIndex(u32::MAX);
    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == u32::MAX
    }
}

/// Handle into the chunk (byte) arena.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChunkOffset(pub u32);

impl ChunkOffset {
    pub const NULL: ChunkOffset = ChunkOffset(u32::MAX);
    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == u32::MAX
    }
}

/// Slot kind byte. Values mirror the XS `XS_*_KIND` ordering for the
/// kinds stage 1 uses; the full ~66-kind set arrives with the object
/// model in stage 2.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Undefined = 0,
    Null = 1,
    Boolean = 2,
    Integer = 3,
    Number = 4,
    /// String data living in the chunk arena (CESU-8, resolved
    /// question 4). Payload holds a `ChunkOffset`.
    String = 5,
    /// Reference to a heap instance (slot arena). Payload holds a
    /// `SlotIndex`. Populated in stage 2.
    Reference = 10,
}

/// The 16-byte value payload (XS's value union arm subset for stage 1).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Payload {
    None,
    Boolean(bool),
    /// `txInteger` is 32-bit in XS.
    Integer(i32),
    Number(f64),
    String(ChunkOffset),
    Reference(SlotIndex),
}

/// One 32-byte slot record. The struct is deliberately compact; the
/// `#[repr(C)]`-style field order matches XS's `txSlot` (next, id,
/// flag, kind, value) so a future snapshot writer is a serializer, not
/// a relocator (design § Snapshots).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Slot {
    /// XS `next` link (property lists, frame chains): a slot index.
    pub next: SlotIndex,
    /// XS 16-bit `ID`. Doubles as the argument count on frame slots.
    pub id: u16,
    /// XS `flag` byte.
    pub flag: u8,
    pub kind: Kind,
    pub value: Payload,
}

impl Slot {
    #[inline]
    pub fn undefined() -> Slot {
        Slot::of(Kind::Undefined, Payload::None)
    }
    #[inline]
    pub fn null() -> Slot {
        Slot::of(Kind::Null, Payload::None)
    }
    #[inline]
    pub fn boolean(b: bool) -> Slot {
        Slot::of(Kind::Boolean, Payload::Boolean(b))
    }
    #[inline]
    pub fn integer(i: i32) -> Slot {
        Slot::of(Kind::Integer, Payload::Integer(i))
    }
    #[inline]
    pub fn number(n: f64) -> Slot {
        Slot::of(Kind::Number, Payload::Number(n))
    }
    #[inline]
    pub fn of(kind: Kind, value: Payload) -> Slot {
        Slot {
            next: SlotIndex::NULL,
            id: 0,
            flag: 0,
            kind,
            value,
        }
    }

    /// Invoke `f` with every slot index this slot references as a GC
    /// edge: its `next` link (property/frame chains) and any
    /// reference-bearing payload arm. The mark-sweep tracer
    /// ([`crate::gc`]) uses this; extend it whenever a new
    /// reference-bearing [`Payload`] arm lands.
    #[inline]
    pub fn each_ref_slot(&self, mut f: impl FnMut(SlotIndex)) {
        if !self.next.is_null() {
            f(self.next);
        }
        match self.value {
            Payload::Reference(r) => f(r),
            _ => {}
        }
    }

    /// The chunk this slot references (a heap-string payload), if any —
    /// what the slide-compactor must relocate and rewrite.
    #[inline]
    pub fn chunk_ref(&self) -> Option<ChunkOffset> {
        match self.value {
            Payload::String(o) => Some(o),
            _ => None,
        }
    }

    /// Rewrite this slot's chunk reference after compaction. A no-op on
    /// a slot that holds no chunk offset.
    #[inline]
    pub fn set_chunk_ref(&mut self, off: ChunkOffset) {
        if let Payload::String(_) = self.value {
            self.value = Payload::String(off);
        }
    }

    #[inline]
    pub fn as_integer(&self) -> Option<i32> {
        match self.value {
            Payload::Integer(i) => Some(i),
            _ => None,
        }
    }
    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        match self.value {
            Payload::Number(n) => Some(n),
            _ => None,
        }
    }
}

/// A slot arena: fixed-size 32-byte records that never move, with a
/// free list. This is XS's slot heap; the mark-sweep collector
/// ([`crate::gc`]) sweeps it to the free list (design § Value and heap
/// model). Because it is index-based it is safe code: a stale index is
/// a kind-checked logic bug, not undefined behavior.
#[derive(Default)]
pub struct SlotArena {
    slots: Vec<Slot>,
    free: Vec<u32>,
    /// One mark bit per slot, used by the mark-sweep collector. A slot
    /// is never both free and marked; the collector clears all marks
    /// before a collection and sweeps the unmarked-and-not-already-free
    /// slots onto the free list.
    marks: Vec<bool>,
    /// Count of live (non-free) slots, mirroring `currentHeapCount`.
    live: u32,
}

impl SlotArena {
    pub fn new() -> SlotArena {
        SlotArena {
            slots: Vec::new(),
            free: Vec::new(),
            marks: Vec::new(),
            live: 0,
        }
    }

    /// Allocate a slot, reusing the free list first (XS semantics).
    pub fn alloc(&mut self, slot: Slot) -> SlotIndex {
        self.live += 1;
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = slot;
            self.marks[i as usize] = false;
            SlotIndex(i)
        } else {
            let i = self.slots.len() as u32;
            self.slots.push(slot);
            self.marks.push(false);
            SlotIndex(i)
        }
    }

    /// Return a slot to the free list.
    pub fn free(&mut self, index: SlotIndex) {
        debug_assert!(!index.is_null());
        self.free.push(index.0);
        self.live -= 1;
    }

    #[inline]
    pub fn get(&self, index: SlotIndex) -> &Slot {
        &self.slots[index.0 as usize]
    }
    #[inline]
    pub fn get_mut(&mut self, index: SlotIndex) -> &mut Slot {
        &mut self.slots[index.0 as usize]
    }

    /// Total slot records ever allocated (live + free). The collector
    /// walks this range when sweeping.
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.slots.len() as u32
    }

    // --- mark-sweep support (see `crate::gc`) ---

    /// Clear every mark bit. Called at the start of a collection.
    pub fn clear_marks(&mut self) {
        for m in self.marks.iter_mut() {
            *m = false;
        }
    }

    /// Mark a slot reachable. Returns `true` if it was not already
    /// marked, so the tracer can avoid re-following an already-visited
    /// slot (cycle safety).
    #[inline]
    pub fn mark(&mut self, index: SlotIndex) -> bool {
        if index.is_null() {
            return false;
        }
        let i = index.0 as usize;
        if self.marks[i] {
            false
        } else {
            self.marks[i] = true;
            true
        }
    }

    #[inline]
    pub fn is_marked(&self, index: SlotIndex) -> bool {
        !index.is_null() && self.marks[index.0 as usize]
    }

    /// Whether `index` currently sits on the free list (a swept or
    /// never-live record). Linear in the free-list length; used only by
    /// the sweep bookkeeping and tests, not on any hot path.
    fn is_free(&self, i: u32) -> bool {
        self.free.contains(&i)
    }

    /// Sweep: every allocated slot that is not marked and not already
    /// free returns to the free list. Returns the number of slots
    /// reclaimed. Mirrors `fxSweep` reclaiming unmarked slots.
    pub fn sweep(&mut self) -> u32 {
        let mut reclaimed = 0u32;
        for i in 0..self.slots.len() as u32 {
            if !self.marks[i as usize] && !self.is_free(i) {
                self.free.push(i);
                self.live -= 1;
                reclaimed += 1;
            }
        }
        reclaimed
    }

    /// Live slot count. XS accounts 32 bytes per slot; this is the
    /// count `currentHeapCount` reports.
    #[inline]
    pub fn live_count(&self) -> u32 {
        self.live
    }

    /// Slot heap footprint in bytes, held at 32 per record (resolved
    /// question 5) so heap accounting stays comparable with C-XS.
    #[inline]
    pub fn byte_size(&self) -> usize {
        self.slots.len() * 32
    }
}

/// The size of a chunk's length header, in bytes. Each block in the
/// arena is laid out `[u32 length][payload...]`, mirroring XS's
/// `txChunk` header discipline (a size field precedes each chunk) so
/// the slide-compactor can walk and relocate blocks without external
/// bookkeeping.
const CHUNK_HEADER: usize = 4;

/// The chunk arena: variable-size data (strings in CESU-8, and later
/// ArrayBuffers, BigInt digits, bytecode). Each block carries a length
/// header so [`ChunkArena::compact`] can slide-compact during GC,
/// rewriting `ChunkOffset`s exactly where XS rewrites chunk pointers.
#[derive(Default)]
pub struct ChunkArena {
    bytes: Vec<u8>,
}

impl ChunkArena {
    pub fn new() -> ChunkArena {
        ChunkArena { bytes: Vec::new() }
    }

    /// Append bytes behind a length header, returning the offset of the
    /// payload (not the header). Strings are stored in CESU-8 exactly as
    /// XS holds them (resolved question 4).
    pub fn alloc(&mut self, data: &[u8]) -> ChunkOffset {
        let header = self.bytes.len();
        self.bytes
            .extend_from_slice(&(data.len() as u32).to_le_bytes());
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        debug_assert_eq!(off as usize, header + CHUNK_HEADER);
        ChunkOffset(off)
    }

    /// The stored length of the block whose payload begins at `off`.
    #[inline]
    pub fn len_of(&self, off: ChunkOffset) -> usize {
        let h = off.0 as usize - CHUNK_HEADER;
        u32::from_le_bytes([
            self.bytes[h],
            self.bytes[h + 1],
            self.bytes[h + 2],
            self.bytes[h + 3],
        ]) as usize
    }

    #[inline]
    pub fn slice(&self, off: ChunkOffset, len: usize) -> &[u8] {
        let start = off.0 as usize;
        &self.bytes[start..start + len]
    }

    /// The whole payload of the block at `off`, using its stored length.
    #[inline]
    pub fn payload(&self, off: ChunkOffset) -> &[u8] {
        self.slice(off, self.len_of(off))
    }

    /// Slide-compact: keep only the blocks whose payload offsets are in
    /// `live`, packing them to the front of the arena in ascending
    /// offset order, and return the old→new payload-offset remap the
    /// caller applies to every live `ChunkOffset` (design § Value and
    /// heap model: "offsets are rewritten exactly where XS rewrites
    /// pointers"). Duplicate/unknown offsets in `live` are ignored.
    pub fn compact(&mut self, live: &[ChunkOffset]) -> std::collections::HashMap<ChunkOffset, ChunkOffset> {
        use std::collections::{HashMap, HashSet};
        let mut seen: Vec<ChunkOffset> = live
            .iter()
            .copied()
            .filter(|o| !o.is_null())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // Relocate in ascending source order so the copy never overlaps
        // a not-yet-moved block.
        seen.sort_by_key(|o| o.0);

        let mut fresh: Vec<u8> = Vec::with_capacity(self.bytes.len());
        let mut remap: HashMap<ChunkOffset, ChunkOffset> = HashMap::new();
        for old in seen {
            let len = self.len_of(old);
            let start = old.0 as usize;
            let header = fresh.len();
            fresh.extend_from_slice(&(len as u32).to_le_bytes());
            let new_off = fresh.len() as u32;
            fresh.extend_from_slice(&self.bytes[start..start + len]);
            debug_assert_eq!(new_off as usize, header + CHUNK_HEADER);
            remap.insert(old, ChunkOffset(new_off));
        }
        self.bytes = fresh;
        remap
    }

    #[inline]
    pub fn byte_size(&self) -> usize {
        self.bytes.len()
    }
}

/// ToInt32 (ECMAScript 7.1.5 / XS `fxNumberToInteger`): fold a number
/// into the signed 32-bit range used by the bitwise opcodes.
#[inline]
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    // Truncate toward zero, then reduce modulo 2^32 into i32 range.
    let m = n.trunc();
    let m = m.rem_euclid(4294967296.0); // 2^32
    let u = m as u32; // exact: m in [0, 2^32)
    u as i32
}

/// The ECMAScript Number::toString(10) rendering used to compare a
/// completion value against the oracle's `String()` output. Handles
/// the cases XS's dtoa handles differently from Rust's default `{}`:
/// negative zero prints "0", non-finite values print JS spellings, and
/// integer-valued doubles print without a fractional part.
pub fn number_to_ecma_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    if n == 0.0 {
        // Covers +0 and -0; JS String(-0) === "0".
        return "0".to_string();
    }
    // Rust's shortest-round-trip formatter matches JS Number.toString
    // for the non-extreme magnitudes the stage-1 corpus uses; integer
    // valued doubles already render without a decimal point.
    format!("{}", n)
}
