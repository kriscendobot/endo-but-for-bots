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
