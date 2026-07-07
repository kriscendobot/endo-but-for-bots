//! Byte-identity: `endor_compile::compile(src)` must equal
//! `endor_oracle::run(src).bytecode` byte for byte on a corpus of
//! expression + simple-statement programs (stage-5 child 5/7 bar).
//!
//! XS's coder is the ground truth: node shapes, operand widths, branch
//! sizing, and the constant encodings all leak into the bytes, so a
//! single wrong byte fails the test. On divergence the harness prints an
//! **opcode-level diff** from a small disassembler — a triage tool that
//! pays for itself the first time a width or a branch displacement is off.

use endor_compile::compile;

// --------------------------- disassembler ----------------------------
//
// A minimal XS-bytecode disassembler for triage. It knows just enough of
// the operand grammar to line up two byte streams opcode-by-opcode; it is
// not part of the byte-identity contract, only the failure message.

/// Opcode name + operand size class, generated the same way the coder's
/// `opcodes` module is (from the pin's opcode table). We keep a compact
/// hand table of the names the ported surface can emit; anything else
/// prints as `?<byte>` with a best-effort length so the diff still aligns
/// on fixed-size ops.
fn disasm(code: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut pc = 0usize;
    while pc < code.len() {
        let op = code[pc];
        let (name, operand) = decode(op, code, pc);
        out.push(format!("{:04} {}{}", pc, name, operand.text));
        pc += 1 + operand.len;
    }
    out
}

struct Operand {
    len: usize,
    text: String,
}

fn decode(op: u8, code: &[u8], pc: usize) -> (&'static str, Operand) {
    use endor_compile::opcodes as x;
    let o = op as i32;
    let i8_at = |k: usize| code.get(pc + k).map(|b| *b as i8 as i32).unwrap_or(0);
    let i16_at = |k: usize| {
        let a = *code.get(pc + k).unwrap_or(&0);
        let b = *code.get(pc + k + 1).unwrap_or(&0);
        i16::from_le_bytes([a, b]) as i32
    };
    let name_of = |o: i32| -> &'static str { op_name(o) };
    // branch (_1/_2/_4)
    let branch1 = [
        x::XS_CODE_BRANCH_1, x::XS_CODE_BRANCH_CHAIN_1, x::XS_CODE_BRANCH_COALESCE_1,
        x::XS_CODE_BRANCH_ELSE_1, x::XS_CODE_BRANCH_IF_1, x::XS_CODE_BRANCH_STATUS_1,
    ];
    if branch1.contains(&o) {
        return (name_of(o), Operand { len: 1, text: format!(" {:+}", i8_at(1)) });
    }
    if branch1.iter().any(|b| *b + 1 == o) {
        return (name_of(o), Operand { len: 2, text: format!(" {:+}", i16_at(1)) });
    }
    match o {
        x::XS_CODE_INTEGER_1 => (name_of(o), Operand { len: 1, text: format!(" {}", i8_at(1)) }),
        x::XS_CODE_INTEGER_2 => (name_of(o), Operand { len: 2, text: format!(" {}", i16_at(1)) }),
        x::XS_CODE_INTEGER_4 => (name_of(o), Operand { len: 4, text: String::from(" i32") }),
        x::XS_CODE_NUMBER => (name_of(o), Operand { len: 8, text: String::from(" f64") }),
        x::XS_CODE_STRING_1 => {
            let n = *code.get(pc + 1).unwrap_or(&0) as usize;
            (name_of(o), Operand { len: 1 + n, text: format!(" [{}]", n) })
        }
        x::XS_CODE_BEGIN_SLOPPY | x::XS_CODE_BEGIN_STRICT | x::XS_CODE_BEGIN_STRICT_BASE
        | x::XS_CODE_BEGIN_STRICT_DERIVED | x::XS_CODE_BEGIN_STRICT_FIELD => {
            (name_of(o), Operand { len: 1, text: format!(" {}", code.get(pc + 1).copied().unwrap_or(0)) })
        }
        x::XS_CODE_RESERVE_1 | x::XS_CODE_UNWIND_1 => {
            (name_of(o), Operand { len: 1, text: format!(" #{}", code.get(pc + 1).copied().unwrap_or(0)) })
        }
        _ => (name_of(o), Operand { len: 0, text: String::new() }),
    }
}

/// Reverse the opcodes const table into a name. Cheap linear scan (the
/// disassembler runs only on failure).
fn op_name(o: i32) -> &'static str {
    macro_rules! names { ($($n:ident),* $(,)?) => {
        match o { $( x if x == endor_compile::opcodes::$n => stringify!($n), )* _ => "?" }
    }}
    names!(
        XS_NO_CODE, XS_CODE_ADD, XS_CODE_SUBTRACT, XS_CODE_MULTIPLY, XS_CODE_DIVIDE,
        XS_CODE_MODULO, XS_CODE_EXPONENTIATION, XS_CODE_BIT_AND, XS_CODE_BIT_OR,
        XS_CODE_BIT_XOR, XS_CODE_BIT_NOT, XS_CODE_LEFT_SHIFT, XS_CODE_SIGNED_RIGHT_SHIFT,
        XS_CODE_UNSIGNED_RIGHT_SHIFT, XS_CODE_EQUAL, XS_CODE_NOT_EQUAL, XS_CODE_STRICT_EQUAL,
        XS_CODE_STRICT_NOT_EQUAL, XS_CODE_LESS, XS_CODE_LESS_EQUAL, XS_CODE_MORE,
        XS_CODE_MORE_EQUAL, XS_CODE_INSTANCEOF, XS_CODE_IN, XS_CODE_NOT, XS_CODE_MINUS,
        XS_CODE_PLUS, XS_CODE_VOID, XS_CODE_TYPEOF, XS_CODE_TRUE, XS_CODE_FALSE,
        XS_CODE_NULL, XS_CODE_UNDEFINED, XS_CODE_INTEGER_1, XS_CODE_INTEGER_2,
        XS_CODE_INTEGER_4, XS_CODE_NUMBER, XS_CODE_STRING_1, XS_CODE_STRING_2,
        XS_CODE_BEGIN_SLOPPY, XS_CODE_BEGIN_STRICT, XS_CODE_EVAL_ENVIRONMENT,
        XS_CODE_PROGRAM_ENVIRONMENT, XS_CODE_RESERVE_1, XS_CODE_SET_RESULT, XS_CODE_RETURN,
        XS_CODE_POP, XS_CODE_DUB, XS_CODE_UNWIND_1, XS_CODE_BRANCH_1, XS_CODE_BRANCH_2,
        XS_CODE_BRANCH_ELSE_1, XS_CODE_BRANCH_ELSE_2, XS_CODE_BRANCH_IF_1, XS_CODE_BRANCH_IF_2,
        XS_CODE_BRANCH_COALESCE_1, XS_CODE_BRANCH_COALESCE_2,
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" ")
}

/// Assert byte-identity for every source, printing an opcode-level diff
/// for the first few divergences.
fn assert_identical(corpus: &[&str]) {
    let mut fails: Vec<String> = Vec::new();
    for &src in corpus {
        let want = match endor_oracle::run(src) {
            Some(o) => o.bytecode,
            None => {
                fails.push(format!("{src:?}: oracle returned no bytecode"));
                continue;
            }
        };
        match compile(src) {
            Ok(got) if got == want => {}
            Ok(got) => {
                let wd = disasm(&want);
                let gd = disasm(&got);
                let mut diff = String::new();
                let n = wd.len().max(gd.len());
                for i in 0..n {
                    let w = wd.get(i).map(String::as_str).unwrap_or("--");
                    let g = gd.get(i).map(String::as_str).unwrap_or("--");
                    let mark = if w == g { "  " } else { "!!" };
                    diff.push_str(&format!("    {mark} want[{w}]  got[{g}]\n"));
                }
                fails.push(format!(
                    "{src:?} MISMATCH\n  want {}\n  got  {}\n{}",
                    hex(&want),
                    hex(&got),
                    diff
                ));
            }
            Err(e) => fails.push(format!("{src:?}: compile error {e:?} (want {})", hex(&want))),
        }
    }
    if !fails.is_empty() {
        panic!("{} divergence(s):\n{}", fails.len(), fails.join("\n"));
    }
}

#[test]
fn literals_and_operators() {
    assert_identical(&[
        // literals of every scalar kind
        "1", "0", "-0", "300", "70000", "2147483647", "1.5", "0.1", "1e300",
        "true", "false", "null", "\"\"", "\"hi\"", "\"a b c\"",
        // bigint literals (limb encoding)
        "0n", "1n", "255n", "256n", "10n", "0xdeadbeefn", "4294967295n",
        "4294967296n", "18446744073709551616n", "0o17n", "0b1010n",
        // arithmetic / precedence
        "1+2", "1-2", "1*2", "1/2", "1%2", "2**3", "1+2*3", "1*2+3", "(1+2)*3",
        "2**3**2", "1-2-3",
        // bitwise / shift
        "1&2", "1|2", "1^2", "~5", "1<<2", "8>>1", "8>>>1",
        // relational / equality
        "1<2", "1>2", "1<=2", "1>=2", "1==2", "1!=2", "1===2", "1!==2",
        // unary
        "-3", "+3", "!0", "!1", "~0", "void 0", "typeof 1", "typeof \"x\"",
    ]);
}

#[test]
fn logical_conditional_sequence() {
    assert_identical(&[
        "1&&2", "1||2", "1??2", "1&&2&&3", "1||2||3", "1&&2||3", "1??2??3",
        "1?2:3", "true?1:0", "0?1:2?3:4", "1?2?3:4:5",
        "1,2", "1,2,3", "(1,2)", "(1,2)+3", "1,2?3:4",
        "1&&(2||3)", "(1||2)&&3", "1?2:3,4",
    ]);
}

#[test]
fn statements_if_block() {
    assert_identical(&[
        "1;", "1;2;", "1;2;3;", ";", ";;", "1;;2;",
        "{}", "{1;}", "{1;2;}", "{{1;}}", "{;}",
        "if(1)2;", "if(0)2;", "if(1)2;else 3;", "if(1){2;}else{3;}",
        "if(1)if(2)3;else 4;", "if(1)2;else if(3)4;else 5;",
        "if(1&&2)3;else 4;", "if(1?2:3)4;",
        "{if(1)2;}", "if(1){}",
    ]);
}

// NB: `endor_oracle::run` *executes* the program, so every fixture must
// terminate. Byte-identity is about compilation, not the runtime value,
// so a `while(0)` / `for(;0;)` head exercises the same emission as a
// `while(1)` head without spinning the oracle; `break` bodies also
// terminate. Both forms are covered.
#[test]
fn control_flow_loops() {
    assert_identical(&[
        // while / do-while
        "while(0)2;", "while(0);", "while(0){2;}", "while(1)break;",
        "while(0)continue;", "do 1;while(0);", "do{1;}while(0);",
        "do break;while(1);", "do continue;while(0);",
        // C-style for
        "for(;;)break;", "for(;1;)break;", "for(;0;)1;",
        "for(1;;)break;", "for(1;0;2)3;", "for(;;){1;break;}",
        "for(;0;)continue;",
        // nested loops + break/continue
        "while(1){while(2)break;break;}", "while(0){while(2)break;continue;}",
        "for(;;){for(;;)break;break;}",
    ]);
}

#[test]
fn control_flow_labels() {
    assert_identical(&[
        "a:while(1)break a;", "a:while(0)continue a;",
        "a:for(;;)break a;", "a:for(;0;)continue a;",
        "a:b:while(1)break a;", "a:b:while(0)continue b;",
        "a:while(1)while(2)break a;", "a:while(0)while(2)continue a;",
        "a:1;", "a:{1;}", "a:{break a;}",
        "foo:while(0){continue foo;}",
    ]);
}

#[test]
fn control_flow_switch() {
    assert_identical(&[
        "switch(1){}", "switch(1){case 1:break;}",
        "switch(1){case 1:2;break;case 2:3;}",
        "switch(1){default:1;}", "switch(1){case 1:break;default:2;}",
        "switch(1){case 1:case 2:3;break;default:4;}",
        "switch(1){case 1:{2;}break;}",
        "switch(1){case 1:while(2)break;break;}",
    ]);
}

#[test]
fn control_flow_throw_debugger() {
    assert_identical(&[
        "throw 1;", "throw 1+2;", "throw\"x\";", "debugger;",
        "if(1)throw 2;", "while(1)throw 2;",
    ]);
}

#[test]
fn this_and_regexp() {
    assert_identical(&[
        "this;", "this,1;", "typeof this;", "this===this;",
        "/abc/;", "/abc/g;", "/a.c/gi;", "/x/;", "/[0-9]+/m;",
        "/a/,/b/;", "if(1)/x/;",
    ]);
}

#[test]
fn control_flow_try() {
    assert_identical(&[
        "try{1;}catch{2;}", "try{1;}finally{2;}",
        "try{1;}catch{2;}finally{3;}", "try{}catch{}",
        "try{}finally{}", "try{throw 1;}catch{2;}",
        "try{1;}catch{}finally{}",
        // break/continue crossing a finally (target finalization)
        "while(1){try{break;}finally{1;}}",
        "while(0){try{continue;}finally{1;}}",
        "for(;;){try{break;}finally{2;}}",
        "try{try{1;}finally{2;}}finally{3;}",
    ]);
}

#[test]
fn wide_operands_and_branch_widths() {
    // Force INTEGER width transitions and a long branch (BRANCH_2) by
    // padding the then-arm with many statements.
    let long_then = format!("if(1){{{}}}else 2;", "3;".repeat(120));
    assert_identical(&[
        "127", "128", "255", "256", "32767", "32768", "-128", "-129",
        "-32768", "-32769",
        long_then.as_str(),
    ]);
}
