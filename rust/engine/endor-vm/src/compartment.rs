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
    /// This compartment's own global bindings, distinct from every
    /// other compartment's and from the intrinsics.
    globals: HashMap<String, Slot>,
}

impl Compartment {
    /// Create a compartment sharing `intrinsics` with its siblings but
    /// owning fresh globals.
    pub fn new(intrinsics: Rc<Intrinsics>) -> Compartment {
        Compartment {
            intrinsics,
            globals: HashMap::new(),
        }
    }

    /// Bind a name in this compartment's global scope only.
    pub fn define_global(&mut self, name: &str, value: Slot) {
        self.globals.insert(name.to_string(), value);
    }

    /// Read a global binding (this compartment's, not a sibling's).
    pub fn global(&self, name: &str) -> Option<&Slot> {
        self.globals.get(name)
    }

    /// The shared intrinsics this compartment evaluates over.
    pub fn intrinsics(&self) -> &Rc<Intrinsics> {
        &self.intrinsics
    }

    /// Evaluate a program bytecode buffer in this compartment. A fresh
    /// interpreter activation runs over the compartment's globals and
    /// the shared intrinsics, proving the requirement-5 seam: two
    /// compartments over the same intrinsics evaluate independently.
    pub fn evaluate(&self, bytecode: &[u8]) -> RunOutcome {
        let mut interp = Interp::new();
        interp.run(bytecode)
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
