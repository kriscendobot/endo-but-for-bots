#![forbid(unsafe_code)]
//! endor-fuzz: the differential and decoder fuzz logic (design
//! § Fuzzability). The two stage-1 cargo-fuzz targets are thin
//! libFuzzer wrappers (`fuzz/fuzz_targets/`) over the functions here;
//! keeping the substance in a plain, `forbid(unsafe_code)` lib means it
//! builds and is unit-tested without a libFuzzer toolchain, and the
//! same generator/comparator seeds the differential corpus.
//!
//! - **Target 1, differential source fuzzing** (the flagship): a
//!   structure-aware generator produces a subset-grammar program from
//!   raw fuzzer bytes; `differential_check` feeds identical source to
//!   endor and the C-XS oracle and compares completion kind, result
//!   string, and computron count. Any divergence is a finding.
//! - **Target 2, bytecode decoder fuzzing**: `decoder_is_panic_free`
//!   drives arbitrary/truncated bytes through the decoder and
//!   interpreter, which must degrade to a `Halt::Decode`, never panic
//!   (XS treats bytecode as trusted; endor's loader must not).

use endor_vm::{disassemble, run_program};

/// A cursor over fuzzer-provided bytes, used to drive the grammar
/// deterministically (a minimal `arbitrary::Unstructured`).
struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bytes { data, pos: 0 }
    }
    fn next(&mut self) -> u8 {
        if self.data.is_empty() {
            return 0;
        }
        let b = self.data[self.pos % self.data.len()];
        self.pos = self.pos.wrapping_add(1);
        b
    }
    fn choice(&mut self, n: u8) -> u8 {
        self.next() % n
    }
}

/// Structure-aware generator: fold raw bytes into a program in the
/// stage-1 subset grammar (integer/number/boolean literals combined
/// with the implemented arithmetic, bitwise, comparison, logic, unary,
/// and conditional operators). `depth` bounds recursion so generation
/// terminates.
pub fn gen_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    gen_expr(&mut b, 4)
}

fn gen_expr(b: &mut Bytes, depth: u8) -> String {
    if depth == 0 {
        return gen_atom(b);
    }
    match b.choice(9) {
        0 => {
            let op = ["+", "-", "*", "/", "%"][b.choice(5) as usize];
            format!("({} {} {})", gen_expr(b, depth - 1), op, gen_expr(b, depth - 1))
        }
        1 => {
            let op = ["&", "|", "^", "<<", ">>", ">>>"][b.choice(6) as usize];
            format!("({} {} {})", gen_expr(b, depth - 1), op, gen_expr(b, depth - 1))
        }
        2 => {
            let op = ["<", "<=", ">", ">=", "===", "!==", "==", "!="][b.choice(8) as usize];
            format!("({} {} {})", gen_expr(b, depth - 1), op, gen_expr(b, depth - 1))
        }
        3 => {
            let op = ["&&", "||"][b.choice(2) as usize];
            format!("({} {} {})", gen_expr(b, depth - 1), op, gen_expr(b, depth - 1))
        }
        4 => format!("(-{})", gen_expr(b, depth - 1)),
        5 => format!("(!{})", gen_expr(b, depth - 1)),
        6 => format!("(~{})", gen_expr(b, depth - 1)),
        7 => format!(
            "({} ? {} : {})",
            gen_expr(b, depth - 1),
            gen_expr(b, depth - 1),
            gen_expr(b, depth - 1)
        ),
        _ => gen_atom(b),
    }
}

/// Structure-aware generator for the **stage-2 surface**: a program
/// with `var` bindings and a backward-branch loop that mutates them,
/// returning one of the bindings. Every generated program is valid and
/// terminating (the loop bound is a small literal and the counter only
/// increments), exercising the frame/scope/variable/loop opcodes the
/// differential harness compares on results. Computrons are not yet
/// bit-exact for this surface (run-time allocation metering awaits the
/// faithful heap), so [`differential_check_result_only`] drives it.
pub fn gen_statement_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    let seed0 = (b.next() % 20) as i32 - 5;
    let seed1 = (b.next() % 20) as i32 - 5;
    let bound = 1 + (b.next() % 6) as i32; // 1..=6 iterations
    let body_op = ["+", "-", "*"][b.choice(3) as usize];
    let step = 1 + (b.next() % 3) as i32; // keep the counter moving
    let ret_both = b.choice(2) == 0;
    // v0 accumulates over the loop; v1 is a second live binding.
    let mut s = String::new();
    s.push_str(&format!("var v0 = {}; var v1 = {}; ", seed0, seed1));
    s.push_str(&format!(
        "for (var i = 0; i < {}; i = i + {}) {{ v0 = v0 {} i }} ",
        bound, step, body_op
    ));
    if ret_both {
        s.push_str("v0 + v1");
    } else {
        s.push_str("v0");
    }
    s
}

/// Structure-aware generator for the **stage-2b surface**: valid,
/// terminating programs that exercise the object model, user-function
/// calls, closures, and thrown-and-caught exceptions — the machinery this
/// stage made **bit-exact** (result AND computron), so the generated
/// programs are driven by the full [`differential_check`], not the
/// result-only variant. Every branch stays inside the small-integer domain
/// (values bounded, only `+`/`-`/`*`, no division) so results and their
/// `String()` renderings are unambiguous and the loop/closure/recursion
/// counts are bounded.
pub fn gen_stage2b_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    match b.choice(4) {
        0 => gen_object_program(&mut b),
        1 => gen_call_program(&mut b),
        2 => gen_closure_program(&mut b),
        _ => gen_exception_program(&mut b),
    }
}

/// A small non-negative integer literal (0..=9), keeping generated values
/// bounded so repeated `+`/`-`/`*` never overflows the integer fast path
/// or produces a magnitude whose `String()` differs between the engines
/// (they agree regardless, but small values keep the corpus legible).
fn small_int(b: &mut Bytes) -> i32 {
    (b.next() % 10) as i32
}

/// One of the overflow-safe, throw-free binary operators.
fn small_op(b: &mut Bytes) -> &'static str {
    ["+", "-", "*"][b.choice(3) as usize]
}

/// Object model: an object literal or a `{}`-plus-dynamic-property build,
/// then two own-property reads combined. Exercises `OBJECT`,
/// `NEW_PROPERTY`/`SET_PROPERTY`, and `GET_PROPERTY`.
fn gen_object_program(b: &mut Bytes) -> String {
    let x = small_int(b);
    let y = small_int(b);
    let op = small_op(b);
    if b.choice(2) == 0 {
        format!("var o = {{a: {}, b: {}}}; o.a {} o.b", x, y, op)
    } else {
        format!("var o = {{}}; o.x = {}; o.y = {}; o.x {} o.y", x, y, op)
    }
}

/// User functions: an IIFE or a function stored in a var, called with one
/// or two arguments. Exercises `constructor_function`/`function`/`code`/
/// `call`/`run`/`argument`/`end` frame switching.
fn gen_call_program(b: &mut Bytes) -> String {
    let x = small_int(b);
    let y = small_int(b);
    let op = small_op(b);
    match b.choice(3) {
        0 => format!("(function(p){{return p {} {}}})({})", op, y, x),
        1 => format!("(function(p,q){{return p {} q}})({}, {})", op, x, y),
        _ => format!("var f = function(p){{return p {} {}}}; f({})", op, y, x),
    }
}

/// Closures: a counter factory whose returned inner function mutates a
/// captured cell across a bounded number of calls, or a curried two-stage
/// adder. Exercises `new_closure`/`store`/`retrieve`/`get_closure`/
/// `pull_closure` and the shared-cell model.
fn gen_closure_program(b: &mut Bytes) -> String {
    let seed = small_int(b);
    let step = small_int(b);
    if b.choice(2) == 0 {
        let calls = 1 + (b.next() % 3) as usize; // 1..=3 calls
        let mut s = format!(
            "var mk = function(){{var c = {}; return function(){{c = c + {}; return c}}}}; var f = mk();",
            seed, step
        );
        for i in 0..calls {
            s.push_str(if i + 1 < calls { " f();" } else { " f()" });
        }
        s
    } else {
        let x = small_int(b);
        let y = small_int(b);
        format!("var add = function(a){{return function(c){{return a + c}}}}; add({})({})", x, y)
    }
}

/// Thrown-and-caught exceptions: a caught throw whose value is used, a try
/// with no throw, or a try/catch/finally that observes both paths.
/// Exercises `catch`/`throw`/`exception`/`uncatch` and the finally
/// status-temporary skeleton — all caught (so `BothComplete`, bit-exact).
fn gen_exception_program(b: &mut Bytes) -> String {
    let n = small_int(b);
    let m = small_int(b);
    let op = small_op(b);
    match b.choice(3) {
        0 => format!("try {{ throw {} }} catch(e) {{ e {} {} }}", n, op, m),
        1 => format!("try {{ {} {} {} }} catch(e) {{ {} }}", n, op, m, m),
        _ => format!(
            "var r = 0; try {{ throw {} }} catch(e) {{ r = e }} finally {{ r = r + {} }} r",
            n, m
        ),
    }
}

/// Structure-aware generator for the **stage-3 arrays surface**: the array
/// exotic object's grammar that is **bit-exact** (result AND computron) —
/// array literals (with holes), computed index get/set over the item chunk,
/// and the `length` accessor get/set. Deliberately excludes the honest-skip
/// cases (integer-indexed *ordinary* objects, runtime-minted string keys,
/// the iteration protocol, and `Array.prototype` methods), so every program
/// it emits rides the full [`differential_check`]. Values stay in the small
/// non-negative integer domain so element `String()` renderings are
/// unambiguous, and indices stay small so the item chunk grows a bounded
/// amount.
pub fn gen_stage3_arrays_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    // A small array literal (1..=4 elements) of small ints, optionally with
    // one hole, rendered as its source text.
    let n = 1 + (b.next() % 4) as usize; // 1..=4 elements
    let hole_at = if b.choice(2) == 0 { Some((b.next() % n as u8) as usize) } else { None };
    let mut elems: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        if Some(i) == hole_at {
            elems.push(String::new()); // an elision → a hole
        } else {
            elems.push(small_int(&mut b).to_string());
        }
    }
    let lit = format!("[{}]", elems.join(","));
    match b.choice(6) {
        // The literal itself (Array.prototype.toString → join(",")).
        0 => lit,
        // An indexed read (in range or one past the end → undefined).
        1 => {
            let i = (b.next() as usize) % (n + 1);
            format!("var a={}; a[{}]", lit, i)
        }
        // An indexed overwrite, then read it back.
        2 => {
            let i = (b.next() as usize) % n;
            let v = small_int(&mut b);
            format!("var a={}; a[{}]={}; a[{}]", lit, i, v, i)
        }
        // A grow-past-the-end write, then observe the new length.
        3 => {
            let k = n + 1 + (b.next() % 3) as usize;
            let v = small_int(&mut b);
            format!("var a={}; a[{}]={}; a.length", lit, k, v)
        }
        // A `length` read.
        4 => format!("var a={}; a.length", lit),
        // A `length` store (grow with holes, or shrink dropping items), then
        // the resulting array joined.
        _ => {
            let m = (b.next() % (n as u8 + 3)) as usize;
            format!("var a={}; a.length={}; a", lit, m)
        }
    }
}

/// Structure-aware generator for the **dense `Array.prototype` mutation
/// methods** (`push`/`pop`/`indexOf`) — the fast paths that are bit-exact
/// (result AND computron). It always builds a **dense** literal (no holes, so
/// `fxCheckArray`'s fast path applies), then applies a method and observes its
/// return value, the resulting array, or the length. Excludes `join` (its
/// per-element `ToString` metering is a later increment) and sparse receivers
/// (which take XS's slow path). Rides the full symbol-linking differential
/// check.
pub fn gen_stage3_array_methods_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    let n = (b.next() % 5) as usize; // 0..=4 dense elements
    let elems: Vec<String> = (0..n).map(|_| small_int(&mut b).to_string()).collect();
    let lit = format!("[{}]", elems.join(","));
    match b.choice(6) {
        // push one, observe the new length (its return value).
        0 => format!("var a={}; a.push({})", lit, small_int(&mut b)),
        // push one, observe the resulting array.
        1 => format!("var a={}; a.push({}); a", lit, small_int(&mut b)),
        // push several, observe the length.
        2 => format!(
            "var a={}; a.push({},{}); a.length",
            lit,
            small_int(&mut b),
            small_int(&mut b)
        ),
        // pop, observe the removed element.
        3 => format!("var a={}; a.pop()", lit),
        // pop, observe the resulting array and length.
        4 => format!("var a={}; a.pop(); a.length", lit),
        // indexOf a value that may or may not be present.
        _ => format!("var a={}; a.indexOf({})", lit, small_int(&mut b)),
    }
}

/// Structure-aware generator for the **array iterator objects**
/// (`values`/`keys`/`entries` + `next` over the reused result object) — the
/// bit-exact (result AND computron) explicit-iterator grammar. Builds a dense
/// literal, opens an iterator of one of the three kinds, advances it a bounded
/// number of `next()` calls (possibly past the end to reach `done`), and reads
/// `.value` or `.done` off the final result. Rides the full symbol-linking
/// differential check.
pub fn gen_stage3_array_iterators_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    let n = (b.next() % 5) as usize; // 0..=4 dense elements
    let elems: Vec<String> = (0..n).map(|_| small_int(&mut b).to_string()).collect();
    let lit = format!("[{}]", elems.join(","));
    let method = ["values", "keys", "entries"][b.choice(3) as usize];
    // 1..=(n+2) next() calls, so some runs step past the end into `done`.
    let advances = 1 + (b.next() as usize % (n + 2));
    let field = if b.choice(2) == 0 { "value" } else { "done" };
    let mut s = format!("var it={}.{}();", lit, method);
    for i in 0..advances {
        if i + 1 < advances {
            s.push_str(" it.next();");
        } else {
            s.push_str(&format!(" it.next().{}", field));
        }
    }
    s
}

/// Structure-aware generator for **`for-of` over an array literal** — the
/// bit-exact (result AND computron) iteration grammar. Builds a dense literal
/// and a bounded reduce/count loop over it. The loop body stays inside the
/// overflow-safe small-integer domain (`+`/`-`/`*`), so results are
/// unambiguous. Rides the full symbol-linking differential check.
pub fn gen_stage3_for_of_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    let n = (b.next() % 5) as usize; // 0..=4 dense elements
    let elems: Vec<String> = (0..n).map(|_| small_int(&mut b).to_string()).collect();
    let lit = format!("[{}]", elems.join(","));
    match b.choice(3) {
        // A reduce with +/-/*.
        0 => {
            let op = small_op(&mut b);
            let seed = small_int(&mut b);
            format!("var s={}; for (var x of {}) s=s{}x; s", seed, lit, op)
        }
        // A count.
        1 => format!("var n=0; for (var x of {}) n=n+1; n", lit),
        // String concatenation of the elements.
        _ => format!("var s=\"\"; for (var x of {}) s=s+x; s", lit),
    }
}

/// Structure-aware generator for **array spread** (`[...arr]`) — which
/// desugars to the for-of iterator loop appending each element. Emits a single
/// spread of a dense literal, optionally with leading/trailing plain elements,
/// then observes the result or its length. A single spread segment is
/// raw-exact against the pin (each additional segment carries a sub-computron
/// −8-raw residual from XS's item-chunk over-allocation, which never crosses a
/// computron boundary in a bounded program; kept single-segment here so the
/// arm is raw-clean). Rides the full symbol-linking differential check.
pub fn gen_stage3_spread_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    let n = (b.next() % 5) as usize; // 0..=4 spread elements
    let elems: Vec<String> = (0..n).map(|_| small_int(&mut b).to_string()).collect();
    let inner = format!("[{}]", elems.join(","));
    let lead = if b.choice(2) == 0 { format!("{},", small_int(&mut b)) } else { String::new() };
    let trail = if b.choice(2) == 0 { format!(",{}", small_int(&mut b)) } else { String::new() };
    let spread = format!("[{}...{}{}]", lead, inner, trail);
    match b.choice(3) {
        0 => spread,
        1 => format!("{}.length", spread),
        _ => format!("var b={}; b.length", spread),
    }
}

/// Structure-aware generator for **`for-in`** over an object literal or an
/// array — the computron-exact enumeration grammar (a sub-computron ±8-raw
/// chunk residual never crosses a `>> 16` boundary). Builds an object literal
/// (string keys) or a dense array and a count/concat loop over its keys. Rides
/// the full symbol-linking differential check.
pub fn gen_stage3_for_in_program(data: &[u8]) -> String {
    let mut b = Bytes::new(data);
    // The enumerable: either an object literal with 0..=3 single-letter string
    // keys, or a dense array of 0..=4 small ints.
    let target = if b.choice(2) == 0 {
        let n = (b.next() % 4) as usize;
        let mut props: Vec<String> = Vec::with_capacity(n);
        for i in 0..n {
            // Distinct keys a,b,c so the object is well-formed.
            let key = (b'a' + i as u8) as char;
            props.push(format!("{}:{}", key, small_int(&mut b)));
        }
        format!("{{{}}}", props.join(","))
    } else {
        let n = (b.next() % 5) as usize;
        let elems: Vec<String> = (0..n).map(|_| small_int(&mut b).to_string()).collect();
        format!("[{}]", elems.join(","))
    };
    if b.choice(2) == 0 {
        format!("var s=\"\"; for (var k in {}) s=s+k; s", target)
    } else {
        format!("var n=0; for (var k in {}) n=n+1; n", target)
    }
}

fn gen_atom(b: &mut Bytes) -> String {
    match b.choice(6) {
        0 => "true".to_string(),
        1 => "false".to_string(),
        2 => {
            // small signed integer
            let v = b.next() as i32 - 128;
            format!("{}", v)
        }
        3 => {
            // larger integer near i32 edges to exercise overflow
            let v = (b.next() as i64) << 23;
            format!("{}", v)
        }
        4 => {
            // a decimal
            let a = b.next() % 100;
            let c = b.next() % 100;
            format!("{}.{}", a, c)
        }
        _ => format!("{}", b.next() % 10),
    }
}

/// A differential divergence found by target 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub source: String,
    pub detail: String,
}

/// Target 1 body: run `source` on both engines, returning `Err` on any
/// completion / result / computron divergence. `Ok(())` also covers the
/// legitimate "endor reached an opcode outside the stage-1 subset" case
/// (a generated program using an unimplemented feature is not a
/// correctness bug), which keeps the target honest about scope.
pub fn differential_check(source: &str) -> Result<(), Divergence> {
    let oracle = match endor_oracle::run(source) {
        Some(o) => o,
        None => return Ok(()), // machine startup failure, not a finding
    };
    let endor = run_program(&oracle.bytecode);

    // Out-of-subset opcode: not a divergence, just uncovered ground.
    if let endor_vm::Halt::Unsupported(_) = endor.halt {
        return Ok(());
    }

    if oracle.completed != endor.completed {
        return Err(Divergence {
            source: source.to_string(),
            detail: format!(
                "completion: oracle={} endor={} (halt {:?})",
                oracle.completed, endor.completed, endor.halt
            ),
        });
    }
    if oracle.completed {
        if oracle.result != endor.result {
            return Err(Divergence {
                source: source.to_string(),
                detail: format!("result: oracle={:?} endor={:?}", oracle.result, endor.result),
            });
        }
        if oracle.computrons != endor.computrons {
            return Err(Divergence {
                source: source.to_string(),
                detail: format!(
                    "computrons: oracle={} endor={}",
                    oracle.computrons, endor.computrons
                ),
            });
        }
    }
    Ok(())
}

/// Differential check that **links the program's symbol table** before
/// running on endor (`run_program_with_symbols`), the full result+computron
/// comparison. Required for any grammar whose bytecode references a named
/// property or intrinsic the engine must recognize by name — the stage-3
/// arrays surface needs it so `length` routes to the array length semantics
/// (a bare [`differential_check`] runs without symbols, where `arr.length`
/// would be read as an ordinary numeric-id property and diverge).
pub fn differential_check_with_symbols(source: &str) -> Result<(), Divergence> {
    let oracle = match endor_oracle::run(source) {
        Some(o) => o,
        None => return Ok(()),
    };
    let endor = endor_vm::run_program_with_symbols(&oracle.bytecode, &oracle.symbols);
    if let endor_vm::Halt::Unsupported(_) = endor.halt {
        return Ok(());
    }
    if oracle.completed != endor.completed {
        return Err(Divergence {
            source: source.to_string(),
            detail: format!(
                "completion: oracle={} endor={} (halt {:?})",
                oracle.completed, endor.completed, endor.halt
            ),
        });
    }
    if oracle.completed {
        if oracle.result != endor.result {
            return Err(Divergence {
                source: source.to_string(),
                detail: format!("result: oracle={:?} endor={:?}", oracle.result, endor.result),
            });
        }
        if oracle.computrons != endor.computrons {
            return Err(Divergence {
                source: source.to_string(),
                detail: format!(
                    "computrons: oracle={} endor={}",
                    oracle.computrons, endor.computrons
                ),
            });
        }
    }
    Ok(())
}

/// Differential check for the **stage-2 allocating surface**: compares
/// completion kind and result string, but not computrons, which are not
/// yet bit-exact while run-time slot/chunk allocation metering awaits
/// the faithful heap (`endor_vm::interp` § Metering scope). A result or
/// completion divergence on a valid generated program is still a real
/// finding — the frame/scope/loop semantics must match C-XS.
pub fn differential_check_result_only(source: &str) -> Result<(), Divergence> {
    let oracle = match endor_oracle::run(source) {
        Some(o) => o,
        None => return Ok(()),
    };
    let endor = run_program(&oracle.bytecode);
    if let endor_vm::Halt::Unsupported(_) = endor.halt {
        return Ok(());
    }
    if oracle.completed != endor.completed {
        return Err(Divergence {
            source: source.to_string(),
            detail: format!(
                "completion: oracle={} endor={} (halt {:?})",
                oracle.completed, endor.completed, endor.halt
            ),
        });
    }
    if oracle.completed && oracle.result != endor.result {
        return Err(Divergence {
            source: source.to_string(),
            detail: format!("result: oracle={:?} endor={:?}", oracle.result, endor.result),
        });
    }
    Ok(())
}

/// Target 2 body: the decoder and interpreter must not panic on
/// arbitrary bytes. Returns the disassembled length so a caller can
/// assert liveness; the point is simply that it returns.
pub fn decoder_is_panic_free(bytes: &[u8]) -> usize {
    let dis = disassemble(bytes);
    // The interpreter must also degrade gracefully (Halt::Decode on a
    // truncated or invalid stream), never panic.
    let _ = run_program(bytes);
    dis.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_programs_agree_with_oracle() {
        // Sweep a spread of seeds; every generated subset program must
        // hold bit-exact (result, computron) agreement.
        let mut checked = 0;
        for seed in 0u32..300 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(4 + (seed % 12)) {
                buf.push(data[(k as usize) % 4].wrapping_add(k as u8));
            }
            let prog = gen_program(&buf);
            match differential_check(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn generated_statement_programs_agree_on_results() {
        // The stage-2 generator's var/loop programs must all agree with
        // C-XS on the completion value (computron parity for this
        // allocating surface awaits the faithful heap).
        let mut checked = 0;
        for seed in 0u32..300 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(6 + (seed % 10)) {
                buf.push(data[(k as usize) % 4].wrapping_add(k as u8 * 7));
            }
            let prog = gen_statement_program(&buf);
            match differential_check_result_only(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-2 differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn generated_stage3_arrays_programs_agree_bit_exact() {
        // The stage-3 arrays generator's literals, indexed get/set, grow, and
        // length get/set programs must ALL agree with C-XS bit-for-bit
        // (result AND computron): the array item chunk's allocation metering
        // and the length accessor are modeled faithfully, so they ride the
        // full `differential_check`. Sweep a spread of seeds so every branch
        // of the grammar (all six shapes) and the hole/no-hole literal split
        // are exercised.
        let mut checked = 0;
        let mut features = [false; 4]; // literal-only, indexed read, a write, a length op
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            // Widen the byte budget so the generator's later `choice`/`next`
            // reads are never starved (a short buffer biases the shape).
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(7))
                        .wrapping_add((seed as u8).wrapping_mul(3)),
                );
            }
            let prog = gen_stage3_arrays_program(&buf);
            distinct.insert(prog.clone());
            if !prog.starts_with("var a=") {
                features[0] = true; // a bare literal
            }
            if prog.contains("]=") {
                features[2] = true; // an element write
            } else if prog.contains('[') && prog.starts_with("var a=") {
                features[1] = true; // an indexed read
            }
            if prog.contains("length") {
                features[3] = true; // a length get/set
            }
            // Arrays need the symbol table linked so `length` routes to the
            // array length semantics.
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 arrays differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        // The sweep must be real, varied coverage: a spread of distinct
        // programs, and every top-level grammar feature exercised at least
        // once.
        assert!(distinct.len() > 30, "arrays sweep too uniform: {} distinct", distinct.len());
        for (i, f) in features.iter().enumerate() {
            assert!(*f, "arrays grammar feature {} never generated", i);
        }
    }

    #[test]
    fn generated_stage3_array_methods_agree_bit_exact() {
        // The dense push/pop/indexOf fast paths meter their mxMeterSome
        // annotations and chunk (re)size faithfully, so they ride the full
        // result+computron differential (symbol-linked, since the method
        // names resolve through the program symbol table). Sweep a spread of
        // seeds so every method shape and a range of receiver lengths appear.
        let mut checked = 0;
        let mut methods = [false; 3]; // push, pop, indexOf seen
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(5))
                        .wrapping_add((seed as u8).wrapping_mul(2)),
                );
            }
            let prog = gen_stage3_array_methods_program(&buf);
            distinct.insert(prog.clone());
            if prog.contains(".push(") {
                methods[0] = true;
            } else if prog.contains(".pop(") {
                methods[1] = true;
            } else if prog.contains(".indexOf(") {
                methods[2] = true;
            }
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 array-methods differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        assert!(distinct.len() > 30, "methods sweep too uniform: {} distinct", distinct.len());
        for (i, m) in methods.iter().enumerate() {
            assert!(*m, "array method {} never generated", i);
        }
    }

    #[test]
    fn generated_stage3_array_iterators_agree_bit_exact() {
        // The array iterator objects (values/keys/entries + next over the
        // reused result object) meter their fxNewIteratorInstance creation and
        // per-next yield/element-read faithfully, so they ride the full
        // result+computron differential. Sweep a spread of seeds so every
        // iterator kind, a range of lengths, and both past-the-end and
        // in-range exhaustion appear.
        let mut checked = 0;
        let mut kinds = [false; 3]; // values, keys, entries seen
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(11))
                        .wrapping_add((seed as u8).wrapping_mul(4)),
                );
            }
            let prog = gen_stage3_array_iterators_program(&buf);
            distinct.insert(prog.clone());
            if prog.contains(".values(") {
                kinds[0] = true;
            } else if prog.contains(".keys(") {
                kinds[1] = true;
            } else if prog.contains(".entries(") {
                kinds[2] = true;
            }
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 array-iterators differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        assert!(distinct.len() > 30, "iterators sweep too uniform: {} distinct", distinct.len());
        for (i, k) in kinds.iter().enumerate() {
            assert!(*k, "iterator kind {} never generated", i);
        }
    }

    #[test]
    fn generated_stage3_for_of_programs_agree_bit_exact() {
        // for-of over an array literal drives fxGetIterator + the values
        // iterator's per-element next() protocol, all metered faithfully, so
        // it rides the full result+computron differential. Sweep a spread of
        // seeds over the three loop shapes and a range of array lengths
        // (including the empty array).
        let mut checked = 0;
        let mut shapes = [false; 3];
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(13))
                        .wrapping_add((seed as u8).wrapping_mul(6)),
                );
            }
            let prog = gen_stage3_for_of_program(&buf);
            distinct.insert(prog.clone());
            if prog.contains("n=n+1") {
                shapes[1] = true;
            } else if prog.contains("s=\"\"") {
                shapes[2] = true;
            } else {
                shapes[0] = true;
            }
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 for-of differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        assert!(distinct.len() > 20, "for-of sweep too uniform: {} distinct", distinct.len());
        for (i, s) in shapes.iter().enumerate() {
            assert!(*s, "for-of shape {} never generated", i);
        }
    }

    #[test]
    fn generated_stage3_spread_programs_agree_bit_exact() {
        // Single-segment array spread desugars to the for-of iterator loop
        // appending each element; raw-exact against the pin. Sweep a spread of
        // seeds over the three observation shapes and a range of lengths and
        // lead/trail combinations.
        let mut checked = 0;
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(17))
                        .wrapping_add((seed as u8).wrapping_mul(9)),
                );
            }
            let prog = gen_stage3_spread_program(&buf);
            distinct.insert(prog.clone());
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 spread differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        assert!(distinct.len() > 20, "spread sweep too uniform: {} distinct", distinct.len());
    }

    #[test]
    fn generated_stage3_for_in_programs_agree_bit_exact() {
        // for-in over an object literal or array drives the enumerator's key
        // collection + per-key yield, computron-exact. Sweep a spread of seeds
        // over object/array targets, a range of key counts (including empty),
        // and both loop bodies.
        let mut checked = 0;
        let mut kinds = [false; 2]; // object target, array target
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0u32..600 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(16 + (seed % 24)) {
                buf.push(
                    data[(k as usize) % 4]
                        .wrapping_add((k as u8).wrapping_mul(19))
                        .wrapping_add((seed as u8).wrapping_mul(2)),
                );
            }
            let prog = gen_stage3_for_in_program(&buf);
            distinct.insert(prog.clone());
            if prog.contains("in {") {
                kinds[0] = true;
            } else if prog.contains("in [") {
                kinds[1] = true;
            }
            match differential_check_with_symbols(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-3 for-in differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        assert!(distinct.len() > 20, "for-in sweep too uniform: {} distinct", distinct.len());
        for (i, k) in kinds.iter().enumerate() {
            assert!(*k, "for-in target {} never generated", i);
        }
    }

    #[test]
    fn generated_stage2b_programs_agree_bit_exact() {
        // The stage-2b generator's object / call / closure / exception
        // programs must ALL agree with C-XS bit-for-bit (result AND
        // computron) — the object model, call frames, closure cells, and
        // the exception jump-chain are metered faithfully, so unlike the
        // stage-2 result-only surface these ride the full
        // `differential_check`. Sweep a spread of seeds so every branch of
        // the grammar (both object shapes, all three call shapes, both
        // closure shapes, all three exception shapes) is exercised.
        let mut checked = 0;
        let mut kinds = [0usize; 4];
        for seed in 0u32..400 {
            let data = seed.to_le_bytes();
            let mut buf = Vec::new();
            for k in 0..(6 + (seed % 14)) {
                buf.push(data[(k as usize) % 4].wrapping_add(k as u8 * 5));
            }
            kinds[(buf[0] % 4) as usize] += 1;
            let prog = gen_stage2b_program(&buf);
            match differential_check(&prog) {
                Ok(()) => checked += 1,
                Err(d) => panic!("stage-2b differential divergence: {:?}", d),
            }
        }
        assert!(checked > 0);
        // Every grammar family must have been generated at least once, so
        // the sweep is real coverage, not one branch 400 times.
        for (i, c) in kinds.iter().enumerate() {
            assert!(*c > 0, "grammar family {} never generated", i);
        }
    }

    #[test]
    fn decoder_never_panics_on_arbitrary_bytes() {
        for seed in 0u32..2000 {
            let mut s = seed.wrapping_mul(2654435761);
            let n = (s % 40) as usize;
            let mut bytes = Vec::with_capacity(n);
            for _ in 0..n {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                bytes.push((s >> 16) as u8);
            }
            let _ = decoder_is_panic_free(&bytes);
        }
        // Truncated operand: NUMBER opcode (0x8f) needs 8 bytes, give 2.
        let _ = decoder_is_panic_free(&[0x8f, 0x00, 0x00]);
        // Backward branch off the front.
        let _ = decoder_is_panic_free(&[0x16, 0x80]);
    }
}
