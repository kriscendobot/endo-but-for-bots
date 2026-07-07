//! The coder — a transliteration of `c/moddable/xs/sources/xsCode.c` at
//! the oracle pin (design § roadmap row 5; Design Decisions 4 and 5).
//! It is the last stratum of the stage-5 pipeline (lexer → parser →
//! scoper → **coder**) and the first that produces the byte-identity
//! evidence the stage bar is measured against: `compile(src)` must equal
//! `endor_oracle::run(src).bytecode` byte for byte.
//!
//! **Scope of this child (5/7).** The emitter *framework* is ported in
//! full and faithfully: the code-record list ([`Code`]), the `fxCoderAdd*`
//! constructors, targets/fixups, stack-depth accounting, and — the part
//! that visibly shapes the bytes — XS's exact **three-pass** serializer
//! (`fxParserCode`): pass 1 sizes every record with branches assumed
//! widest and accrues the `delta` slack; pass 2 chooses each branch's
//! `_1`/`_2`/`_4` width from the now-known target offsets, narrowing
//! `size`; pass 3 emits the bytes with the chosen widths and the
//! back-patched branch displacements. That is ported as the algorithm,
//! not an equivalent.
//!
//! The *node* surface ported here is the expression + simple-statement
//! half of `xsCode.c`: literals of every scalar kind, every
//! unary/binary/relational/logical/coalescing operator, the conditional,
//! sequence, expression statements, `if`/`else`, and blocks — the
//! constructs whose emission needs neither the scoper's per-access slot
//! resolution nor a nested function body. Identifier/property
//! loads-and-stores, `var`/lexical declarations, calls, `new`, member
//! access, object/array/template construction, and destructuring are the
//! back half, deferred to child 6; the honest fold is named in the
//! crate README and the completion report.
//!
//! **Child 6, first slice: symbol-free control flow.** The loop forms
//! (`while` / `do` / C-style `for`), labeled statements with break /
//! continue resolution (XS's `firstBreakTarget` / `firstContinueTarget`
//! label-target stacks and the environment/scope adjustments), `switch`,
//! `throw`, `debugger`, and `try` / `catch` / `finally` (including the
//! alias / finalize / jump target machinery that threads break / continue
//! / return out through a `finally` via the selector local) are ported
//! here. What each of these needs and *doesn't* yet have is a symbol
//! (atom) table: a declaring loop / block / `switch` scope, a `catch(e)`
//! binding, `for-in` / `for-of` / `for-await-of` (they emit `GET_PROPERTY
//! next` etc.), and `with` all reach a `NEW_LOCAL` / `SYMBOL` op keyed on
//! a source name, so they assert loudly and are deferred to the
//! atom-table slice. `for(let …)` / declaring cases are the same fold.

#![allow(clippy::too_many_arguments)]

use crate::ast::{Item, Node, Value};
use crate::opcodes::*;
use crate::scoper::{node_key, ScopeTree};
use crate::token::Token;
use std::collections::HashMap;

/// The payload a code record carries beside its mutable `id`. Mirrors the
/// `txByteCode` subtype union XS switches on: a plain byte, a branch to a
/// target, a placed target (`XS_NO_CODE`), an index/integer/number/string
/// operand, or a symbol/variable (deferred to child 6, modeled so the
/// framework compiles).
#[derive(Clone, Debug)]
enum Payload {
    /// A fixed opcode with no operand.
    Byte,
    /// A placed target (`XS_NO_CODE`): its `offset` is set each sizing
    /// pass. `tid` indexes the target arena.
    Target { tid: usize },
    /// A branch/`CODE`/`CATCH` record referencing target `tid`.
    Branch { tid: usize },
    /// A `u1`/`u2` index operand (`BEGIN_*`, `RESERVE_1`, `UNWIND_1`,
    /// `LINE`, `HOST`, `NEW_TEMPORARY`…). `plus_one` selects the
    /// local/closure family whose serialized value is `index + 1`.
    Index { index: i32, plus_one: bool },
    /// A signed integer operand (`INTEGER_1`, `RUN_1`, `RUN_TAIL_1`).
    Integer { value: i32 },
    /// An IEEE-754 double operand (`NUMBER`).
    Number { value: f64 },
    /// A string operand (`STRING_1`): `bytes` includes XS's trailing NUL,
    /// and `len` (== `bytes.len()`) is both the emitted length and the
    /// width selector.
    Str { bytes: Vec<u8>, len: i32 },
    /// A symbol operand (`GET_VARIABLE` / `GET_PROPERTY` / …). Carries the
    /// index of the interned symbol in the coder's atom table; the emitted
    /// 2-byte `txID` is resolved from that table's bucket-walk assignment
    /// once every emitted symbol's `usage` is known.
    Symbol { sym: usize },
    /// A BigInt operand (`BIGINT_1`): the value as XS stores it — a
    /// little-endian array of `txU4` limbs (`bigint->data`) — with
    /// `measure` (== `bytes.len()`, `bigint->size * 4`) the emitted
    /// length and width selector.
    BigInt { bytes: Vec<u8>, measure: i32 },
}

// ============================= atom table ==============================

/// XS's `parserTableModulo` at the oracle pin (the shim's creation record,
/// `endor_shim.c`). The symbol hash bucket count.
const SYMBOL_MODULO: u32 = 1993;

/// The built-in symbols `fxInitializeParser` interns *before* lexing, in
/// exact source order (`xsScript.c`). They occupy their hash buckets ahead
/// of every program symbol, so their position is part of the ID contract
/// whenever the program (or the coder) emits one of them.
const SEED_SYMBOLS: &[&str] = &[
    "Object", "__dirname", "__filename", "__jsx__", "__proto__", "*", "args",
    "arguments", "=>", "as", "async", "await", "call", "caller", "constructor",
    "default", "done", "eval", "exports", "fill", "freeze", "from", "get", "id",
    "include", "Infinity", "json", "length", "let", "meta", "module", "name",
    "NaN", "Native", "native", "next", "new.target", "of", "#constructor",
    "prototype", "RangeError", "raw", "return", "set", "slice", "SyntaxError",
    "static", "String", "target", "this", "throw", "toString", "undefined",
    "uri", "using", "value", "with", "yield",
];

/// One interned symbol — XS's `txSymbol` (the fields the coder reads).
#[derive(Clone, Debug)]
struct SymEntry {
    /// The interned spelling (kept for the atom-table dump / debugging).
    #[allow(dead_code)]
    string: String,
    /// `sum % symbolModulo`.
    bucket: u32,
    /// `usage & 1` — set when the symbol is actually emitted in code; only
    /// used symbols are assigned an ID.
    usage: bool,
    /// The assigned `txID` (1-based), or 0 until [`SymbolTable::assign_ids`].
    id: i32,
}

/// The parser/coder symbol table — a transliteration of `parser->symbolTable`
/// and the `fxNewParserSymbol` interning discipline: hash by
/// `sum = Σ (sum<<1 + ch)` masked to 31 bits, bucket `sum % modulo`, new
/// symbols **prepended** to their bucket. IDs are assigned by walking the
/// buckets in index order and, within a bucket, most-recent-first (prepend
/// order), numbering only the `usage` symbols — exactly `fxParserCode`'s
/// symbol-table walk. That order leaks into every symbol operand's bytes.
struct SymbolTable {
    /// Interned symbols in insertion (chronological) order.
    entries: Vec<SymEntry>,
    index: HashMap<String, usize>,
}

impl SymbolTable {
    /// A table pre-seeded with the built-in symbols, matching XS's
    /// `fxInitializeParser`.
    fn seeded() -> SymbolTable {
        let mut t = SymbolTable { entries: Vec::new(), index: HashMap::new() };
        for s in SEED_SYMBOLS {
            t.intern(s);
        }
        t
    }

    /// `fxNewParserSymbol`'s hash: `sum = (sum << 1) + ch` over the bytes
    /// (C promotes `char`, signed on the pin's platform, to `int`), masked
    /// to 31 bits.
    fn hash(s: &str) -> u32 {
        let mut sum: u32 = 0;
        for &b in s.as_bytes() {
            sum = sum.wrapping_shl(1).wrapping_add((b as i8 as i32) as u32);
        }
        sum & 0x7FFF_FFFF
    }

    /// Intern `s`, returning its stable index. New symbols get a bucket but
    /// no usage; re-interning returns the existing index.
    fn intern(&mut self, s: &str) -> usize {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let bucket = SymbolTable::hash(s) % SYMBOL_MODULO;
        let i = self.entries.len();
        self.entries.push(SymEntry { string: s.to_string(), bucket, usage: false, id: 0 });
        self.index.insert(s.to_string(), i);
        i
    }

    /// Intern `s` and mark it emitted (`usage |= 1`), returning its index.
    fn use_symbol(&mut self, s: &str) -> usize {
        let i = self.intern(s);
        self.entries[i].usage = true;
        i
    }

    /// `fxParserCode`'s ID walk: buckets in index order, most-recent-first
    /// within each bucket, numbering only `usage` symbols from 1.
    fn assign_ids(&mut self) {
        // Per-bucket index lists in prepend (reverse-insertion) order.
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); SYMBOL_MODULO as usize];
        for i in 0..self.entries.len() {
            buckets[self.entries[i].bucket as usize].push(i);
        }
        let mut id: i32 = 1;
        for bucket in &buckets {
            for &i in bucket.iter().rev() {
                if self.entries[i].usage {
                    self.entries[i].id = id;
                    id += 1;
                }
            }
        }
    }

    /// The id-by-index table for emission.
    fn id_table(&self) -> Vec<i32> {
        self.entries.iter().map(|e| e.id).collect()
    }
}

/// One code record — XS's `txByteCode` with its subtype fields. `id` is
/// the opcode byte value and is *mutated* across the sizing passes
/// exactly as `fxParserCode` mutates `code->id` (a contiguous `+1`/`+2`
/// widens `_1`→`_2`→`_4`).
#[derive(Clone, Debug)]
struct Code {
    id: i32,
    stack_level: i32,
    payload: Payload,
}

/// A branch target — XS's `txTargetCode`. `offset` is the byte position
/// the target resolves to, recomputed each sizing pass. The
/// `environment_level` / `scope_level` / `stack_level` snapshots and the
/// `labels` / `next_target` / `original` fields are the coder's live
/// target-stack machinery (`firstBreakTarget` / `firstContinueTarget` /
/// `returnTarget`), consumed by break/continue resolution and the `try`
/// finalizer.
#[derive(Clone, Debug, Default)]
struct Target {
    index: u32,
    offset: i32,
    used: bool,
    /// The environment (`with`) nesting the target was created at.
    environment_level: i32,
    /// The frame slot level the target was created at.
    scope_level: i32,
    /// The stack depth the target was created at.
    stack_level: i32,
    /// The label symbols a break/continue target answers to (XS's
    /// `target->label` `nextLabel` chain). `None` is the anonymous
    /// (loop / `switch`) label; a `Some(name)` is a labeled statement.
    labels: Vec<Option<String>>,
    /// The next target down the break/continue/return stack.
    next_target: Option<usize>,
    /// For a `try` alias, the original target it forwards to.
    original: Option<usize>,
}

/// The coder — XS's `txCoder`. Holds the record list, the target arena,
/// the running stack/scope counters, and the program/eval flags the node
/// emitters branch on.
pub struct Coder<'a> {
    codes: Vec<Code>,
    targets: Vec<Target>,
    stack_level: i32,
    scope_level: i32,
    environment_level: i32,
    target_index: u32,
    program_flag: bool,
    eval_flag: bool,
    /// XS's `coder->firstBreakTarget` / `firstContinueTarget` /
    /// `returnTarget` — heads of the target stacks the loop / `switch` /
    /// `try` / label coders push and pop.
    first_break_target: Option<usize>,
    first_continue_target: Option<usize>,
    return_target: Option<usize>,
    /// The atom table (`parser->symbolTable`), seeded with the built-ins.
    symbols: SymbolTable,
    tree: &'a ScopeTree,
}

impl<'a> Coder<'a> {
    fn new(tree: &'a ScopeTree) -> Coder<'a> {
        Coder {
            codes: Vec::new(),
            targets: Vec::new(),
            stack_level: 0,
            scope_level: 0,
            environment_level: 0,
            target_index: 0,
            program_flag: false,
            eval_flag: false,
            first_break_target: None,
            first_continue_target: None,
            return_target: None,
            symbols: SymbolTable::seeded(),
            tree,
        }
    }

    /// Pre-intern every source symbol in AST pre-order, mirroring XS's
    /// lex-time interning so the atom table's insertion order (and thus the
    /// bucket-list order that decides within-bucket IDs) matches. Runs once
    /// before coding; usage is set later, as each symbol is emitted.
    ///
    /// Fold: AST pre-order equals XS's lex order for the ported surface
    /// (identifier refs, member property names). Constructs where the
    /// parser interns in an order that diverges from AST pre-order (e.g.
    /// numeric/computed property keys, some declaration positions) are a
    /// named edge for the declaration/object slices.
    fn intern_tree(&mut self, item: &Item) {
        match item {
            Item::Symbol(s) => {
                self.symbols.intern(s);
            }
            Item::Node(n) => {
                for c in &n.children {
                    self.intern_tree(c);
                }
            }
            Item::List(items) => {
                for c in items {
                    self.intern_tree(c);
                }
            }
            Item::Null => {}
        }
    }

    /// `fxCoderAddSymbol` — emit a symbol-operand op, marking the symbol
    /// used so it earns an ID.
    fn add_symbol(&mut self, delta: i32, id: i32, name: &str) {
        let sym = self.symbols.use_symbol(name);
        self.add(delta, Payload::Symbol { sym }, id);
    }

    // ---- fxCoderAdd* constructors -----------------------------------

    fn add(&mut self, delta: i32, payload: Payload, id: i32) {
        self.stack_level += delta;
        let stack_level = self.stack_level;
        self.codes.push(Code { id, stack_level, payload });
    }

    fn add_byte(&mut self, delta: i32, id: i32) {
        self.add(delta, Payload::Byte, id);
    }

    fn add_index(&mut self, delta: i32, id: i32, index: i32) {
        self.add(delta, Payload::Index { index, plus_one: false }, id);
    }

    fn add_integer(&mut self, delta: i32, id: i32, value: i32) {
        self.add(delta, Payload::Integer { value }, id);
    }

    fn add_number(&mut self, delta: i32, id: i32, value: f64) {
        self.add(delta, Payload::Number { value }, id);
    }

    /// `fxCoderAddString`: XS stores `length + 1` (the trailing NUL) and
    /// copies that many bytes. `bytes` here already carries the NUL.
    fn add_string(&mut self, delta: i32, id: i32, bytes: Vec<u8>) {
        let len = bytes.len() as i32;
        self.add(delta, Payload::Str { bytes, len }, id);
    }

    /// `fxCoderAddBigInt`.
    fn add_bigint(&mut self, delta: i32, id: i32, bytes: Vec<u8>) {
        let measure = bytes.len() as i32;
        self.add(delta, Payload::BigInt { bytes, measure }, id);
    }

    /// `fxCoderCreateTarget` — a fresh target snapshotting the coder's
    /// current environment/scope/stack levels (break/continue resolution
    /// and the `try` finalizer read these back).
    fn create_target(&mut self) -> usize {
        let index = self.target_index;
        self.target_index += 1;
        self.targets.push(Target {
            index,
            environment_level: self.environment_level,
            scope_level: self.scope_level,
            stack_level: self.stack_level,
            ..Target::default()
        });
        self.targets.len() - 1
    }

    fn add_branch(&mut self, delta: i32, id: i32, tid: usize) {
        self.targets[tid].used = true;
        self.add(delta, Payload::Branch { tid }, id);
    }

    /// `fxCoderAdd(self, delta, target)` — place a created target into
    /// the record stream (`XS_NO_CODE`).
    fn place_target(&mut self, delta: i32, tid: usize) {
        self.add(delta, Payload::Target { tid }, XS_NO_CODE);
    }

    /// `fxCoderUseTemporaryVariable` — allocate the next frame slot and
    /// emit a bare `NEW_TEMPORARY` (a 1-byte opcode; its index is coder
    /// bookkeeping, not serialized). Returns the slot.
    fn use_temporary(&mut self) -> i32 {
        let result = self.scope_level;
        self.scope_level += 1;
        self.add_index(0, XS_CODE_NEW_TEMPORARY, result);
        result
    }

    /// `fxCoderUnuseTemporaryVariables`.
    fn unuse_temporaries(&mut self, count: i32) {
        self.add_index(0, XS_CODE_UNWIND_1, count);
        self.scope_level -= count;
    }

    /// `fxCoderAdjustEnvironment` — pop `with` environments down to a
    /// break/continue target's level.
    fn adjust_environment(&mut self, tid: usize) {
        let mut count = self.environment_level - self.targets[tid].environment_level;
        while count != 0 {
            self.add_byte(0, XS_CODE_WITHOUT);
            count -= 1;
        }
    }

    /// `fxCoderAdjustScope` — unwind frame slots down to a target's level.
    fn adjust_scope(&mut self, tid: usize) {
        let count = self.scope_level - self.targets[tid].scope_level;
        if count != 0 {
            self.add_index(0, XS_CODE_UNWIND_1, count);
        }
    }

    // ---- scope helpers ----------------------------------------------

    /// The primary scope XS hung off `node`.
    fn scope_of(&self, node: &Node) -> usize {
        self.tree.node_scopes.get(&node_key(node)).expect("scope for node").0
    }

    /// The secondary scope XS hung off `node` (`statementScope` /
    /// `symbolScope`) — e.g. a `catch(e)` binding's block scope, reached
    /// by the deferred catch-parameter path.
    #[allow(dead_code)]
    fn scope_secondary(&self, node: &Node) -> usize {
        self.tree
            .node_scopes
            .get(&node_key(node))
            .expect("scope for node")
            .1
            .expect("secondary scope for node")
    }

    fn declare_count(&self, scope: usize) -> i32 {
        self.tree.scopes[scope].declare_count
    }

    // ---- scope coding (declare-free surface) ------------------------
    //
    // The full `fxScopeCodingBlock` / `fxScopeCoded` / `fxScopeCodeRefresh`
    // / `fxScopeCodeDefineNodes` emit `NEW_LOCAL` / `NEW_CLOSURE` /
    // `VAR_LOCAL` clusters keyed on declaration symbols — that needs the
    // atom table (a later child). Here they are the exact no-ops XS runs
    // when a scope declares nothing, asserted so a declaring loop / block /
    // `switch` / `catch` fails loudly rather than emitting wrong bytes.

    /// `fxScopeCodingBlock` for a non-declaring scope (no-op).
    fn scope_coding_block(&mut self, scope: usize) {
        assert_eq!(
            self.declare_count(scope),
            0,
            "declaring scope reached in control-flow coder (later child)"
        );
    }

    /// `fxScopeCoded` for a non-declaring scope (no-op).
    fn scope_coded(&mut self, scope: usize) {
        assert_eq!(self.declare_count(scope), 0, "declaring scope reached (later child)");
    }

    /// `fxScopeCodeRefresh` for a non-declaring scope (no-op).
    fn scope_code_refresh(&mut self, scope: usize) {
        assert_eq!(self.declare_count(scope), 0, "declaring scope reached (later child)");
    }

    /// `fxScopeCodeDefineNodes` for a scope with no define nodes (no-op).
    fn scope_code_define_nodes(&mut self, scope: usize) {
        assert!(
            self.tree.scopes[scope].defines.is_empty(),
            "define nodes reached in control-flow coder (later child)"
        );
    }
}

/// The public entry: compile `source` as a Script to XS bytecode, or
/// return the first parser/scoper early error. The returned bytes are
/// the `codeBuffer` half of XS's `txScript` — exactly what
/// `endor_oracle::run(source).bytecode` returns.
pub fn compile(source: &str) -> Result<Vec<u8>, crate::parser::ParseError> {
    compile_with(source, false)
}

/// `compile`, choosing the Script strictness (a bare `"use strict"`
/// program is strict).
pub fn compile_with(source: &str, strict: bool) -> Result<Vec<u8>, crate::parser::ParseError> {
    let mut parser = crate::parser::Parser::new(source, strict, false)?;
    let root = parser.parse_program(strict)?;
    let tree = crate::scoper::run(&root)?;
    let mut coder = Coder::new(&tree);
    // Intern the program's symbols in lex order before coding so the atom
    // table's bucket lists match XS's.
    coder.intern_tree(&root);
    // The oracle shim compiles the *script* goal as an eval program
    // (`fxParseScript(..., mxProgramFlag | mxEvalFlag)`), so the program
    // header is coded through `fxScopeCodingEval`.
    coder.eval_flag = true;
    coder.code_program(node_of(&root));
    Ok(coder.serialize())
}

fn node_of(item: &Item) -> &Node {
    match item {
        Item::Node(n) => n.as_ref(),
        _ => panic!("expected node"),
    }
}

// ============================ node dispatch ============================

impl Coder<'_> {
    /// `fxNodeDispatchCode` for one child slot. A real node dispatches by
    /// kind; the other `Item` shapes never appear where an expression/
    /// statement is expected in the ported surface.
    fn code(&mut self, item: &Item) {
        match item {
            Item::Node(n) => self.code_node(n),
            Item::Null => {}
            other => panic!("unexpected item in coder: {:?}", other),
        }
    }

    fn code_node(&mut self, node: &Node) {
        use Token::*;
        match node.token {
            Program => self.code_program(node),
            Statements => self.code_statements(node),
            Statement => self.code_statement(node),
            Block => self.code_block(node),
            If => self.code_if(node),
            // value leaves (`fxValueNodeCode`: push the description code)
            True | False | Null | Undefined => {
                self.add_byte(1, value_code(node.token));
            }
            Integer => {
                let v = match node.value {
                    Value::Integer(v) => v,
                    _ => panic!("Integer node without integer value"),
                };
                self.add_integer(1, XS_CODE_INTEGER_1, v);
            }
            Number => {
                let v = match node.value {
                    Value::Number(v) => v,
                    _ => panic!("Number node without number value"),
                };
                self.add_number(1, XS_CODE_NUMBER, v);
            }
            String => {
                let s = match &node.value {
                    Value::Str(s) => s,
                    _ => panic!("String node without string value"),
                };
                let mut bytes = s.clone().into_bytes();
                bytes.push(0);
                self.add_string(1, XS_CODE_STRING_1, bytes);
            }
            Bigint => {
                let lit = match &node.value {
                    Value::BigInt(b) => b,
                    _ => panic!("BigInt node without bigint value"),
                };
                let bytes = bigint_limbs_le(&lit.digits, lit.radix as u32);
                self.add_bigint(1, XS_CODE_BIGINT_1, bytes);
            }
            // unary (`fxUnaryExpressionNodeCode`): operand then op, delta 0
            Void | Not | BitNot | Minus | Plus | Typeof => {
                self.code(&node.children[0]);
                self.add_byte(0, unary_code(node.token));
            }
            // binary (`fxBinaryExpressionNodeCode`): left, right, op, delta -1
            Add | Subtract | Multiply | Divide | Modulo | Exponentiation | BitAnd | BitOr
            | BitXor | LeftShift | SignedRightShift | UnsignedRightShift | Equal | NotEqual
            | StrictEqual | StrictNotEqual | Less | LessEqual | More | MoreEqual | Instanceof
            | In => {
                self.code(&node.children[0]);
                self.code(&node.children[1]);
                self.add_byte(-1, binary_code(node.token));
            }
            And => self.code_and(node),
            Or => self.code_or(node),
            Coalesce => self.code_coalesce(node),
            QuestionMark => self.code_question_mark(node),
            Expressions => self.code_expressions(node),
            // control flow (symbol-free surface)
            Label => self.code_label(node),
            While => self.code_while(node),
            Do => self.code_do(node),
            For => self.code_for(node),
            Break | Continue => self.code_break_continue(node),
            Throw => self.code_throw(node),
            Debugger => self.add_byte(0, XS_CODE_DEBUGGER),
            Switch => self.code_switch(node),
            Try => self.code_try(node),
            Catch => self.code_catch(node),
            // `this` (`fxThisNodeCode`): a derived-constructor `this` reads
            // the frame slot; otherwise the plain `THIS` opcode. Both are
            // symbol-free.
            This => {
                if node.flags & crate::ast::flags::DERIVED != 0 {
                    self.add_byte(1, XS_CODE_GET_THIS);
                } else {
                    self.add_byte(1, XS_CODE_THIS);
                }
            }
            Regexp => self.code_regexp(node),
            Template => self.code_template(node),
            Access => self.code_access(node),
            Member => self.code_member(node),
            MemberAt => self.code_member_at(node),
            Call => self.code_call(node),
            New => self.code_new(node),
            Params => self.code_params(node),
            Assign => self.code_assign_node(node),
            AddAssign | SubtractAssign | MultiplyAssign | DivideAssign | ModuloAssign
            | ExponentiationAssign | BitAndAssign | BitOrAssign | BitXorAssign
            | LeftShiftAssign | SignedRightShiftAssign | UnsignedRightShiftAssign
            | AndAssign | OrAssign | CoalesceAssign => self.code_compound(node),
            other => panic!("coder: unsupported node kind {:?}", other),
        }
    }

    /// `fxProgramNodeCode`.
    fn code_program(&mut self, node: &Node) {
        self.program_flag = true;
        if node.flags & crate::ast::flags::STRICT != 0 {
            self.add_index(0, XS_CODE_BEGIN_STRICT, 0);
        } else {
            self.add_index(0, XS_CODE_BEGIN_SLOPPY, 0);
        }
        // The oracle compiles with `mxEvalFlag`, so the header is the
        // eval shape (`fxScopeCodingEval`).
        self.code_scope_eval(node);
        // `coder->returnTarget` — the program's implicit return point.
        // It must live on the coder so a `try`/`return` inside the body
        // can alias it (the alias count feeds the `try` selector, so a
        // missing return target skews every `try` selection by one).
        let return_target = self.create_target();
        self.return_target = Some(return_target);
        // `fxScopeCodeDefineNodes` — no define nodes in the ported
        // surface (function/var defines are child 6).
        self.code(&node.children[0]);
        let rt = self.return_target.take().expect("program return target");
        self.place_target(0, rt);
        self.add_byte(0, XS_CODE_RETURN);
    }

    /// `fxScopeCodingEval` for the program scope. Only the branch the
    /// ported surface reaches is emitted: a sloppy eval program whose
    /// scope declares nothing but block-locals emits `EVAL_ENVIRONMENT`
    /// and, when `scopeCount > 0`, a `RESERVE_1`. `var`/function hoists
    /// (which prepend `RESERVE`/`NEW_LOCAL` clusters) are child 6.
    fn code_scope_eval(&mut self, node: &Node) {
        let scope = self.scope_of(node);
        let strict = self.tree.scopes[scope].flags & crate::ast::flags::STRICT != 0;
        let scope_count = *self.tree.scope_counts.get(&scope).unwrap_or(&0);
        if strict {
            if scope_count != 0 {
                // Strict eval reserves up front; block declares handled
                // by `fxScopeCodingBlock` (child 6 for declaring bodies).
                self.add_index(0, XS_CODE_RESERVE_1, scope_count);
            }
        } else {
            // Sloppy: no top-level `var`/define in the ported surface, so
            // the DEFINE/VAR prelude is empty.
            self.add_byte(0, XS_CODE_EVAL_ENVIRONMENT);
            self.scope_level = 0;
            if scope_count != 0 {
                self.add_index(0, XS_CODE_RESERVE_1, scope_count);
                // Declaring bodies (block lets reached at program scope)
                // are child 6; the ported corpus keeps scopeCount == 0.
            }
        }
    }

    /// `fxStatementsNodeCode`.
    fn code_statements(&mut self, node: &Node) {
        if let Some(Item::List(items)) = node.children.first() {
            for item in items {
                self.code(item);
            }
        }
    }

    /// `fxStatementNodeCode` (program-flag branch; the ported surface is
    /// always at program scope for now).
    fn code_statement(&mut self, node: &Node) {
        if self.program_flag {
            self.code(&node.children[0]);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        } else {
            // Non-program (function-body) statements pop their value;
            // the SET_LOCAL/SET_CLOSURE peephole is child 6.
            self.code(&node.children[0]);
            self.add_byte(-1, XS_CODE_POP);
        }
    }

    /// `fxBlockNodeCode`. In the ported surface a block declares nothing,
    /// so `fxScopeCodingBlock`/`fxScopeCoded` are no-ops guarded on the
    /// declare count; declaring blocks are child 6.
    fn code_block(&mut self, node: &Node) {
        let scope = self.scope_of(node);
        assert_eq!(
            self.declare_count(scope),
            0,
            "declaring block reached in expr/simple-statement coder (child 6)"
        );
        // `fxScopeCodeUsingStatement` with no disposables just dispatches
        // the block's statement.
        self.code(&node.children[0]);
    }

    /// `fxIfNodeCode` (program-flag branch: each arm sets the result to
    /// `undefined` first, per XS).
    fn code_if(&mut self, node: &Node) {
        self.code(&node.children[0]);
        if self.program_flag {
            let else_target = self.create_target();
            let end_target = self.create_target();
            self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, else_target);
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
            self.code(&node.children[1]);
            self.add_branch(0, XS_CODE_BRANCH_1, end_target);
            self.place_target(0, else_target);
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
            if !matches!(node.children[2], Item::Null) {
                self.code(&node.children[2]);
            }
            self.place_target(0, end_target);
        } else {
            let has_else = !matches!(node.children[2], Item::Null);
            if has_else {
                let else_target = self.create_target();
                let end_target = self.create_target();
                self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, else_target);
                self.code(&node.children[1]);
                self.add_branch(0, XS_CODE_BRANCH_1, end_target);
                self.place_target(0, else_target);
                self.code(&node.children[2]);
                self.place_target(0, end_target);
            } else {
                let end_target = self.create_target();
                self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, end_target);
                self.code(&node.children[1]);
                self.place_target(0, end_target);
            }
        }
    }

    /// `fxAndExpressionNodeCode`.
    fn code_and(&mut self, node: &Node) {
        let end_target = self.create_target();
        self.code(&node.children[0]);
        self.add_byte(1, XS_CODE_DUB);
        self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, end_target);
        self.add_byte(-1, XS_CODE_POP);
        self.code(&node.children[1]);
        self.place_target(0, end_target);
    }

    /// `fxOrExpressionNodeCode`.
    fn code_or(&mut self, node: &Node) {
        let end_target = self.create_target();
        self.code(&node.children[0]);
        self.add_byte(1, XS_CODE_DUB);
        self.add_branch(-1, XS_CODE_BRANCH_IF_1, end_target);
        self.add_byte(-1, XS_CODE_POP);
        self.code(&node.children[1]);
        self.place_target(0, end_target);
    }

    /// `fxCoalesceExpressionNodeCode`.
    fn code_coalesce(&mut self, node: &Node) {
        let end_target = self.create_target();
        self.code(&node.children[0]);
        self.add_branch(-1, XS_CODE_BRANCH_COALESCE_1, end_target);
        self.code(&node.children[1]);
        self.place_target(0, end_target);
    }

    /// `fxQuestionMarkNodeCode`.
    fn code_question_mark(&mut self, node: &Node) {
        let else_target = self.create_target();
        let end_target = self.create_target();
        self.code(&node.children[0]);
        self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, else_target);
        self.code(&node.children[1]);
        self.add_branch(0, XS_CODE_BRANCH_1, end_target);
        self.place_target(-1, else_target);
        self.code(&node.children[2]);
        self.place_target(0, end_target);
    }

    /// `fxExpressionsNodeCode` (sequence): each item but the first is
    /// preceded by a `POP` of the previous value.
    fn code_expressions(&mut self, node: &Node) {
        if let Some(Item::List(items)) = node.children.first() {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.add_byte(-1, XS_CODE_POP);
                }
                self.code(item);
            }
        }
    }

    // ------------------------- control flow --------------------------

    /// `fxLabelNodeCode`. A `Label` wraps its statement (loops carry an
    /// anonymous label). Nested labels are collapsed into one break /
    /// continue target answering to the whole symbol chain, exactly as XS
    /// folds `former->nextLabel = self`.
    fn code_label(&mut self, node: &Node) {
        // Descend the label chain to the wrapped statement, collecting the
        // label symbols. XS's collapsed `nextLabel` order is innermost
        // first, so we reverse the outermost-first descent.
        let mut labels: Vec<Option<String>> = Vec::new();
        let mut cur = node;
        loop {
            labels.push(match &cur.children[0] {
                Item::Symbol(s) => Some(s.clone()),
                _ => None,
            });
            match &cur.children[1] {
                Item::Node(n) if n.token == Token::Label => cur = n.as_ref(),
                _ => break,
            }
        }
        labels.reverse();
        // Dispatch the wrapped statement by reference — the scoper keys
        // scopes by node address, so a clone would miss its registration.
        let statement = &cur.children[1];
        // `self->symbol` after the collapse is the innermost label's.
        let inner_has_symbol = labels[0].is_some();

        let break_target = self.create_target();
        self.targets[break_target].labels = labels.clone();
        self.targets[break_target].next_target = self.first_break_target;
        self.first_break_target = Some(break_target);

        if inner_has_symbol {
            self.code(statement);
        } else {
            let continue_target = self.create_target();
            self.targets[continue_target].labels = labels;
            self.targets[continue_target].next_target = self.first_continue_target;
            self.first_continue_target = Some(continue_target);
            self.code(statement);
            self.first_continue_target = self.targets[continue_target].next_target;
        }
        self.place_target(0, break_target);
        self.first_break_target = self.targets[break_target].next_target;
    }

    /// `fxWhileNodeCode`. Children `[expression, statement]`; break /
    /// continue targets come from the enclosing `Label`.
    fn code_while(&mut self, node: &Node) {
        let cont = self.first_continue_target.expect("while continue target");
        let brk = self.first_break_target.expect("while break target");
        if self.program_flag {
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        }
        self.place_target(0, cont);
        self.code(&node.children[0]);
        self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, brk);
        self.code(&node.children[1]);
        self.add_branch(0, XS_CODE_BRANCH_1, cont);
    }

    /// `fxDoNodeCode`. Children `[statement, expression]`.
    fn code_do(&mut self, node: &Node) {
        let cont = self.first_continue_target.expect("do continue target");
        let loop_target = self.create_target();
        if self.program_flag {
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        }
        self.place_target(0, loop_target);
        self.code(&node.children[0]);
        self.place_target(0, cont);
        self.code(&node.children[1]);
        self.add_branch(-1, XS_CODE_BRANCH_IF_1, loop_target);
    }

    /// `fxForNodeCode` — the C-style loop. Children `[initialization,
    /// expression, iteration, statement]` (any of the first three may be
    /// `Null`).
    fn code_for(&mut self, node: &Node) {
        let scope = self.scope_of(node);
        // Detach the loop's own continue target from the stack for the
        // header, re-inserting it around the body (XS's swap).
        let continue_target = self.first_continue_target.expect("for continue target");
        self.first_continue_target = self.targets[continue_target].next_target;
        self.targets[continue_target].next_target = None;

        self.scope_coding_block(scope);
        self.scope_code_define_nodes(scope);
        let next_target = self.create_target();
        let done_target = self.create_target();
        if !matches!(node.children[0], Item::Null) {
            self.code(&node.children[0]);
        }
        if self.program_flag {
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        }
        self.scope_code_refresh(scope);
        self.place_target(0, next_target);
        if !matches!(node.children[1], Item::Null) {
            self.code(&node.children[1]);
            self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, done_target);
        }

        self.targets[continue_target].environment_level = self.environment_level;
        self.targets[continue_target].scope_level = self.scope_level;
        self.targets[continue_target].stack_level = self.stack_level;
        self.targets[continue_target].next_target = self.first_continue_target;
        self.first_continue_target = Some(continue_target);
        self.code(&node.children[3]);
        self.place_target(0, continue_target);
        self.first_continue_target = self.targets[continue_target].next_target;
        self.targets[continue_target].next_target = None;

        if !matches!(node.children[2], Item::Null) {
            self.scope_code_refresh(scope);
            self.code(&node.children[2]);
            self.add_byte(-1, XS_CODE_POP);
        }
        self.add_branch(0, XS_CODE_BRANCH_1, next_target);
        self.place_target(0, done_target);
        self.scope_coded(scope);

        self.targets[continue_target].next_target = self.first_continue_target;
        self.first_continue_target = Some(continue_target);
    }

    /// `fxBreakContinueNodeCode`. Child `[symbol-or-null]`.
    fn code_break_continue(&mut self, node: &Node) {
        let symbol = match node.children.first() {
            Some(Item::Symbol(s)) => Some(s.clone()),
            _ => None,
        };
        let is_break = node.token == Token::Break;
        let mut target = if is_break {
            self.first_break_target
        } else {
            self.first_continue_target
        };
        while let Some(t) = target {
            if self.targets[t].labels.iter().any(|l| *l == symbol) {
                self.adjust_environment(t);
                self.adjust_scope(t);
                self.add_branch(0, XS_CODE_BRANCH_1, t);
                return;
            }
            target = self.targets[t].next_target;
        }
        panic!("coder: invalid {}", if is_break { "break" } else { "continue" });
    }

    /// `fxThrowNodeCode`. Child `[expression]`.
    fn code_throw(&mut self, node: &Node) {
        self.code(&node.children[0]);
        self.add_byte(-1, XS_CODE_THROW);
    }

    /// The symbol name in an `Item::Symbol` child slot.
    fn symbol_of(item: &Item) -> &str {
        match item {
            Item::Symbol(s) => s.as_str(),
            _ => panic!("expected symbol slot"),
        }
    }

    /// `fxAccessNodeCode`. Child `[symbol]`. At program scope every
    /// identifier is a free (global) reference, so the coder takes the
    /// unresolved path: an `EVAL_REFERENCE` (the program is coded with the
    /// eval flag) then `GET_VARIABLE`. Resolved (local/closure) access
    /// needs the scoper's per-node declaration and arrives with the
    /// declaration slices.
    fn code_access(&mut self, node: &Node) {
        let name = Self::symbol_of(&node.children[0]).to_string();
        // fxAccessNodeCodeReference (unresolved, evalFlag branch)
        if self.eval_flag {
            self.add_symbol(1, XS_CODE_EVAL_REFERENCE, &name);
        } else {
            self.add_symbol(1, XS_CODE_PROGRAM_REFERENCE, &name);
        }
        self.add_symbol(0, XS_CODE_GET_VARIABLE, &name);
    }

    /// `fxMemberNodeCode`. Children `[reference, symbol]` → the reference
    /// then a `GET_PROPERTY` (or `GET_SUPER` for a `super.x` reference;
    /// `super` is deferred with classes).
    fn code_member(&mut self, node: &Node) {
        self.code(&node.children[0]);
        let is_super = self.node_is_super(&node.children[0]);
        let name = Self::symbol_of(&node.children[1]).to_string();
        let op = if is_super { XS_CODE_GET_SUPER } else { XS_CODE_GET_PROPERTY };
        self.add_symbol(0, op, &name);
    }

    /// Whether a reference child carries `mxSuperFlag` (a `super.x` base).
    fn node_is_super(&self, item: &Item) -> bool {
        matches!(item, Item::Node(n) if n.flags & crate::ast::flags::SUPER != 0)
    }

    /// `fxMemberAtNodeCode` — computed access `ref[at]`. Children
    /// `[reference, at]`. Symbol-free (`AT` + `GET_PROPERTY_AT`); the
    /// subexpressions carry any symbols.
    fn code_member_at(&mut self, node: &Node) {
        let is_super = self.node_is_super(&node.children[0]);
        self.code(&node.children[0]);
        self.code(&node.children[1]);
        self.add_byte(0, if is_super { XS_CODE_SUPER_AT } else { XS_CODE_AT });
        self.add_byte(-1, if is_super { XS_CODE_GET_SUPER_AT } else { XS_CODE_GET_PROPERTY_AT });
    }

    /// `fxCallNodeCode`. Children `[reference, params]`: set up the callee
    /// and its `this`, `CALL`, then the argument list + `RUN`.
    fn code_call(&mut self, node: &Node) {
        self.code_this(&node.children[0], 0);
        self.add_byte(1, XS_CODE_CALL);
        self.code(&node.children[1]);
    }

    /// `fxNewNodeCode`. Children `[reference, params]`: the constructor,
    /// `NEW`, then the argument list + `RUN`.
    fn code_new(&mut self, node: &Node) {
        self.code(&node.children[0]);
        self.add_byte(2, XS_CODE_NEW);
        self.code(&node.children[1]);
    }

    /// `fxParamsNodeCode`, the non-spread / non-eval branch. Children
    /// `[List(items)]`; each arg is pushed then a single `RUN_1 count`
    /// pops callee+this+args and leaves the result. Spread arguments and
    /// direct-`eval` parameter passing (the `EVAL` opcode) are deferred.
    fn code_params(&mut self, node: &Node) {
        assert!(
            node.flags & crate::ast::flags::SPREAD == 0,
            "spread arguments reached (later child)"
        );
        let items: &[Item] = match node.children.first() {
            Some(Item::List(v)) => v,
            _ => &[],
        };
        let mut c: i32 = 0;
        for item in items {
            self.code(item);
            c += 1;
        }
        self.add_integer(-2 - c, XS_CODE_RUN_1, c);
    }

    // ---- the `codeThis` family (callee + receiver setup) ------------

    /// `fxNodeDispatchCodeThis` — dispatch a callee reference in
    /// receiver-setup mode, returning the residual `flag`.
    fn code_this(&mut self, item: &Item, flag: i32) -> i32 {
        match item {
            Item::Node(n) => match n.token {
                Token::Access => self.code_access_this(n, flag),
                Token::Member => self.code_member_this(n, flag),
                Token::MemberAt => self.code_member_at_this(n, flag),
                Token::Expressions => self.code_expressions_this(n, flag),
                _ => self.code_node_this(item, flag),
            },
            _ => self.code_node_this(item, flag),
        }
    }

    /// `fxNodeCodeThis` — the fallback: push `undefined` as the receiver,
    /// then the value.
    fn code_node_this(&mut self, item: &Item, _flag: i32) -> i32 {
        self.add_byte(1, XS_CODE_UNDEFINED);
        self.code(item);
        1
    }

    /// `fxAccessNodeCodeThis` (unresolved/global path).
    fn code_access_this(&mut self, node: &Node, flag: i32) -> i32 {
        let name = Self::symbol_of(&node.children[0]).to_string();
        if flag == 0 {
            self.add_byte(1, XS_CODE_UNDEFINED);
        }
        // unresolved: reference then GET_THIS_VARIABLE
        if self.eval_flag {
            self.add_symbol(1, XS_CODE_EVAL_REFERENCE, &name);
        } else {
            self.add_symbol(1, XS_CODE_PROGRAM_REFERENCE, &name);
        }
        if flag != 0 {
            self.add_byte(1, XS_CODE_DUB);
        }
        self.add_symbol(0, XS_CODE_GET_THIS_VARIABLE, &name);
        flag
    }

    /// `fxMemberNodeCodeThis` — the object is the receiver (`DUB`'d).
    fn code_member_this(&mut self, node: &Node, _flag: i32) -> i32 {
        self.code(&node.children[0]);
        let is_super = self.node_is_super(&node.children[0]);
        let name = Self::symbol_of(&node.children[1]).to_string();
        self.add_byte(1, XS_CODE_DUB);
        self.add_symbol(0, if is_super { XS_CODE_GET_SUPER } else { XS_CODE_GET_PROPERTY }, &name);
        1
    }

    /// `fxMemberAtNodeCodeThis`.
    fn code_member_at_this(&mut self, node: &Node, flag: i32) -> i32 {
        let is_super = self.node_is_super(&node.children[0]);
        let mut flag = flag;
        if flag != 0 {
            // fxMemberAtNodeCodeReference(flag=0): reference, at, then AT.
            self.code(&node.children[0]);
            self.code(&node.children[1]);
            self.add_byte(0, if is_super { XS_CODE_SUPER_AT } else { XS_CODE_AT });
            self.add_byte(2, XS_CODE_DUB_AT);
            flag = 2;
        } else {
            self.code(&node.children[0]);
            self.add_byte(1, XS_CODE_DUB);
            self.code(&node.children[1]);
            self.add_byte(0, if is_super { XS_CODE_SUPER_AT } else { XS_CODE_AT });
        }
        self.add_byte(-1, if is_super { XS_CODE_GET_SUPER_AT } else { XS_CODE_GET_PROPERTY_AT });
        flag
    }

    /// `fxExpressionsNodeCodeThis` — a single-item sequence forwards to its
    /// item's `codeThis`; otherwise the fallback (`undefined` receiver +
    /// the sequence's value), dispatched on the original node so scope
    /// keying stays intact.
    fn code_expressions_this(&mut self, node: &Node, flag: i32) -> i32 {
        if let Some(Item::List(items)) = node.children.first() {
            if items.len() == 1 {
                return self.code_this(&items[0], flag);
            }
        }
        let _ = flag;
        self.add_byte(1, XS_CODE_UNDEFINED);
        self.code_node(node);
        1
    }

    // ---- assignment: the codeReference / codeAssign families --------

    /// `fxAssignNodeCode` — plain `=`. Children `[reference, value]`:
    /// prepare the reference, evaluate the value, store.
    fn code_assign_node(&mut self, node: &Node) {
        self.code_reference(&node.children[0], 1);
        self.code(&node.children[1]);
        self.code_assign(&node.children[0], 1);
    }

    /// `fxCompoundExpressionNodeCode` — `+=`, `-=`, … and the short-circuit
    /// `&&=` / `||=` / `??=`. Children `[reference, value]`.
    fn code_compound(&mut self, node: &Node) {
        use Token::*;
        let token = node.token;
        let shortcut = matches!(token, AndAssign | OrAssign | CoalesceAssign);
        let else_target = if shortcut { Some(self.create_target()) } else { None };
        let end_target = if shortcut { Some(self.create_target()) } else { None };
        let swap = self.code_this(&node.children[0], 1);
        match token {
            AndAssign => {
                self.add_byte(1, XS_CODE_DUB);
                self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, else_target.unwrap());
                self.add_byte(-1, XS_CODE_POP);
                self.code(&node.children[1]);
                self.code_compound_name(node);
            }
            CoalesceAssign => {
                self.add_branch(-1, XS_CODE_BRANCH_COALESCE_1, else_target.unwrap());
                self.code(&node.children[1]);
                self.code_compound_name(node);
            }
            OrAssign => {
                self.add_byte(1, XS_CODE_DUB);
                self.add_branch(-1, XS_CODE_BRANCH_IF_1, else_target.unwrap());
                self.add_byte(-1, XS_CODE_POP);
                self.code(&node.children[1]);
                self.code_compound_name(node);
            }
            _ => {
                self.code(&node.children[1]);
                self.add_byte(-1, compound_op(token));
            }
        }
        self.code_assign(&node.children[0], 0);
        if shortcut {
            self.add_branch(0, XS_CODE_BRANCH_1, end_target.unwrap());
            self.place_target(0, else_target.unwrap());
            let mut swap = swap;
            while swap > 0 {
                if node.flags & crate::ast::flags::EXPRESSION_NO_VALUE == 0 {
                    self.add_byte(0, XS_CODE_SWAP);
                }
                self.add_byte(-1, XS_CODE_POP);
                swap -= 1;
            }
            self.place_target(0, end_target.unwrap());
        }
    }

    /// `fxCompoundExpressionNodeCodeName` — name an anonymous function /
    /// class assigned to a plain identifier. Its trigger nodes (function /
    /// class values) are not in the ported surface, so it is a no-op here.
    fn code_compound_name(&mut self, node: &Node) {
        if let Item::Node(r) = &node.children[0] {
            if r.token == Token::Access && node_code_name(&node.children[1]) {
                let name = Self::symbol_of(&r.children[0]).to_string();
                self.add_symbol(0, XS_CODE_NAME, &name);
            }
        }
    }

    /// `fxNodeDispatchCodeReference` — prepare a store target.
    fn code_reference(&mut self, item: &Item, flag: i32) {
        match item {
            Item::Node(n) => match n.token {
                Token::Access => {
                    let name = Self::symbol_of(&n.children[0]).to_string();
                    if self.eval_flag {
                        self.add_symbol(1, XS_CODE_EVAL_REFERENCE, &name);
                    } else {
                        self.add_symbol(1, XS_CODE_PROGRAM_REFERENCE, &name);
                    }
                }
                Token::Member => {
                    // fxMemberNodeCodeReference: just the object.
                    self.code(&n.children[0]);
                }
                Token::MemberAt => {
                    let is_super = self.node_is_super(&n.children[0]);
                    self.code(&n.children[0]);
                    self.code(&n.children[1]);
                    if flag == 0 {
                        self.add_byte(0, if is_super { XS_CODE_SUPER_AT } else { XS_CODE_AT });
                    }
                }
                // fxNodeCodeReference: nothing.
                _ => {}
            },
            _ => {}
        }
    }

    /// `fxNodeDispatchCodeAssign` — store into the prepared reference.
    fn code_assign(&mut self, item: &Item, flag: i32) {
        match item {
            Item::Node(n) => match n.token {
                Token::Access => {
                    // Unresolved (global) → SET_VARIABLE; resolved store is
                    // the declaration slice.
                    let name = Self::symbol_of(&n.children[0]).to_string();
                    self.add_symbol(-1, XS_CODE_SET_VARIABLE, &name);
                }
                Token::Member => {
                    let is_super = self.node_is_super(&n.children[0]);
                    let name = Self::symbol_of(&n.children[1]).to_string();
                    self.add_symbol(-1, if is_super { XS_CODE_SET_SUPER } else { XS_CODE_SET_PROPERTY }, &name);
                }
                Token::MemberAt => {
                    let is_super = self.node_is_super(&n.children[0]);
                    if flag != 0 {
                        self.add_byte(0, if is_super { XS_CODE_SUPER_AT_2 } else { XS_CODE_AT_2 });
                    }
                    self.add_byte(-2, if is_super { XS_CODE_SET_SUPER_AT } else { XS_CODE_SET_PROPERTY_AT });
                }
                other => panic!("coder: no reference for assignment target {:?}", other),
            },
            _ => panic!("coder: no reference for assignment target"),
        }
    }

    /// `fxTemplateNodeCode`, untagged branch. Children `[reference,
    /// List(items)]`; the items alternate `TemplateMiddle` (a cooked +
    /// raw string pair) with substitution expressions. The tagged branch
    /// builds the raw/cooked cache via `GET_PROPERTY` symbols and is
    /// deferred to the atom-table slice.
    fn code_template(&mut self, node: &Node) {
        assert!(
            matches!(node.children[0], Item::Null),
            "tagged template reached in control-flow coder (later child)"
        );
        let items = match &node.children[1] {
            Item::List(v) => v,
            _ => panic!("template without items list"),
        };
        // The first item is always a `TemplateMiddle`; emit its cooked
        // string, then fold each following part in with `+`.
        self.code(&node_of(&items[0]).children[0]);
        for item in &items[1..] {
            let n = node_of(item);
            if n.token == Token::TemplateMiddle {
                self.code(&n.children[0]);
            } else {
                self.code(item);
                self.add_byte(1, XS_CODE_TO_STRING);
            }
            self.add_byte(-1, XS_CODE_ADD);
        }
    }

    /// `fxRegexpNodeCode`. A `new RegExp(pattern, flags)` via the
    /// dedicated `REGEXP` intrinsic (no `RegExp` symbol lookup). XS
    /// dispatches `self->modifier` (the pattern string) then `self->value`
    /// (the flags string); those land in child slots `[1, 0]` here (the
    /// slot order is the reverse of XS's field order, as the byte stream
    /// pins: pattern first, flags second).
    fn code_regexp(&mut self, node: &Node) {
        self.add_byte(1, XS_CODE_REGEXP);
        self.add_byte(2, XS_CODE_NEW);
        self.code(&node.children[1]);
        self.code(&node.children[0]);
        self.add_integer(-4, XS_CODE_RUN_1, 2);
    }

    /// `fxSwitchNodeCode`. Children `[expression, List(cases)]`; each
    /// `Case` is `[test-or-null, body-or-null]`.
    fn code_switch(&mut self, node: &Node) {
        let scope = self.scope_of(node);
        self.code(&node.children[0]);
        self.scope_coding_block(scope);
        let break_target = self.create_target();
        // XS gives the switch break target a zeroed (anonymous) label so a
        // bare `break;` matches it.
        self.targets[break_target].labels = vec![None];
        self.targets[break_target].next_target = self.first_break_target;
        self.first_break_target = Some(break_target);
        if self.program_flag {
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        }
        // Reference the case nodes in place (the scoper keys scopes by
        // node address, so cloning would miss registrations).
        let cases: Vec<&Node> = match &node.children[1] {
            Item::List(items) => items.iter().map(node_of).collect(),
            _ => Vec::new(),
        };
        let mut case_targets = Vec::with_capacity(cases.len());
        let mut default_target: Option<usize> = None;
        for case in &cases {
            let t = self.create_target();
            case_targets.push(t);
            if !matches!(case.children[0], Item::Null) {
                self.add_byte(1, XS_CODE_DUB);
                self.code(&case.children[0]);
                self.add_byte(-1, XS_CODE_STRICT_EQUAL);
                self.add_branch(-1, XS_CODE_BRANCH_IF_1, t);
            } else {
                default_target = Some(t);
            }
        }
        match default_target {
            Some(dt) => self.add_branch(0, XS_CODE_BRANCH_1, dt),
            None => self.add_branch(0, XS_CODE_BRANCH_1, break_target),
        }
        for (i, case) in cases.iter().enumerate() {
            self.place_target(0, case_targets[i]);
            if !matches!(case.children[1], Item::Null) {
                self.code(&case.children[1]);
            }
        }
        self.place_target(0, break_target);
        self.first_break_target = self.targets[break_target].next_target;
        self.scope_coded(scope);
        self.add_byte(-1, XS_CODE_POP);
    }

    /// `fxCatchNodeCode`. Children `[parameter-or-null, statements]`. The
    /// parameter-binding branch emits `NEW_LOCAL` (a symbol op) and is
    /// deferred to the atom-table child; the bare `catch {}` form is here.
    fn code_catch(&mut self, node: &Node) {
        assert!(
            matches!(node.children[0], Item::Null),
            "catch binding reached in control-flow coder (later child)"
        );
        let statement_scope = self.scope_of(node);
        self.scope_coding_block(statement_scope);
        self.scope_code_define_nodes(statement_scope);
        self.code(&node.children[1]);
        self.scope_coded(statement_scope);
    }

    /// `fxTryNodeCode`. Children `[tryBlock, catch-or-null, finally-or-null]`.
    fn code_try(&mut self, node: &Node) {
        let exception = self.use_temporary();
        let selector = self.use_temporary();
        let result = self.use_temporary();

        self.first_break_target = self.alias_targets(self.first_break_target);
        self.first_continue_target = self.alias_targets(self.first_continue_target);
        self.return_target = self.alias_targets(self.return_target);
        let mut catch_target = self.create_target();
        let normal_target = self.create_target();
        let finally_target = self.create_target();

        self.add_integer(1, XS_CODE_INTEGER_1, 0);
        self.add_index(0, XS_CODE_SET_LOCAL_1, selector);
        self.add_byte(-1, XS_CODE_POP);

        self.add_branch(0, XS_CODE_CATCH_1, catch_target);
        if self.program_flag {
            self.add_byte(1, XS_CODE_UNDEFINED);
            self.add_byte(-1, XS_CODE_SET_RESULT);
        }
        self.code(&node.children[0]);
        self.add_branch(0, XS_CODE_BRANCH_1, normal_target);
        if !matches!(node.children[1], Item::Null) {
            self.add_byte(0, XS_CODE_UNCATCH);
            self.place_target(0, catch_target);
            catch_target = self.create_target();
            self.add_branch(0, XS_CODE_CATCH_1, catch_target);
            if self.program_flag {
                self.add_byte(1, XS_CODE_UNDEFINED);
                self.add_byte(-1, XS_CODE_SET_RESULT);
            }
            self.code(&node.children[1]);
            self.add_branch(0, XS_CODE_BRANCH_1, normal_target);
        }

        let mut selection = 1;
        self.first_break_target =
            self.finalize_targets(self.first_break_target, selector, &mut selection, finally_target);
        self.first_continue_target = self.finalize_targets(
            self.first_continue_target,
            selector,
            &mut selection,
            finally_target,
        );
        self.return_target =
            self.finalize_targets(self.return_target, selector, &mut selection, finally_target);
        self.place_target(0, normal_target);
        self.add_integer(1, XS_CODE_INTEGER_1, selection);
        self.add_index(0, XS_CODE_SET_LOCAL_1, selector);
        self.add_byte(-1, XS_CODE_POP);
        self.place_target(0, finally_target);
        self.add_byte(0, XS_CODE_UNCATCH);
        self.place_target(0, catch_target);
        self.add_byte(1, XS_CODE_EXCEPTION);
        self.add_index(0, XS_CODE_SET_LOCAL_1, exception);
        self.add_byte(-1, XS_CODE_POP);
        if !matches!(node.children[2], Item::Null) {
            if self.program_flag {
                self.add_byte(1, XS_CODE_GET_RESULT);
                self.add_index(-1, XS_CODE_PULL_LOCAL_1, result);
                self.add_byte(1, XS_CODE_UNDEFINED);
                self.add_byte(-1, XS_CODE_SET_RESULT);
            }
            self.code(&node.children[2]);
            if self.program_flag {
                self.add_index(1, XS_CODE_GET_LOCAL_1, result);
                self.add_byte(-1, XS_CODE_SET_RESULT);
            }
        }
        let end_catch = self.create_target();
        self.add_index(1, XS_CODE_GET_LOCAL_1, selector);
        self.add_branch(-1, XS_CODE_BRANCH_IF_1, end_catch);
        self.add_index(1, XS_CODE_GET_LOCAL_1, exception);
        self.add_byte(-1, XS_CODE_THROW);
        self.place_target(0, end_catch);
        let mut selection = 1;
        let bt = self.first_break_target;
        self.jump_targets(bt, selector, &mut selection);
        let ct = self.first_continue_target;
        self.jump_targets(ct, selector, &mut selection);
        let rt = self.return_target;
        self.jump_targets(rt, selector, &mut selection);
        self.unuse_temporaries(3);
    }

    /// `fxCoderAliasTargets` — parallel alias chain for the `try` unwind.
    fn alias_targets(&mut self, head: Option<usize>) -> Option<usize> {
        let mut result = None;
        let mut prev: Option<usize> = None;
        let mut cur = head;
        while let Some(t) = cur {
            let a = self.create_target();
            self.targets[a].labels = self.targets[t].labels.clone();
            self.targets[a].original = Some(t);
            if prev.is_none() {
                result = Some(a);
            } else {
                self.targets[prev.unwrap()].next_target = Some(a);
            }
            prev = Some(a);
            cur = self.targets[t].next_target;
        }
        result
    }

    /// `fxCoderFinalizeTargets`.
    fn finalize_targets(
        &mut self,
        alias: Option<usize>,
        selector: i32,
        selection: &mut i32,
        finally_target: usize,
    ) -> Option<usize> {
        let mut result = None;
        if let Some(first) = alias {
            result = self.targets[first].original;
            let mut a = alias;
            while let Some(al) = a {
                if self.targets[al].used {
                    self.place_target(0, al);
                    self.add_integer(1, XS_CODE_INTEGER_1, *selection);
                    self.add_index(-1, XS_CODE_PULL_LOCAL_1, selector);
                    self.add_branch(0, XS_CODE_BRANCH_1, finally_target);
                    let orig = self.targets[al].original.unwrap();
                    self.targets[orig].used = true;
                }
                a = self.targets[al].next_target;
                *selection += 1;
            }
        }
        result
    }

    /// `fxCoderJumpTargets`.
    fn jump_targets(&mut self, target: Option<usize>, selector: i32, selection: &mut i32) {
        let mut t = target;
        while let Some(tt) = t {
            if self.targets[tt].used {
                let else_target = self.create_target();
                self.add_integer(1, XS_CODE_INTEGER_1, *selection);
                self.add_index(1, XS_CODE_GET_LOCAL_1, selector);
                self.add_byte(-1, XS_CODE_STRICT_EQUAL);
                self.add_branch(-1, XS_CODE_BRANCH_ELSE_1, else_target);
                self.adjust_environment(tt);
                self.adjust_scope(tt);
                self.add_branch(0, XS_CODE_BRANCH_1, tt);
                self.place_target(0, else_target);
            }
            t = self.targets[tt].next_target;
            *selection += 1;
        }
    }
}

// ======================= three-pass serializer =========================

impl Coder<'_> {
    /// `fxCoderOptimize` — the four peephole rewrites XS runs before
    /// sizing. Only the two that can fire in the ported surface are
    /// implemented as observable rewrites; the END-folding pair needs the
    /// function-frame `END*` opcodes (child 6) and is a no-op here.
    fn optimize(&mut self) {
        // "branch to next =>": a `BRANCH_1` whose target is placed as the
        // immediately following record is dropped.
        let mut i = 0;
        while i < self.codes.len() {
            if self.codes[i].id == XS_CODE_BRANCH_1 {
                if let Payload::Branch { tid } = self.codes[i].payload {
                    // Is the next record the placed target for `tid`?
                    if let Some(next) = self.codes.get(i + 1) {
                        if let Payload::Target { tid: ntid } = next.payload {
                            if ntid == tid {
                                self.codes.remove(i);
                                continue;
                            }
                        }
                    }
                }
            }
            i += 1;
        }
    }

    /// `fxParserCode`'s three passes over the record list, producing the
    /// `codeBuffer` bytes.
    fn serialize(&mut self) -> Vec<u8> {
        self.optimize();

        // ---- pass 1: size with branches assumed widest, accrue delta --
        let mut size: i32 = 0;
        let mut delta: i32 = 0;
        for c in &mut self.codes {
            match c.id {
                XS_NO_CODE => {
                    if let Payload::Target { tid } = c.payload {
                        // set later; record offset now
                        let _ = tid;
                    }
                }
                _ => {}
            }
            size1_step(c, &mut size, &mut delta, &mut self.targets);
        }

        // ---- pass 2: choose branch widths from target offsets ---------
        size = 0;
        for c in &mut self.codes {
            size2_step(c, &mut size, &mut delta, &mut self.targets);
        }

        // Assign symbol IDs from the now-complete usage marks (XS does the
        // bucket walk here, between sizing and emission).
        self.symbols.assign_ids();
        let sym_ids = self.symbols.id_table();

        // ---- pass 3: emit ---------------------------------------------
        let mut out: Vec<u8> = Vec::with_capacity(size.max(0) as usize);
        for c in &self.codes {
            emit_step(c, &mut out, &self.targets, &sym_ids);
        }
        out
    }
}

/// Pass 1 of `fxParserCode`: accumulate `size` assuming every branch is
/// the widest form (its slack tracked in `delta`), select the width of
/// value-bearing opcodes (`INTEGER`/`STRING`/index families) from their
/// payloads, and record each target's provisional offset.
fn size1_step(c: &mut Code, size: &mut i32, delta: &mut i32, targets: &mut [Target]) {
    match c.id {
        XS_NO_CODE => {
            if let Payload::Target { tid } = c.payload {
                targets[tid].offset = *size;
            }
        }
        // branch family (`_1` forms): widest-assumption size 2, delta 3
        XS_CODE_BRANCH_1 | XS_CODE_BRANCH_CHAIN_1 | XS_CODE_BRANCH_COALESCE_1
        | XS_CODE_BRANCH_ELSE_1 | XS_CODE_BRANCH_IF_1 | XS_CODE_BRANCH_STATUS_1
        | XS_CODE_CATCH_1 | XS_CODE_CODE_1 => {
            *size += 2;
            *delta += 3;
        }
        // 2-byte fixed (`BEGIN_*`, `ARGUMENT(S)*`, `MODULE`)
        XS_CODE_ARGUMENT | XS_CODE_ARGUMENTS | XS_CODE_ARGUMENTS_SLOPPY
        | XS_CODE_ARGUMENTS_STRICT | XS_CODE_BEGIN_SLOPPY | XS_CODE_BEGIN_STRICT
        | XS_CODE_BEGIN_STRICT_BASE | XS_CODE_BEGIN_STRICT_DERIVED
        | XS_CODE_BEGIN_STRICT_FIELD | XS_CODE_MODULE => {
            *size += 2;
        }
        XS_CODE_LINE => *size += 3,
        // string: string bytes then width-select on length
        XS_CODE_STRING_1 => {
            if let Payload::Str { len, .. } = &c.payload {
                *size += *len;
            }
            width_select_index_family(c, size);
        }
        XS_CODE_RESERVE_1 | XS_CODE_RETRIEVE_1 | XS_CODE_UNWIND_1 => {
            width_select_index_family(c, size);
        }
        XS_CODE_INTEGER_1 | XS_CODE_RUN_1 | XS_CODE_RUN_TAIL_1 => {
            width_select_integer_family(c, size);
        }
        XS_CODE_NUMBER => *size += 9,
        // bigint: width-select on the measure, then the limb bytes
        XS_CODE_BIGINT_1 => {
            let measure = match &c.payload {
                Payload::BigInt { measure, .. } => *measure,
                _ => 0,
            };
            if measure > 255 {
                c.id += 1;
                *size += 3;
            } else {
                *size += 2;
            }
            *size += measure;
        }
        XS_CODE_HOST => *size += 3,
        _ => {
            if is_symbol_op(c.id) {
                *size += 1 + ID_SIZE;
            } else if is_index_plus_one_1(c.id) {
                width_select_index_plus_one_family(c, size);
            } else {
                *size += 1;
            }
        }
    }
}

/// The `INTEGER`/`STRING`/`RESERVE` width selector when the width key is
/// the raw index/length (`value > 255` / `> 65535`).
fn width_select_index_family(c: &mut Code, size: &mut i32) {
    let value = match &c.payload {
        Payload::Index { index, .. } => *index,
        Payload::Str { len, .. } => *len,
        _ => 0,
    };
    if value > 65535 {
        c.id += 2;
        *size += 5;
    } else if value > 255 {
        c.id += 1;
        *size += 3;
    } else {
        *size += 2;
    }
}

/// The local/closure family (`GET_LOCAL_1`…) width selector: the key is
/// `index + 1`.
fn width_select_index_plus_one_family(c: &mut Code, size: &mut i32) {
    let value = match &c.payload {
        Payload::Index { index, .. } => *index + 1,
        _ => 0,
    };
    if value > 65535 {
        c.id += 2;
        *size += 5;
    } else if value > 255 {
        c.id += 1;
        *size += 3;
    } else {
        *size += 2;
    }
}

/// The signed-integer family (`INTEGER_1`/`RUN_1`) width selector.
fn width_select_integer_family(c: &mut Code, size: &mut i32) {
    let value = match &c.payload {
        Payload::Integer { value } => *value,
        _ => 0,
    };
    if value < -32768 || value > 32767 {
        c.id += 2;
        *size += 5;
    } else if value < -128 || value > 127 {
        c.id += 1;
        *size += 3;
    } else {
        *size += 2;
    }
}

/// Pass 2: with pass-1 target offsets known, choose each branch's real
/// width (narrowing `size` and drawing down `delta`), and re-record
/// target offsets against the narrowed `size`.
fn size2_step(c: &mut Code, size: &mut i32, delta: &mut i32, targets: &mut [Target]) {
    match c.id {
        XS_NO_CODE => {
            if let Payload::Target { tid } = c.payload {
                targets[tid].offset = *size;
            }
        }
        XS_CODE_BRANCH_1 | XS_CODE_BRANCH_CHAIN_1 | XS_CODE_BRANCH_COALESCE_1
        | XS_CODE_BRANCH_ELSE_1 | XS_CODE_BRANCH_IF_1 | XS_CODE_BRANCH_STATUS_1
        | XS_CODE_CATCH_1 | XS_CODE_CODE_1 => {
            let tid = match c.payload {
                Payload::Branch { tid } => tid,
                _ => unreachable!(),
            };
            let offset = targets[tid].offset - (*size + 5);
            if offset < -32768 || offset + *delta > 32767 {
                c.id += 2;
                *size += 5;
            } else if offset < -128 || offset + *delta > 127 {
                c.id += 1;
                *delta -= 2;
                *size += 3;
            } else {
                *delta -= 3;
                *size += 2;
            }
        }
        XS_CODE_ARGUMENT | XS_CODE_ARGUMENTS | XS_CODE_ARGUMENTS_SLOPPY
        | XS_CODE_ARGUMENTS_STRICT | XS_CODE_BEGIN_SLOPPY | XS_CODE_BEGIN_STRICT
        | XS_CODE_BEGIN_STRICT_BASE | XS_CODE_BEGIN_STRICT_DERIVED
        | XS_CODE_BEGIN_STRICT_FIELD | XS_CODE_MODULE => {
            *size += 2;
        }
        XS_CODE_LINE => *size += 3,
        XS_CODE_INTEGER_1 | XS_CODE_RUN_1 | XS_CODE_RUN_TAIL_1 => *size += 2,
        XS_CODE_INTEGER_2 | XS_CODE_RUN_2 | XS_CODE_RUN_TAIL_2 => *size += 3,
        XS_CODE_INTEGER_4 | XS_CODE_RUN_4 | XS_CODE_RUN_TAIL_4 => *size += 5,
        XS_CODE_NUMBER => *size += 9,
        XS_CODE_STRING_1 => {
            if let Payload::Str { len, .. } = &c.payload {
                *size += 2 + *len;
            }
        }
        XS_CODE_STRING_2 => {
            if let Payload::Str { len, .. } = &c.payload {
                *size += 3 + *len;
            }
        }
        XS_CODE_STRING_4 => {
            if let Payload::Str { len, .. } = &c.payload {
                *size += 5 + *len;
            }
        }
        XS_CODE_BIGINT_1 => {
            if let Payload::BigInt { measure, .. } = &c.payload {
                *size += 2 + *measure;
            }
        }
        XS_CODE_BIGINT_2 => {
            if let Payload::BigInt { measure, .. } = &c.payload {
                *size += 3 + *measure;
            }
        }
        XS_CODE_HOST => *size += 3,
        _ => {
            if is_symbol_op(c.id) {
                *size += 1 + ID_SIZE;
            } else if is_index_1_fixed(c.id) {
                *size += 2;
            } else if is_index_2_fixed(c.id) {
                *size += 3;
            } else {
                *size += 1;
            }
        }
    }
}

/// Pass 3: emit the opcode byte and its operand with the chosen width.
fn emit_step(c: &Code, out: &mut Vec<u8>, targets: &[Target], sym_ids: &[i32]) {
    if c.id != XS_NO_CODE {
        out.push(c.id as u8);
    }
    match c.id {
        XS_NO_CODE => {}
        // branch _1/_2/_4: displacement from just past the operand
        XS_CODE_BRANCH_1 | XS_CODE_BRANCH_CHAIN_1 | XS_CODE_BRANCH_COALESCE_1
        | XS_CODE_BRANCH_ELSE_1 | XS_CODE_BRANCH_IF_1 | XS_CODE_BRANCH_STATUS_1
        | XS_CODE_CATCH_1 | XS_CODE_CODE_1 => {
            let tid = branch_tid(c);
            let offset = targets[tid].offset - (out.len() as i32 + 1);
            out.push(offset as i8 as u8);
        }
        XS_CODE_BRANCH_2 | XS_CODE_BRANCH_CHAIN_2 | XS_CODE_BRANCH_COALESCE_2
        | XS_CODE_BRANCH_ELSE_2 | XS_CODE_BRANCH_IF_2 | XS_CODE_BRANCH_STATUS_2
        | XS_CODE_CATCH_2 | XS_CODE_CODE_2 => {
            let tid = branch_tid(c);
            let offset = targets[tid].offset - (out.len() as i32 + 2);
            out.extend_from_slice(&(offset as i16).to_le_bytes());
        }
        XS_CODE_BRANCH_4 | XS_CODE_BRANCH_CHAIN_4 | XS_CODE_BRANCH_COALESCE_4
        | XS_CODE_BRANCH_ELSE_4 | XS_CODE_BRANCH_IF_4 | XS_CODE_BRANCH_STATUS_4
        | XS_CODE_CATCH_4 | XS_CODE_CODE_4 => {
            let tid = branch_tid(c);
            let offset = targets[tid].offset - (out.len() as i32 + 4);
            out.extend_from_slice(&offset.to_le_bytes());
        }
        // 2-byte fixed u1 (BEGIN_*, ARGUMENT(S), MODULE, RESERVE/RETRIEVE/UNWIND_1)
        XS_CODE_ARGUMENT | XS_CODE_ARGUMENTS | XS_CODE_ARGUMENTS_SLOPPY
        | XS_CODE_ARGUMENTS_STRICT | XS_CODE_BEGIN_SLOPPY | XS_CODE_BEGIN_STRICT
        | XS_CODE_BEGIN_STRICT_BASE | XS_CODE_BEGIN_STRICT_DERIVED
        | XS_CODE_BEGIN_STRICT_FIELD | XS_CODE_MODULE | XS_CODE_RESERVE_1
        | XS_CODE_RETRIEVE_1 | XS_CODE_UNWIND_1 => {
            out.push(index_value(c) as u8);
        }
        XS_CODE_LINE | XS_CODE_RESERVE_2 | XS_CODE_RETRIEVE_2 | XS_CODE_UNWIND_2 => {
            out.extend_from_slice(&(index_value(c) as u16).to_le_bytes());
        }
        XS_CODE_INTEGER_1 | XS_CODE_RUN_1 | XS_CODE_RUN_TAIL_1 => {
            out.push(integer_value(c) as i8 as u8);
        }
        XS_CODE_INTEGER_2 | XS_CODE_RUN_2 | XS_CODE_RUN_TAIL_2 => {
            out.extend_from_slice(&(integer_value(c) as i16).to_le_bytes());
        }
        XS_CODE_INTEGER_4 | XS_CODE_RUN_4 | XS_CODE_RUN_TAIL_4 => {
            out.extend_from_slice(&integer_value(c).to_le_bytes());
        }
        XS_CODE_NUMBER => {
            if let Payload::Number { value } = &c.payload {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        XS_CODE_STRING_1 => {
            if let Payload::Str { bytes, len } = &c.payload {
                out.push(*len as u8);
                out.extend_from_slice(bytes);
            }
        }
        XS_CODE_STRING_2 => {
            if let Payload::Str { bytes, len } = &c.payload {
                out.extend_from_slice(&(*len as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
        XS_CODE_STRING_4 => {
            if let Payload::Str { bytes, len } = &c.payload {
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
        XS_CODE_BIGINT_1 => {
            if let Payload::BigInt { bytes, measure } = &c.payload {
                out.push(*measure as u8);
                out.extend_from_slice(bytes);
            }
        }
        XS_CODE_BIGINT_2 => {
            if let Payload::BigInt { bytes, measure } = &c.payload {
                out.extend_from_slice(&(*measure as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
        XS_CODE_HOST => {
            out.extend_from_slice(&(index_value(c) as u16).to_le_bytes());
        }
        _ => {
            if is_symbol_op(c.id) {
                out.extend_from_slice(&(symbol_id(c, sym_ids) as u16).to_le_bytes());
            } else if is_index_1_fixed(c.id) {
                out.push((index_value(c) + 1) as u8);
            } else if is_index_2_fixed(c.id) {
                out.extend_from_slice(&((index_value(c) + 1) as u16).to_le_bytes());
            }
            // else: a plain 1-byte opcode, already pushed
        }
    }
}

// --------------------------- payload readers ---------------------------

fn branch_tid(c: &Code) -> usize {
    match c.payload {
        Payload::Branch { tid } => tid,
        _ => 0,
    }
}
fn index_value(c: &Code) -> i32 {
    match &c.payload {
        Payload::Index { index, .. } => *index,
        _ => 0,
    }
}
fn integer_value(c: &Code) -> i32 {
    match &c.payload {
        Payload::Integer { value } => *value,
        _ => 0,
    }
}
fn symbol_id(c: &Code, sym_ids: &[i32]) -> i32 {
    match &c.payload {
        Payload::Symbol { sym } => sym_ids[*sym],
        _ => 0,
    }
}

/// `sizeof(txID)` at the oracle pin (`mx32bitID` undefined → 2 bytes),
/// matching `endor_vm::opcode::ID_SIZE`.
const ID_SIZE: i32 = 2;

/// XS stores a BigInt literal as `bigint->data`: an array of `txU4` limbs
/// in machine (little-endian) byte order, `bigint->size` of them, trimmed
/// of high zero limbs but never below one limb (so `0n` is one zero
/// limb). `fxBigIntEncode` memcpys those limb bytes; `fxBigIntMeasure` is
/// `size * 4`. The parse path (decimal `fxBigIntParse`, or the shift-based
/// hex/octal/binary parsers) all converge on the same canonical limbs, so
/// a radix-accumulate reproduces `data` exactly. Returns the limb bytes.
fn bigint_limbs_le(digits: &str, radix: u32) -> Vec<u8> {
    let mut limbs: Vec<u32> = vec![0];
    for ch in digits.chars() {
        let d = ch.to_digit(radix).expect("bigint digit in radix") as u64;
        // limbs = limbs * radix + d  (base 2^32, little-endian)
        let mut carry = d;
        for limb in limbs.iter_mut() {
            let v = (*limb as u64) * (radix as u64) + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        while carry > 0 {
            limbs.push(carry as u32);
            carry >>= 32;
        }
    }
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 {
        limbs.pop();
    }
    let mut bytes = Vec::with_capacity(limbs.len() * 4);
    for l in limbs {
        bytes.extend_from_slice(&l.to_le_bytes());
    }
    bytes
}

// --------------------------- opcode classes ----------------------------

/// The symbol-operand opcodes (`1 + sizeof(txID)` bytes): the
/// `GET_VARIABLE`/`SET_PROPERTY`/`NEW_LOCAL`… set XS lists together.
fn is_symbol_op(id: i32) -> bool {
    matches!(
        id,
        XS_CODE_ASYNC_FUNCTION
            | XS_CODE_ASYNC_GENERATOR_FUNCTION
            | XS_CODE_CONSTRUCTOR_FUNCTION
            | XS_CODE_DELETE_PROPERTY
            | XS_CODE_DELETE_SUPER
            | XS_CODE_FILE
            | XS_CODE_FUNCTION
            | XS_CODE_GENERATOR_FUNCTION
            | XS_CODE_GET_PROPERTY
            | XS_CODE_GET_SUPER
            | XS_CODE_GET_THIS_VARIABLE
            | XS_CODE_GET_VARIABLE
            | XS_CODE_EVAL_PRIVATE
            | XS_CODE_EVAL_REFERENCE
            | XS_CODE_NAME
            | XS_CODE_NEW_CLOSURE
            | XS_CODE_NEW_LOCAL
            | XS_CODE_NEW_PROPERTY
            | XS_CODE_PROGRAM_REFERENCE
            | XS_CODE_SET_PROPERTY
            | XS_CODE_SET_SUPER
            | XS_CODE_SET_VARIABLE
            | XS_CODE_SYMBOL
            | XS_CODE_PROFILE
    )
}

/// The `_1` local/closure/private/store family whose serialized value is
/// `index + 1` and whose width is selected on that.
fn is_index_plus_one_1(id: i32) -> bool {
    matches!(
        id,
        XS_CODE_CONST_CLOSURE_1
            | XS_CODE_CONST_LOCAL_1
            | XS_CODE_GET_CLOSURE_1
            | XS_CODE_GET_LOCAL_1
            | XS_CODE_GET_PRIVATE_1
            | XS_CODE_HAS_PRIVATE_1
            | XS_CODE_LET_CLOSURE_1
            | XS_CODE_LET_LOCAL_1
            | XS_CODE_NEW_PRIVATE_1
            | XS_CODE_PULL_CLOSURE_1
            | XS_CODE_PULL_LOCAL_1
            | XS_CODE_REFRESH_CLOSURE_1
            | XS_CODE_REFRESH_LOCAL_1
            | XS_CODE_RESET_CLOSURE_1
            | XS_CODE_RESET_LOCAL_1
            | XS_CODE_SET_CLOSURE_1
            | XS_CODE_SET_LOCAL_1
            | XS_CODE_SET_PRIVATE_1
            | XS_CODE_STORE_1
            | XS_CODE_USED_1
            | XS_CODE_VAR_CLOSURE_1
            | XS_CODE_VAR_LOCAL_1
    )
}

/// Pass-2 fixed 2-byte forms: the `_1` local/closure family (already
/// width-selected in pass 1) plus `RESERVE_1`/`RETRIEVE_1`/`UNWIND_1`.
fn is_index_1_fixed(id: i32) -> bool {
    is_index_plus_one_1(id)
        || matches!(id, XS_CODE_RESERVE_1 | XS_CODE_RETRIEVE_1 | XS_CODE_UNWIND_1)
}

/// Pass-2 fixed 3-byte forms: the `_2` local/closure family plus
/// `RESERVE_2`/`RETRIEVE_2`/`UNWIND_2`.
fn is_index_2_fixed(id: i32) -> bool {
    matches!(
        id,
        XS_CODE_CONST_CLOSURE_2
            | XS_CODE_CONST_LOCAL_2
            | XS_CODE_GET_CLOSURE_2
            | XS_CODE_GET_LOCAL_2
            | XS_CODE_GET_PRIVATE_2
            | XS_CODE_HAS_PRIVATE_2
            | XS_CODE_LET_CLOSURE_2
            | XS_CODE_LET_LOCAL_2
            | XS_CODE_NEW_PRIVATE_2
            | XS_CODE_PULL_CLOSURE_2
            | XS_CODE_PULL_LOCAL_2
            | XS_CODE_REFRESH_CLOSURE_2
            | XS_CODE_REFRESH_LOCAL_2
            | XS_CODE_RESET_CLOSURE_2
            | XS_CODE_RESET_LOCAL_2
            | XS_CODE_SET_CLOSURE_2
            | XS_CODE_SET_LOCAL_2
            | XS_CODE_SET_PRIVATE_2
            | XS_CODE_STORE_2
            | XS_CODE_USED_2
            | XS_CODE_VAR_CLOSURE_2
            | XS_CODE_VAR_LOCAL_2
            | XS_CODE_RESERVE_2
            | XS_CODE_RETRIEVE_2
            | XS_CODE_UNWIND_2
    )
}

// --------------------------- token → opcode ----------------------------

/// `description->code` for the value leaves (`fxValueNodeCode`).
fn value_code(token: Token) -> i32 {
    match token {
        Token::True => XS_CODE_TRUE,
        Token::False => XS_CODE_FALSE,
        Token::Null => XS_CODE_NULL,
        Token::Undefined => XS_CODE_UNDEFINED,
        _ => unreachable!("not a value leaf: {:?}", token),
    }
}

/// `description->code` for the unary operators (`fxUnaryExpressionNodeCode`).
fn unary_code(token: Token) -> i32 {
    match token {
        Token::Void => XS_CODE_VOID,
        Token::Not => XS_CODE_NOT,
        Token::BitNot => XS_CODE_BIT_NOT,
        Token::Minus => XS_CODE_MINUS,
        Token::Plus => XS_CODE_PLUS,
        Token::Typeof => XS_CODE_TYPEOF,
        _ => unreachable!("not a unary op: {:?}", token),
    }
}

/// `fxNodeCodeName` — whether an assigned value is an anonymous
/// function / generator / class that should receive an inferred `.name`.
/// The trigger node kinds are not in the ported surface, so this is
/// always `false` for now (a named edge for the function/class slice).
fn node_code_name(_value: &Item) -> bool {
    false
}

/// The arithmetic opcode a compound assignment folds with
/// (`description->code` for the `*Assign` tokens).
fn compound_op(token: Token) -> i32 {
    match token {
        Token::AddAssign => XS_CODE_ADD,
        Token::SubtractAssign => XS_CODE_SUBTRACT,
        Token::MultiplyAssign => XS_CODE_MULTIPLY,
        Token::DivideAssign => XS_CODE_DIVIDE,
        Token::ModuloAssign => XS_CODE_MODULO,
        Token::ExponentiationAssign => XS_CODE_EXPONENTIATION,
        Token::BitAndAssign => XS_CODE_BIT_AND,
        Token::BitOrAssign => XS_CODE_BIT_OR,
        Token::BitXorAssign => XS_CODE_BIT_XOR,
        Token::LeftShiftAssign => XS_CODE_LEFT_SHIFT,
        Token::SignedRightShiftAssign => XS_CODE_SIGNED_RIGHT_SHIFT,
        Token::UnsignedRightShiftAssign => XS_CODE_UNSIGNED_RIGHT_SHIFT,
        _ => unreachable!("not a compound-assign op: {:?}", token),
    }
}

/// `description->code` for the binary operators (`fxBinaryExpressionNodeCode`),
/// exactly `gxTokenDescriptions`' `code` column.
fn binary_code(token: Token) -> i32 {
    match token {
        Token::Add => XS_CODE_ADD,
        Token::Subtract => XS_CODE_SUBTRACT,
        Token::Multiply => XS_CODE_MULTIPLY,
        Token::Divide => XS_CODE_DIVIDE,
        Token::Modulo => XS_CODE_MODULO,
        Token::Exponentiation => XS_CODE_EXPONENTIATION,
        Token::BitAnd => XS_CODE_BIT_AND,
        Token::BitOr => XS_CODE_BIT_OR,
        Token::BitXor => XS_CODE_BIT_XOR,
        Token::LeftShift => XS_CODE_LEFT_SHIFT,
        Token::SignedRightShift => XS_CODE_SIGNED_RIGHT_SHIFT,
        Token::UnsignedRightShift => XS_CODE_UNSIGNED_RIGHT_SHIFT,
        Token::Equal => XS_CODE_EQUAL,
        Token::NotEqual => XS_CODE_NOT_EQUAL,
        Token::StrictEqual => XS_CODE_STRICT_EQUAL,
        Token::StrictNotEqual => XS_CODE_STRICT_NOT_EQUAL,
        Token::Less => XS_CODE_LESS,
        Token::LessEqual => XS_CODE_LESS_EQUAL,
        Token::More => XS_CODE_MORE,
        Token::MoreEqual => XS_CODE_MORE_EQUAL,
        Token::Instanceof => XS_CODE_INSTANCEOF,
        Token::In => XS_CODE_IN,
        _ => unreachable!("not a binary op: {:?}", token),
    }
}
