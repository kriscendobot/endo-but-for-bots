//! A primordial `Compartment.evaluate` seam (design § Hardened
//! JavaScript and Compartment; requirement 5).
//!
//! XS implements SES natively: intrinsics are created once per machine
//! and referenced per realm, and every evaluator is reachable for
//! per-compartment replacement. Stage 1 carves exactly those seams,
//! early, so the `lockdown`/`harden`/`Compartment` port in stage 4
//! slots in without re-architecture: shared intrinsics behind an `Rc`,
//! fresh per-compartment globals, and an `evaluate` that runs bytecode
//! against them. No modules yet (that is stage 4).

use std::collections::HashMap;
use std::rc::Rc;

use crate::interp::{Interp, RunOutcome};
use crate::value::Slot;

/// The shared intrinsics seam: primordials created once per machine and
/// shared, frozen, across every compartment. Stage 1 holds the seam
/// (the shape the transitive-freeze worklist and per-realm evaluator
/// replacement need); the actual frozen primordial graph fills in with
/// the object model and `lockdown` in later stages.
#[derive(Default)]
pub struct Intrinsics {
    /// Whether `lockdown` has frozen the shared intrinsics. Once true,
    /// per-compartment evaluators are the only mutable evaluator seam.
    pub locked_down: bool,
}

impl Intrinsics {
    pub fn new() -> Rc<Intrinsics> {
        Rc::new(Intrinsics::default())
    }
}

/// A compartment: a fresh `globalThis` over shared frozen intrinsics.
/// `evaluate` compiles-then-runs; stage 1 takes the compiled bytecode
/// from the oracle (the Rust compiler lands in stage 5), which is why
/// `evaluate` accepts a bytecode buffer rather than source.
pub struct Compartment {
    intrinsics: Rc<Intrinsics>,
    /// This compartment's own global bindings by display name, distinct
    /// from every other compartment's and from the intrinsics.
    globals: HashMap<String, Slot>,
    /// The same bindings keyed by the interned symbol id the bytecode
    /// references them through (`GET_VARIABLE`/`SET_VARIABLE` operands).
    /// Until the compiler and symbol table land (stage 5) the harness
    /// supplies these ids alongside the names.
    globals_by_id: HashMap<u16, Slot>,
}

impl Compartment {
    /// Create a compartment sharing `intrinsics` with its siblings but
    /// owning fresh globals.
    pub fn new(intrinsics: Rc<Intrinsics>) -> Compartment {
        Compartment {
            intrinsics,
            globals: HashMap::new(),
            globals_by_id: HashMap::new(),
        }
    }

    /// Bind a name in this compartment's global scope only.
    pub fn define_global(&mut self, name: &str, value: Slot) {
        self.globals.insert(name.to_string(), value);
    }

    /// Bind a global by the interned symbol id the bytecode addresses it
    /// through, so [`Compartment::evaluate`] can seed a program that
    /// reads that global. (`define_global` is the name-keyed seam that
    /// resolves ids once the symbol table lands.)
    pub fn define_global_id(&mut self, id: u16, value: Slot) {
        self.globals_by_id.insert(id, value);
    }

    /// Read a global binding (this compartment's, not a sibling's).
    pub fn global(&self, name: &str) -> Option<&Slot> {
        self.globals.get(name)
    }

    /// The shared intrinsics this compartment evaluates over.
    pub fn intrinsics(&self) -> &Rc<Intrinsics> {
        &self.intrinsics
    }

    /// Evaluate a program bytecode buffer in this compartment. The
    /// interpreter is seeded with **this compartment's** own globals
    /// (finding 5): a program that reads a global observes this
    /// compartment's binding, so two compartments over the same shared
    /// intrinsics diverge exactly and only in their own globals — the
    /// requirement-5 seam, now load-bearing rather than a stub.
    pub fn evaluate(&self, bytecode: &[u8]) -> RunOutcome {
        let mut interp = Interp::new();
        for (&id, &value) in &self.globals_by_id {
            interp.define_global_id(id, value);
        }
        interp.run(bytecode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Opcode;
    use crate::value::Slot;

    /// Program bytecode reading the global symbol `id` and returning it:
    /// `EVAL_REFERENCE id; GET_VARIABLE id; SET_RESULT; END`.
    fn read_global_program(id: u16) -> Vec<u8> {
        let [lo, hi] = id.to_le_bytes();
        vec![
            Opcode::XS_CODE_EVAL_REFERENCE as u8, lo, hi,
            Opcode::XS_CODE_GET_VARIABLE as u8, lo, hi,
            Opcode::XS_CODE_SET_RESULT as u8,
            Opcode::XS_CODE_END as u8,
        ]
    }

    #[test]
    fn compartments_diverge_only_in_their_own_globals() {
        let m = Machine::new();
        let program = read_global_program(7);

        let mut a = m.new_compartment();
        let mut b = m.new_compartment();
        a.define_global_id(7, Slot::integer(1));
        b.define_global_id(7, Slot::integer(2));

        let ra = a.evaluate(&program);
        let rb = b.evaluate(&program);

        assert!(ra.completed && rb.completed, "both read their own binding");
        assert_eq!(ra.result, "1", "compartment A sees its own global");
        assert_eq!(rb.result, "2", "compartment B sees its own global");
        // Shared intrinsics, divergent globals: the requirement-5 seam.
        assert_ne!(ra.result, rb.result);
    }

    #[test]
    fn unbound_global_read_throws_not_reads_a_sibling() {
        let m = Machine::new();
        let program = read_global_program(9);
        let a = m.new_compartment();
        // No binding for id 9 in this compartment: the read is a
        // ReferenceError, never a leak from a sibling compartment.
        let r = a.evaluate(&program);
        assert!(!r.completed, "an unbound global read does not complete");
    }
}

/// A machine hosts one shared intrinsics graph and any number of
/// compartments over it (design: intrinsics once per machine,
/// referenced per realm).
pub struct Machine {
    intrinsics: Rc<Intrinsics>,
}

impl Default for Machine {
    fn default() -> Self {
        Machine::new()
    }
}

impl Machine {
    pub fn new() -> Machine {
        Machine {
            intrinsics: Intrinsics::new(),
        }
    }

    /// A fresh compartment over this machine's shared intrinsics.
    pub fn new_compartment(&self) -> Compartment {
        Compartment::new(Rc::clone(&self.intrinsics))
    }
}
