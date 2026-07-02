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
/// free list. This is XS's slot heap; the mark-sweep collector that
/// sweeps it to the free list lands in stage 2 (design § Value and
/// heap model). Because it is index-based it is safe code: a stale
/// index is a kind-checked logic bug, not undefined behavior.
#[derive(Default)]
pub struct SlotArena {
    slots: Vec<Slot>,
    free: Vec<u32>,
    /// Count of live (non-free) slots, mirroring `currentHeapCount`.
    live: u32,
}

impl SlotArena {
    pub fn new() -> SlotArena {
        SlotArena {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }

    /// Allocate a slot, reusing the free list first (XS semantics).
    pub fn alloc(&mut self, slot: Slot) -> SlotIndex {
        self.live += 1;
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = slot;
            SlotIndex(i)
        } else {
            let i = self.slots.len() as u32;
            self.slots.push(slot);
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

/// The chunk arena: variable-size data (strings in CESU-8, and later
/// ArrayBuffers, BigInt digits, bytecode). Slide-compaction during GC
/// rewrites `ChunkOffset`s exactly where XS rewrites chunk pointers;
/// that compaction lands with the collector in stage 2.
#[derive(Default)]
pub struct ChunkArena {
    bytes: Vec<u8>,
}

impl ChunkArena {
    pub fn new() -> ChunkArena {
        ChunkArena { bytes: Vec::new() }
    }

    /// Append bytes, returning their offset. Strings are stored in
    /// CESU-8 exactly as XS holds them (resolved question 4).
    pub fn alloc(&mut self, data: &[u8]) -> ChunkOffset {
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        ChunkOffset(off)
    }

    #[inline]
    pub fn slice(&self, off: ChunkOffset, len: usize) -> &[u8] {
        let start = off.0 as usize;
        &self.bytes[start..start + len]
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
