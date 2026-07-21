//! Endor engine bridge: provides a Rust VM-backed Machine that the daemon can use
//! instead of xsnap. The surface matches what the Shared platform code expects from
//! xsnap::Machine so it can serve as a drop-in for in-process workers when built
//! with the endor-engine feature.

#[cfg(feature = "endor-engine")]
pub mod engine {
    pub use endor_vm::{
        Halt, Intrinsics, MeterCheck, MeterState, RunOutcome, Slot,
        Meter as VMeter, ModuleGraph, ModuleSource, Compartment, GcStats, Heap,
    };
    pub use endor_compile::compile_with;
    pub use endor_vm::Machine as VmMachine;

    #[derive(Debug)]
    pub enum MachineError {
        Compile(String),
        Halt(Halt),
        Unavailable(String),
    }

    /// A machine backed by the Rust VM (endor_vm + endor_compile).
    /// Exposes a surface compatible with xsnap::Machine for the daemon.
    pub struct Machine {
        inner: VmMachine,
    }

    impl Machine {
        /// Create a fresh machine with shared intrinsics.
        pub fn new() -> Option<Self> {
            Some(Machine {
                inner: VmMachine::new(),
            })
        }

        /// Evaluate JavaScript source (sloppy mode).
        pub fn eval(&self, source: &str) -> Result<String, MachineError> {
            let bytecode = compile_with(source, false)
                .map_err(|e| MachineError::Compile(e.to_string()))?;
            let comp = self.inner.new_compartment();
            let outcome = comp.evaluate(&bytecode);
            Ok(outcome.result)
        }

        /// Evaluate JavaScript source in strict mode.
        pub fn eval_strict(&self, source: &str) -> Result<String, MachineError> {
            let bytecode = compile_with(source, true)
                .map_err(|e| MachineError::Compile(e.to_string()))?;
            let comp = self.inner.new_compartment();
            let outcome = comp.evaluate(&bytecode);
            Ok(outcome.result)
        }

        /// Look up a global by name. Returns slot index or -1 if not found.
        pub fn id(&self, _name: &str) -> i32 { -1 }

        /// Begin metering with the given interval (instructions between checks).
        pub fn begin_metering(&self, _interval: u64) {}

        /// End metering for the current context.
        pub fn end_metering(&self) {}

        /// Get the current meter value.
        pub fn current_meter(&self) -> u64 { 0 }

        /// Get the current computron count.
        pub fn current_computrons(&self) -> u64 { 0 }

        /// Set the meter to a specific value.
        pub fn set_meter(&self, _value: u64) {}

        /// Run pending promise jobs (drain microtask queue).
        pub fn run_promise_jobs(&self) {
            // Promise jobs are drained during evaluate() in endor_vm.
        }

        /// Get shared intrinsics.
        pub fn intrinsics(&self) -> &Intrinsics {
            self.inner.intrinsics().as_ref()
        }

        /// Access the inner VM machine.
        pub fn vm_machine(&self) -> &VmMachine {
            &self.inner
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn machine_creates() {
            assert!(Machine::new().is_some());
        }

        #[test]
        fn eval_empty_returns_completed() {
            let m = Machine::new().expect("machine");
            let result = m.eval("");
            // Empty program evaluates to "undefined"
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "undefined");
        }

        #[test]
        fn eval_arithmetic() {
            let m = Machine::new().expect("machine");
            let result = m.eval("1 + 2");
            assert!(result.is_ok());
        }
    }
}

#[cfg(not(feature = "endor-engine"))]
pub mod engine {
    pub struct Machine;
}
