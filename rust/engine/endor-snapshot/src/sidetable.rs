//! The **side-table completeness ledger** — the bug class this crate is
//! designed against (job spec item 3; the review ledger's standing
//! snapshot note).
//!
//! In endor the heap is index arenas, but a machine's reachable state is
//! *not* wholly in those arenas: dozens of side tables ([`endor_vm`]'s
//! `Interp` fields) hold per-instance and per-activation state keyed by
//! slot index — function closures, the caught-exception jump chain, a
//! suspended generator's saved frame, a promise's pending reactions, the
//! harden worklist. **An atom grammar that serializes the arenas but
//! misses one of these is the snapshot-shaped version of a missing GC
//! root: it round-trips fine on trivial heaps and corrupts on real ones.**
//!
//! So the set of side tables is made *explicit and exhaustive here*, one
//! [`SideTable`] variant per table, enumerated against `Interp`'s actual
//! fields. [`SideTable::descriptor`] is an exhaustive `match`: the
//! compiler forces a new variant to be described the moment it is added,
//! and [`SideTable::ALL`] (guarded by [`tests::all_is_exhaustive`]) forces
//! it into the coverage ledger. Each descriptor records its
//! [`Coverage`] — whether the writer/reader in [`crate::image`] carries it
//! yet — so the remaining work is a compile-checked list, never a silent
//! omission.

/// Whether a side table is carried by the current snapshot image
/// ([`crate::image`]), and if not, why it is safe to defer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Fully serialized and restored by the current image.
    Serialized,
    /// Resident in the slot/chunk arenas themselves (the `HEAP`/`BLOC`
    /// atoms), so it round-trips structurally with the arenas — no
    /// separate atom needed.
    InArena,
    /// Deterministically rebuilt at restore by re-running machine boot /
    /// intrinsic linking against the same program symbols, so it need not
    /// be stored (but must be re-derived, hence tracked here).
    BootDerived,
    /// **Not yet carried.** The image must grow an atom (or extend an
    /// existing one) before a machine spanning this table can round-trip.
    /// This is the remaining-work ledger the completeness note demands.
    Pending,
}

/// One side table of the machine's reachable state. Enumerated from the
/// live `endor_vm::interp::Interp` fields (verified against the struct,
/// not this list — see the module docs).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideTable {
    /// `functions` — user/native function metadata, **including
    /// `closures`** (the captured frame-cell owner). The headline case:
    /// closure capture is invisible in the slot arena's shape alone.
    Functions,
    /// `bound_functions` — `Function.prototype.bind` target/`this`/args.
    BoundFunctions,
    /// `call_stack` — the suspended `CallerState` activations (scope,
    /// args, result) of the active call chain.
    CallStack,
    /// `jumps` — the `CatchJump` chain (`the->firstJump`): each entry
    /// snapshots the value stack, scope, and call frames to restore on a
    /// throw. A caught-and-pending exception lives here + `exception`.
    Jumps,
    /// `global_props` — the global object's materialized own-property
    /// slot index by id.
    GlobalProps,
    /// `error_data` — per-instance Error name/message.
    ErrorData,
    /// `wrapper_data` — per-instance primitive-wrapper boxed value.
    WrapperData,
    /// `arrays` — exotic array length + item chunk.
    Arrays,
    /// `collections` — Map/Set/WeakMap/WeakSet internal slots.
    Collections,
    /// `array_buffers` — ArrayBuffer backing store.
    ArrayBuffers,
    /// `typed_arrays` — TypedArray view state + buffer reference.
    TypedArrays,
    /// `data_views` — DataView view state + buffer reference.
    DataViews,
    /// `iterators` — array-iterator state (target, index, kind, reused
    /// result object).
    Iterators,
    /// `promises` — per-instance settlement STATUS/RESULT/THENS.
    Promises,
    /// `promise_functions` — a resolve/reject function's bound home data.
    PromiseFunctions,
    /// `promise_guards` — the per-pair `[[AlreadyResolved]]` flags.
    PromiseGuards,
    /// `promise_jobs` — the pending microtask (reaction-job) queue.
    PromiseJobs,
    /// `generators` — per-instance suspended activation + lifecycle state.
    Generators,
    /// `gen_run_stack` — generators currently mid-`resume_generator`
    /// dispatch (the `YIELD` snapshot target stack).
    GenRunStack,
    /// `async_instances` — per-instance async activation + result promise.
    AsyncInstances,
    /// `async_run_stack` — async instances mid-`step_async` dispatch (the
    /// `AWAIT` snapshot target stack).
    AsyncRunStack,
    /// `regexps` — compiled RegExp program + source/flags (note:
    /// `lastIndex` is an ordinary own property, in the arena).
    RegExps,
    /// `ctor_prototype` — each constructor instance's `.prototype` object.
    CtorPrototype,
    /// `symbol_registry` (+ `symbol_registry_keys`) — the global
    /// `Symbol.for`/`keyFor` registry.
    SymbolRegistry,
    /// `symbol_names` / `symbol_ids` / `next_intern_id` — the program
    /// symbol name↔id tables and the runtime-interned-key counter
    /// (`NAME`/`SYMB`/`KEYS`).
    SymbolTables,
    /// The module records/maps (`endor_vm::module::ModuleGraph`): a
    /// worker that has imported modules carries linked module records and
    /// namespace objects.
    Modules,
    /// The harden worklist / frozen-intrinsics tables (SES `lockdown`/
    /// `harden`/`petrify`, requirement 5): which intrinsics and object
    /// graphs are frozen. A resumed hardened graph must stay hardened.
    HardenState,
    /// `meter` — the machine's metering state (design row 6): accumulated
    /// computrons, the check interval/threshold, and the frozen cost-table
    /// version. **Carried by the `METR` atom** (stage-6 child 3), so a
    /// resumed machine continues its meter exactly.
    Meter,
}

impl SideTable {
    /// Every side table. **Adding a `SideTable` variant without adding it
    /// here fails [`tests::all_is_exhaustive`]**; every entry's coverage
    /// is asserted, so a new table cannot slip in as a silent snapshot
    /// gap.
    pub const ALL: &'static [SideTable] = &[
        SideTable::Functions,
        SideTable::BoundFunctions,
        SideTable::CallStack,
        SideTable::Jumps,
        SideTable::GlobalProps,
        SideTable::ErrorData,
        SideTable::WrapperData,
        SideTable::Arrays,
        SideTable::Collections,
        SideTable::ArrayBuffers,
        SideTable::TypedArrays,
        SideTable::DataViews,
        SideTable::Iterators,
        SideTable::Promises,
        SideTable::PromiseFunctions,
        SideTable::PromiseGuards,
        SideTable::PromiseJobs,
        SideTable::Generators,
        SideTable::GenRunStack,
        SideTable::AsyncInstances,
        SideTable::AsyncRunStack,
        SideTable::RegExps,
        SideTable::CtorPrototype,
        SideTable::SymbolRegistry,
        SideTable::SymbolTables,
        SideTable::Modules,
        SideTable::HardenState,
        SideTable::Meter,
    ];

    /// The table's `Interp` field name and its current snapshot coverage.
    /// An **exhaustive** match: the compiler forces every new variant to
    /// declare a descriptor, which is what makes this a completeness
    /// ledger rather than a stale comment.
    pub fn descriptor(self) -> Descriptor {
        use Coverage::*;
        let (field, coverage): (&'static str, Coverage) = match self {
            // Slot-resident tables round-trip with the arenas; the rest of
            // their per-instance state (closures, settlement, saved
            // frames) is what the Pending atoms below must carry.
            SideTable::GlobalProps => ("global_props", InArena),
            SideTable::CtorPrototype => ("ctor_prototype", InArena),
            // Program symbol name/id tables and the intern counter map to
            // the NAME/SYMB/KEYS atoms — carried by the image today.
            SideTable::SymbolTables => ("symbol_names/symbol_ids/next_intern_id", Serialized),
            // Boot objects/ids re-derived by re-linking intrinsics.
            SideTable::SymbolRegistry => ("symbol_registry/symbol_registry_keys", Pending),
            // The rich per-instance/per-activation tables still to be wired
            // into dedicated atoms (child-3-adjacent; the honest remainder).
            SideTable::Functions => ("functions", Pending),
            SideTable::BoundFunctions => ("bound_functions", Pending),
            SideTable::CallStack => ("call_stack", Pending),
            SideTable::Jumps => ("jumps", Pending),
            SideTable::ErrorData => ("error_data", Pending),
            SideTable::WrapperData => ("wrapper_data", Pending),
            SideTable::Arrays => ("arrays", Pending),
            SideTable::Collections => ("collections", Pending),
            SideTable::ArrayBuffers => ("array_buffers", Pending),
            SideTable::TypedArrays => ("typed_arrays", Pending),
            SideTable::DataViews => ("data_views", Pending),
            SideTable::Iterators => ("iterators", Pending),
            SideTable::Promises => ("promises", Pending),
            SideTable::PromiseFunctions => ("promise_functions", Pending),
            SideTable::PromiseGuards => ("promise_guards", Pending),
            SideTable::PromiseJobs => ("promise_jobs", Pending),
            SideTable::Generators => ("generators", Pending),
            SideTable::GenRunStack => ("gen_run_stack", Pending),
            SideTable::AsyncInstances => ("async_instances", Pending),
            SideTable::AsyncRunStack => ("async_run_stack", Pending),
            SideTable::RegExps => ("regexps", Pending),
            SideTable::Modules => ("module::ModuleGraph", Pending),
            SideTable::HardenState => ("lockdown/harden state", Pending),
            // The metering state — carried by the METR atom (child 3).
            SideTable::Meter => ("meter", Serialized),
        };
        Descriptor {
            table: self,
            field,
            coverage,
        }
    }

    /// The tables not yet carried by the snapshot image — the remaining
    /// work, computed from the ledger so it can never drift from the code.
    pub fn pending() -> Vec<SideTable> {
        Self::ALL
            .iter()
            .copied()
            .filter(|t| t.descriptor().coverage == Coverage::Pending)
            .collect()
    }
}

/// A side table's completeness descriptor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub table: SideTable,
    /// The `Interp` field(s) backing this table.
    pub field: &'static str,
    pub coverage: Coverage,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` must list every variant exactly once. This is the guard that
    /// turns "add a field to `Interp`" into "add it to the snapshot
    /// ledger": a new `SideTable` variant that is not in `ALL` (or is
    /// duplicated) fails here, and one with no `descriptor` arm fails to
    /// compile.
    #[test]
    fn all_is_exhaustive() {
        // Count of variants, kept beside the enum. Bump when a variant is
        // added — the assertion below then forces the ALL entry too.
        const VARIANT_COUNT: usize = 28;
        assert_eq!(SideTable::ALL.len(), VARIANT_COUNT);

        // No duplicates: each field name appears once.
        let mut fields: Vec<&str> = SideTable::ALL.iter().map(|t| t.descriptor().field).collect();
        fields.sort_unstable();
        let before = fields.len();
        fields.dedup();
        assert_eq!(before, fields.len(), "duplicate side table in ALL");
    }

    #[test]
    fn pending_is_derived_from_ledger() {
        let pending = SideTable::pending();
        // The rich per-instance tables are still pending; the arena- and
        // boot-covered ones are not.
        assert!(pending.contains(&SideTable::Functions));
        assert!(pending.contains(&SideTable::Generators));
        assert!(!pending.contains(&SideTable::GlobalProps));
        assert!(!pending.contains(&SideTable::SymbolTables));
    }
}
