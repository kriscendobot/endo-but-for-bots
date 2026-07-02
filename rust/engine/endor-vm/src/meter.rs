//! The computation meter (design § Metering; requirement 1a).
//!
//! A `u64` in 16.16 fixed point, incremented at exactly XS's points
//! with exactly XS's weights, and checked only at loop-closing points.
//! The host callback sees `meterIndex >> 16` ("computrons").
//!
//! Ground truth is `xs/sources/xsRun.c` and `xsCommon.h` at the
//! `c/moddable` pin: under `mxMetering` the dispatch loop adds
//! `XS_CODE_METERING = 1 << 16` per bytecode; built-in steps add
//! `XS_BUILTIN_METERING = 1 << 14`; the parser adds
//! `XS_PARSE_CODE_METERING = 1 << 16` per unit (parse metering is out
//! of the interpreter-parity window, so it is not modeled here). The
//! check (`fxCheckMetering`) fires only when `meterInterval` is set and
//! `meterIndex > meterCount`, at backward branches, calls, returns, and
//! catches; a false return aborts the crank.

/// `XS_CODE_METERING`: one bytecode dispatch.
pub const CODE_METERING: u64 = 1 << 16;
/// `XS_BUILTIN_METERING`: one built-in operation step (used once the
/// built-ins land in stage 3; the `mxMeterSome(k)` fast-path
/// annotations become port obligations there).
pub const BUILTIN_METERING: u64 = 1 << 14;

/// Outcome of a metering check at a loop-closing point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeterCheck {
    /// Keep running.
    Continue,
    /// The host refused more computation: abort the crank with
    /// `XS_TOO_MUCH_COMPUTATION_EXIT` semantics.
    Abort,
}

/// The 16.16 fixed-point meter.
#[derive(Debug, Clone)]
pub struct Meter {
    /// `the->meterIndex`.
    index: u64,
    /// `the->meterInterval`. Zero disables checks (but the index still
    /// accumulates, exactly as under `mxMetering`).
    interval: u64,
    /// `the->meterCount`: the next threshold a check compares against.
    count: u64,
    /// The last computron value the host callback was shown, for
    /// reporting.
    last_reported: u64,
}

impl Default for Meter {
    fn default() -> Self {
        Meter::new()
    }
}

impl Meter {
    pub fn new() -> Meter {
        Meter {
            index: 0,
            interval: 0,
            count: 0,
            last_reported: 0,
        }
    }

    /// `fxBeginMetering`: install an interval and arm the first check.
    pub fn begin(&mut self, interval: u64) {
        self.interval = interval;
        self.count = interval;
    }

    /// Reset the raw index to zero (the oracle shim does this after
    /// parse so the run-only count is comparable).
    pub fn reset(&mut self) {
        self.index = 0;
        self.count = self.interval;
    }

    /// Add one bytecode dispatch (`the->meterIndex += XS_CODE_METERING`
    /// in `mxBreak`).
    #[inline]
    pub fn tick_code(&mut self) {
        self.index += CODE_METERING;
    }

    /// Add `n` bytecode-equivalent units in one step (the explicit
    /// `meterIndex += k * XS_CODE_METERING` sites, e.g. the computed
    /// element-access path).
    #[inline]
    pub fn tick_code_n(&mut self, n: u64) {
        self.index += n * CODE_METERING;
    }

    /// Add one built-in step (`mxMeterOne`).
    #[inline]
    pub fn tick_builtin(&mut self) {
        self.index += BUILTIN_METERING;
    }

    /// Add `k` built-in steps (`mxMeterSome(k)`).
    #[inline]
    pub fn tick_builtin_some(&mut self, k: u64) {
        self.index += k * BUILTIN_METERING;
    }

    /// Raw fixed-point index (`the->meterIndex`).
    #[inline]
    pub fn raw(&self) -> u64 {
        self.index
    }

    /// Computrons (`meterIndex >> 16`), what the host callback sees.
    #[inline]
    pub fn computrons(&self) -> u64 {
        self.index >> 16
    }

    /// `mxCheckMeter`: at a loop-closing point, if metering is armed and
    /// the index passed the threshold, consult `host`. Mirrors
    /// `fxCheckMetering`: on continue, advance `meterCount` by the
    /// interval; on refusal, signal an abort.
    #[inline]
    pub fn check<F: FnMut(u64) -> bool>(&mut self, host: &mut F) -> MeterCheck {
        if self.interval != 0 && self.index > self.count {
            self.last_reported = self.computrons();
            if host(self.last_reported) {
                self.count = self.index + self.interval;
                MeterCheck::Continue
            } else {
                MeterCheck::Abort
            }
        } else {
            MeterCheck::Continue
        }
    }
}
