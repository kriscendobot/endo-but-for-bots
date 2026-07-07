//! The scoper — a transliteration of the AST pass in
//! `c/moddable/xs/sources/xsScope.c` at the oracle pin (design § roadmap
//! row 5; Design Decisions 4 and 5). It runs between the parser
//! ([`crate::parser`], stage-5 children 2–3) and the coder (a later
//! child), performing XS's two dispatch passes over the tree:
//!
//!   * **hoist** ([`fxParserHoist`]): builds the scope tree
//!     (global/eval/function/module/block/`with`/`catch` scopes), hoists
//!     `var` / function declarations, records lexical (`let` / `const` /
//!     `using`) declarations with their duplicate-declaration early
//!     errors, and poisons scopes reached by direct `eval` / `with`.
//!   * **bind** ([`fxParserBind`]): resolves every identifier access to
//!     its declaration up the scope chain, marks captured declarations as
//!     **closure** slots, and computes each function's `scopeCount` — the
//!     frame slot count the coder consumes.
//!
//! **Why this shape is normative.** The coder downstream assigns each
//! declaration its final frame index by walking a scope's declare list in
//! order (`node->index = coder->scopeLevel++`, `xsCode.c` `fxScopeCoded`);
//! bytecode embeds those indices, so the *order and count* of declare
//! nodes in each scope, the closure flags, and the per-function
//! `scopeCount` are a byte-identity contract. This module reproduces XS's
//! exact declare ordering, counter arithmetic, and closure marking; the
//! fixture tests ([`tests`]) pin them as the contract for the coder child.
//!
//! **Representation.** XS mutates the AST in place, hanging a `txScope*`
//! off each scope-creating node and an `access->declaration` off each
//! identifier. The endor AST ([`crate::ast`]) is an immutable value tree,
//! so instead this module keeps a scope **arena** ([`Scope`]) and keys the
//! per-node associations it needs (a node's scope, a node's hoist-time
//! extra flags) by the node's stable address. The observable result — the
//! scope tree, declare lists, counts, closure flags, `scopeCount`, and
//! access resolutions — is the same.
//!
//! Fold (named for the coder child, byte-identity is its bar): `using` /
//! `await using` are parser-folded at the pin already; class member →
//! init-function desugaring is likewise deferred to the coder, so class
//! scoping here covers the binding/heritage/`this` structure but not the
//! synthesized `constructorInit` / `instanceInit` bodies.

#![allow(clippy::needless_range_loop)]

use crate::ast::{flags, node_name, Item, Node};
use crate::parser::{ParseError, Parser};
use crate::token::Token;
use std::collections::HashMap;

// ============================ declare flags ============================

/// The `txDeclareNode`/`txScope` flag bits the scoper sets, at XS's exact
/// bit positions (`xsScript.h` enum) so a declare's flag word matches.
pub mod dflags {
    /// `mxDeclareNodeClosureFlag` — the declaration is captured by an
    /// inner function and lives in a closure slot, not a plain local.
    pub const CLOSURE: u32 = 1 << 12;
    /// `mxDeclareNodeUseClosureFlag` — an access reaches it through a
    /// closure alias.
    pub const USE_CLOSURE: u32 = 1 << 13;
    /// `mxDeclareNodeDisposableFlag` — the synthetic `const` slot a
    /// `using` declaration adds for its disposal record.
    pub const DISPOSABLE: u32 = 1 << 14;
    /// `mxDefineNodeBoundFlag` — a define node was already bound (guards
    /// the declare-list vs define-list double reach). Reserved for the
    /// deferred host-define scoping.
    #[allow(dead_code)]
    pub const BOUND: u32 = 1 << 15;
}

/// `mxEvalFlag` on a scope's own `flags` word (a scope reached by direct
/// `eval` / `with`). Same bit as [`flags::EVAL`].
const SCOPE_EVAL: u32 = flags::EVAL;
/// `mxStrictFlag` on a scope's `flags` word.
const SCOPE_STRICT: u32 = flags::STRICT;

// =============================== symbols ===============================

/// A declaration/access name. XS interns identifiers to `txSymbol*` and
/// compares by pointer; equal spellings share a pointer, so [`String`]
/// equality is faithful. [`Sym::Anon`] models XS's `symbol->ID == -1`
/// synthetic slots (class computed keys, init records) which never equal
/// a source name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sym {
    Named(String),
    Anon(u32),
}

// ============================ declare node =============================

/// One declaration in a scope's declare list — a transliteration of the
/// `txDeclareNode` fields the scoper reads and writes. Carries a stable
/// `id` so define-list entries, closure aliases, and access resolutions
/// can reference it across the index shifts an `eval`-scope prepend or a
/// block placeholder removal cause.
#[derive(Clone, Debug)]
pub struct Declare {
    /// A scope-stable identity, unique within its owning [`Scope`].
    pub id: u32,
    /// `description->token`: `Arg` / `Var` / `Let` / `Const` / `Using` /
    /// `Define` / `Private` / `Specifier`, or [`Token::NoToken`] for the
    /// intermediate-block var placeholder and the function-scope closure
    /// alias.
    pub token: Token,
    /// The declared name, or `None` for an anonymous specifier slot.
    pub symbol: Option<Sym>,
    /// `node->flags` — closure / useClosure / disposable / bound bits.
    pub flags: u32,
    /// 1-based source line.
    pub line: u32,
    /// For a closure alias (`NoToken` in a function scope) the ancestor
    /// declaration it forwards to: `(scope, declare id)`.
    pub alias: Option<(usize, u32)>,
}

/// A define-list entry (`firstDefineNode`…). In XS the define node is the
/// same object as its declare-list entry, but a define covered by an
/// `arg` / `var` slot is in the define list yet absent from the declare
/// list, so the two lists are modeled separately. This list is coder
/// output (initializer emission order); the scoper binds each initializer
/// during the tree walk.
#[derive(Clone, Debug)]
pub struct DefineEntry {
    pub symbol: Option<Sym>,
    pub line: u32,
}

// ================================ scope ================================

/// One scope in the tree — a transliteration of `sxScope`.
#[derive(Clone, Debug)]
pub struct Scope {
    pub parent: Option<usize>,
    /// The scope kind: `Block` / `Function` / `Program` / `Eval` /
    /// `Module` / `With`.
    pub token: Token,
    /// `scope->flags`: `mxStrictFlag` (seeded from the node) plus
    /// `mxEvalFlag` if poisoned.
    pub flags: u32,
    /// The creating node's address, so `self->node->flags` can be read
    /// *live* (its `mxEvalFlag`/`mxArgumentsFlag` are set after creation).
    node_ptr: usize,
    /// The creating node's parse-time `flags` word (before hoist extras).
    node_base_flags: u32,
    /// The declare list, in XS's order (`firstDeclareNode`…). Removals in
    /// [`fx_scope_hoisted`] are applied here.
    pub declares: Vec<Declare>,
    /// The define list, in define order (coder output).
    pub defines: Vec<DefineEntry>,
    /// The next declare id to hand out in this scope.
    next_id: u32,
    /// `declareNodeCount` — the counter, which diverges from
    /// `declares.len()` after the program/eval `var`/define discount.
    pub declare_count: i32,
    /// `closureNodeCount`.
    pub closure_count: i32,
    /// `defineNodeCount`.
    pub define_count: i32,
    /// `disposableNodeCount`.
    pub disposable_count: i32,
    /// `mxDefaultFlag` was propagated here by [`fx_scope_arrow`] — an
    /// arrow function that transitively uses `this` / `super` / `target`.
    pub arrow_default: bool,
}

impl Scope {
    fn new(parent: Option<usize>, token: Token, node_ptr: usize, node_base_flags: u32) -> Scope {
        Scope {
            parent,
            token,
            flags: node_base_flags & SCOPE_STRICT,
            node_ptr,
            node_base_flags,
            declares: Vec::new(),
            defines: Vec::new(),
            next_id: 0,
            declare_count: 0,
            closure_count: 0,
            define_count: 0,
            disposable_count: 0,
            arrow_default: false,
        }
    }
}

// ============================ access record ============================

/// The resolution the bind pass records for one identifier access, for
/// the fixture dump. XS writes `access->declaration`; here we log the
/// resolved `(scope, index)` or `None` for a global / `with` / eval
/// non-binding.
#[derive(Clone, Debug)]
pub struct AccessRecord {
    pub symbol: String,
    pub line: u32,
    pub resolved: Option<(usize, u32)>,
}

// =========================== scoper output ============================

/// The whole scoper result: the scope arena plus the access log and the
/// root scope index.
#[derive(Clone, Debug)]
pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub root: usize,
    pub accesses: Vec<AccessRecord>,
    /// `scopeCount` per function/program/module scope, keyed by scope
    /// index (the coder's frame slot count).
    pub scope_counts: HashMap<usize, i32>,
    /// A scope-creating node's address → its scope(s): `.0` primary
    /// (`self->scope`), `.1` secondary (`statementScope`/`symbolScope`).
    /// The coder walks the *same* parsed tree the scoper walked, so a
    /// node's address keys back to the scope XS hung off it in place
    /// (`self->scope`, `xsScope.c`). Keyed with [`node_key`].
    pub node_scopes: HashMap<usize, (usize, Option<usize>)>,
}

/// The stable identity the scoper/coder use to associate a scope (and,
/// later, an access resolution) with a node: the node's address in the
/// parsed tree. Faithful to XS hanging `txScope*`/`access->declaration`
/// off the node in place — valid only while that tree is alive, which it
/// is for the whole compile.
pub fn node_key(n: &Node) -> usize {
    n as *const Node as usize
}

// ============================ entry points ============================

/// Parse `source` as a Script and run the scoper, returning the scope
/// tree or the first parser/scoper early error.
pub fn scope_program(source: &str, strict: bool) -> Result<ScopeTree, ParseError> {
    let mut parser = Parser::new(source, strict, false)?;
    let root = parser.parse_program(strict)?;
    run(&root)
}

/// Parse `source` as a Module and run the scoper.
pub fn scope_module(source: &str) -> Result<ScopeTree, ParseError> {
    let mut parser = Parser::new(source, true, true)?;
    let root = parser.parse_module()?;
    run(&root)
}

/// Run the two scoper passes over an already-parsed root node.
pub fn run(root: &Item) -> Result<ScopeTree, ParseError> {
    let root_node = match root {
        Item::Node(n) => n.as_ref(),
        _ => return Err(err(1, "invalid root")),
    };
    let mut s = Scoper::default();
    // fxParserHoist
    s.hoist_dispatch(root_node)?;
    // fxParserBind
    s.bind_dispatch(root_node)?;
    let root_scope = *s.node_scope.get(&node_ptr(root_node)).ok_or_else(|| err(root_node.line, "no root scope"))?;
    Ok(ScopeTree {
        scopes: s.scopes,
        root: root_scope.0,
        accesses: s.accesses,
        scope_counts: s.scope_counts,
        node_scopes: s.node_scope,
    })
}

// ============================ scoper state ============================

/// Ambient hoister/binder state threaded through the passes, plus the
/// arena and the by-address side tables the immutable AST needs.
#[derive(Default)]
struct Scoper {
    scopes: Vec<Scope>,
    /// `hoister->scope` / `binder->scope` — the current scope.
    scope: Option<usize>,
    /// `hoister->functionScope`.
    function_scope: Option<usize>,
    /// `hoister->bodyScope`.
    body_scope: Option<usize>,
    /// `hoister->environmentNode` (a node address).
    environment_node: Option<usize>,
    /// `binder->classNode` — the class node whose members are binding.
    /// (Reserved for the deferred class-scoping pass.)
    #[allow(dead_code)]
    class_node: Option<usize>,
    /// node address → its scope(s): `.0` primary (`self->scope`), `.1`
    /// secondary (`statementScope` / `symbolScope`).
    node_scope: HashMap<usize, (usize, Option<usize>)>,
    /// Hoist-time extra flags OR-ed onto a node (`self->node->flags |=`).
    node_extra: HashMap<usize, u32>,
    /// The binder frame counters.
    scope_level: i32,
    scope_maximum: i32,
    scope_counts: HashMap<usize, i32>,
    accesses: Vec<AccessRecord>,
    /// `hoister->firstExportLink` — the exported names seen so far, for
    /// duplicate-export detection.
    export_links: Vec<Sym>,
    /// Next anonymous-symbol id. (Reserved for class computed-key slots.)
    #[allow(dead_code)]
    anon: u32,
}

fn node_ptr(n: &Node) -> usize {
    n as *const Node as usize
}

fn err(line: u32, msg: &str) -> ParseError {
    ParseError { line, kind: crate::parser::ParseErrorKind::Syntax, message: msg.to_string() }
}

// --------- child-slot accessors (positional, per the struct layout) ---------

fn child<'a>(n: &'a Node, i: usize) -> Option<&'a Item> {
    n.children.get(i)
}
fn child_node<'a>(n: &'a Node, i: usize) -> Option<&'a Node> {
    match n.children.get(i) {
        Some(Item::Node(b)) => Some(b.as_ref()),
        _ => None,
    }
}
fn child_sym(n: &Node, i: usize) -> Option<String> {
    match n.children.get(i) {
        Some(Item::Symbol(s)) => Some(s.clone()),
        _ => None,
    }
}
#[allow(dead_code)]
fn child_list<'a>(n: &'a Node, i: usize) -> Option<&'a [Item]> {
    match n.children.get(i) {
        Some(Item::List(v)) => Some(v.as_slice()),
        _ => None,
    }
}

impl Scoper {
    fn node_flags(&self, n: &Node) -> u32 {
        n.flags | self.node_extra.get(&node_ptr(n)).copied().unwrap_or(0)
    }
    fn add_extra(&mut self, ptr: usize, bits: u32) {
        *self.node_extra.entry(ptr).or_insert(0) |= bits;
    }
    fn scope_node_flags(&self, si: usize) -> u32 {
        let sc = &self.scopes[si];
        sc.node_base_flags | self.node_extra.get(&sc.node_ptr).copied().unwrap_or(0)
    }

    // ===================== scope helpers (xsScope.c top) =====================

    /// `fxScopeNew`.
    fn scope_new(&mut self, node: &Node, token: Token) -> usize {
        let parent = self.scope;
        let sc = Scope::new(parent, token, node_ptr(node), self.node_flags(node));
        let id = self.scopes.len();
        self.scopes.push(sc);
        self.scope = Some(id);
        id
    }

    /// Build a fresh declare with a scope-stable id, without inserting it.
    fn new_declare(&mut self, si: usize, token: Token, symbol: Option<Sym>, line: u32) -> Declare {
        let id = self.scopes[si].next_id;
        self.scopes[si].next_id += 1;
        Declare { id, token, symbol, flags: 0, line, alias: None }
    }

    /// `fxScopeAddDeclareNode` — append (or, for an eval scope, prepend)
    /// and, for a `using`, add the disposal `const`. Returns the id.
    fn scope_add_declare(&mut self, si: usize, decl: Declare) -> u32 {
        let is_using = decl.token == Token::Using;
        let id = decl.id;
        let sc = &mut self.scopes[si];
        sc.declare_count += 1;
        if sc.token == Token::Eval {
            sc.declares.insert(0, decl);
        } else {
            sc.declares.push(decl);
        }
        if is_using {
            let mut d = self.new_declare(si, Token::Const, None, 0);
            d.flags |= dflags::DISPOSABLE;
            self.scope_add_declare(si, d);
            self.scopes[si].disposable_count += 1;
        }
        id
    }

    /// `fxScopeAddDefineNode` — append a define-list entry (define-order).
    fn scope_add_define(&mut self, si: usize, symbol: Option<Sym>, line: u32) {
        let sc = &mut self.scopes[si];
        sc.define_count += 1;
        sc.defines.push(DefineEntry { symbol, line });
    }

    /// `fxScopeGetDeclareNode` — linear symbol lookup, returning the id.
    fn scope_get_declare(&self, si: usize, symbol: &Sym) -> Option<u32> {
        let sc = &self.scopes[si];
        sc.declares.iter().find(|d| d.symbol.as_ref() == Some(symbol)).map(|d| d.id)
    }

    fn declare_mut(&mut self, si: usize, id: u32) -> &mut Declare {
        self.scopes[si].declares.iter_mut().find(|d| d.id == id).expect("declare id present")
    }
    fn declare_ref(&self, si: usize, id: u32) -> &Declare {
        self.scopes[si].declares.iter().find(|d| d.id == id).expect("declare id present")
    }

    /// `fxScopeEval` — poison a scope and every ancestor with `mxEvalFlag`.
    fn scope_eval(&mut self, mut si: Option<usize>) {
        while let Some(i) = si {
            self.scopes[i].flags |= SCOPE_EVAL;
            si = self.scopes[i].parent;
        }
    }

    /// `fxScopeArrow` — propagate `mxDefaultFlag` up to the nearest arrow
    /// function that transitively uses `this` / `super` / `new.target`.
    fn scope_arrow(&mut self, si: Option<usize>) {
        let mut cur = si;
        while let Some(i) = cur {
            let tok = self.scopes[i].token;
            if tok == Token::Eval || tok == Token::Program {
                return;
            } else if tok == Token::Function {
                if self.scope_node_flags(i) & flags::ARROW != 0 {
                    self.scopes[i].arrow_default = true;
                    cur = self.scopes[i].parent;
                    continue;
                }
                return;
            } else {
                cur = self.scopes[i].parent;
            }
        }
    }

    /// `fxScopeHoisted` — the count fix-ups when a scope closes.
    fn fx_scope_hoisted(&mut self, si: usize) {
        let tok = self.scopes[si].token;
        if tok == Token::Block {
            // Drop the NoToken var placeholders; fix declareNodeCount. Ids
            // are stable so the define list needs no remap.
            let sc = &mut self.scopes[si];
            let mut removed = 0;
            sc.declares.retain(|d| {
                if d.token == Token::NoToken {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
            sc.declare_count -= removed;
        } else if tok == Token::Program {
            let sc = &mut self.scopes[si];
            for d in &sc.declares {
                if d.token == Token::Define || d.token == Token::Var {
                    sc.declare_count -= 1;
                }
            }
        } else if tok == Token::Eval {
            let sc = &mut self.scopes[si];
            if sc.flags & SCOPE_STRICT == 0 {
                for d in &sc.declares {
                    if d.token == Token::Define || d.token == Token::Var {
                        sc.declare_count -= 1;
                    }
                }
            }
        }
        self.scope = self.scopes[si].parent;
    }
}

// ============================ fxScopeLookup ============================

impl Scoper {
    /// `fxScopeLookup` — resolve `symbol` up the scope chain from scope
    /// `si`, creating function-scope closure aliases as XS does. Returns
    /// the resolved `(scope, declare id)` or `None` for a global / `with`
    /// / eval-shadowed access. `closure_flag` marks captures.
    fn scope_lookup(
        &mut self,
        si: usize,
        symbol: &Sym,
        sym_line: u32,
        is_private_member: bool,
        closure_flag: bool,
    ) -> Option<(usize, u32)> {
        match self.scopes[si].token {
            Token::Eval => {
                let mut found = self.scope_get_declare(si, symbol);
                if let Some(id) = found {
                    let strict = self.scopes[si].flags & SCOPE_STRICT != 0;
                    let dtok = self.declare_ref(si, id).token;
                    if !strict && (dtok == Token::Var || dtok == Token::Define) {
                        found = None;
                    } else if closure_flag {
                        self.declare_mut(si, id).flags |= dflags::CLOSURE;
                    }
                } else if (self.scopes[si].flags & SCOPE_STRICT != 0) && is_private_member {
                    let mut d = self.new_declare(si, Token::Private, Some(symbol.clone()), sym_line);
                    d.flags |= dflags::CLOSURE;
                    let id = self.scope_add_declare(si, d);
                    self.scopes[si].closure_count += 1;
                    found = Some(id);
                }
                found.map(|id| (si, id))
            }
            Token::Function => {
                if let Some(id) = self.scope_get_declare(si, symbol) {
                    if closure_flag {
                        self.declare_mut(si, id).flags |= dflags::CLOSURE;
                    }
                    Some((si, id))
                } else if (self.scope_node_flags(si) & SCOPE_EVAL != 0)
                    && (self.scope_node_flags(si) & SCOPE_STRICT == 0)
                {
                    // eval can create variables that override closures
                    None
                } else if let Some(parent) = self.scopes[si].parent {
                    let resolved = self.scope_lookup(parent, symbol, sym_line, is_private_member, true);
                    if let Some((rscope, rid)) = resolved {
                        let rline = self.declare_ref(rscope, rid).line;
                        let mut alias = self.new_declare(si, Token::NoToken, Some(symbol.clone()), rline);
                        alias.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
                        alias.alias = Some((rscope, rid));
                        let aid = self.scope_add_declare(si, alias);
                        self.scopes[si].closure_count += 1;
                        Some((si, aid))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Token::Program => {
                if let Some(id) = self.scope_get_declare(si, symbol) {
                    let dtok = self.declare_ref(si, id).token;
                    if dtok == Token::Var || dtok == Token::Define {
                        None
                    } else {
                        Some((si, id))
                    }
                } else {
                    None
                }
            }
            Token::With => {
                // a with object can shadow any variable
                None
            }
            _ => {
                if let Some(id) = self.scope_get_declare(si, symbol) {
                    if closure_flag {
                        self.declare_mut(si, id).flags |= dflags::CLOSURE;
                    }
                    Some((si, id))
                } else if let Some(parent) = self.scopes[si].parent {
                    self.scope_lookup(parent, symbol, sym_line, is_private_member, closure_flag)
                } else {
                    None
                }
            }
        }
    }
}

// ============================== hoist pass ==============================

impl Scoper {
    /// `fxNodeDispatchHoist` — dispatch one node's hoist.
    fn hoist_dispatch(&mut self, node: &Node) -> Result<(), ParseError> {
        match node.token {
            Token::Program => self.hoist_program(node),
            Token::Module => self.hoist_module(node),
            Token::Block => self.hoist_block(node),
            Token::Body => self.hoist_body(node),
            Token::Function | Token::Generator => self.hoist_function(node),
            Token::Call | Token::New => self.hoist_call(node),
            Token::Catch => self.hoist_catch(node),
            Token::Coalesce => self.hoist_coalesce(node),
            Token::Arg | Token::Var | Token::Let | Token::Const | Token::Using => self.hoist_declare(node),
            Token::Define => self.hoist_define(node),
            Token::For => self.hoist_for(node),
            Token::ForIn | Token::ForOf | Token::ForAwaitOf => self.hoist_for_in_of(node),
            Token::Switch => self.hoist_switch(node),
            Token::With => self.hoist_with(node),
            Token::String => self.hoist_string(node),
            Token::Import => self.hoist_import(node),
            Token::Export => self.hoist_export(node),
            // fold: Class / Host — deferred (see report).
            _ => self.hoist_children(node),
        }
    }

    /// `fxNodeHoist` / `fxNodeDistribute` default — hoist every child node.
    fn hoist_children(&mut self, node: &Node) -> Result<(), ParseError> {
        for item in &node.children {
            self.hoist_item(item)?;
        }
        Ok(())
    }
    fn hoist_item(&mut self, item: &Item) -> Result<(), ParseError> {
        match item {
            Item::Node(n) => self.hoist_dispatch(n),
            Item::List(v) => {
                for it in v {
                    self.hoist_item(it)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn hoist_program(&mut self, node: &Node) -> Result<(), ParseError> {
        // XS: XS_TOKEN_EVAL when parser->flags has mxEvalFlag, else PROGRAM.
        let token = if self.node_flags(node) & SCOPE_EVAL != 0 { Token::Eval } else { Token::Program };
        let si = self.scope_new(node, token);
        self.function_scope = Some(si);
        self.body_scope = Some(si);
        self.node_scope.insert(node_ptr(node), (si, None));
        self.environment_node = Some(node_ptr(node));
        if let Some(body) = child(node, 0) {
            self.hoist_item(body)?;
        }
        self.environment_node = None;
        // self->variableCount = functionScope->declareNodeCount (coder use)
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_module(&mut self, node: &Node) -> Result<(), ParseError> {
        let si = self.scope_new(node, Token::Module);
        self.function_scope = Some(si);
        self.body_scope = Some(si);
        self.node_scope.insert(node_ptr(node), (si, None));
        self.environment_node = Some(node_ptr(node));
        if let Some(body) = child(node, 0) {
            self.hoist_item(body)?;
        }
        self.environment_node = None;
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_block(&mut self, node: &Node) -> Result<(), ParseError> {
        let si = self.scope_new(node, Token::Block);
        self.node_scope.insert(node_ptr(node), (si, None));
        if let Some(stmt) = child(node, 0) {
            self.hoist_item(stmt)?;
        }
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_body(&mut self, node: &Node) -> Result<(), ParseError> {
        let si = self.scope_new(node, Token::Block);
        self.body_scope = Some(si);
        self.node_scope.insert(node_ptr(node), (si, None));
        let env = self.environment_node;
        self.environment_node = Some(node_ptr(node));
        if let Some(stmt) = child(node, 0) {
            self.hoist_item(stmt)?;
        }
        self.environment_node = env;
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_function(&mut self, node: &Node) -> Result<(), ParseError> {
        let function_scope = self.function_scope;
        let body_scope = self.body_scope;
        let si = self.scope_new(node, Token::Function);
        self.function_scope = Some(si);
        self.body_scope = None;
        self.node_scope.insert(node_ptr(node), (si, None));
        // named function expression: a CONST self-binding define.
        if let Some(sym) = child_sym(node, 0) {
            let s = Sym::Named(sym);
            let d = self.new_declare(si, Token::Define, Some(s.clone()), node.line);
            self.scope_add_declare(si, d);
            self.scope_add_define(si, Some(s), node.line);
        }
        // params (children[1])
        if let Some(params) = child(node, 1) {
            self.hoist_item(params)?;
        }
        // arguments injection
        let nf = self.node_flags(node);
        if (nf & (flags::ARGUMENTS | SCOPE_EVAL) != 0) && (nf & flags::ARROW == 0) {
            let d = self.new_declare(si, Token::Var, Some(Sym::Named("arguments".to_string())), node.line);
            self.scope_add_declare(si, d);
        }
        // body (children[2])
        if let Some(body) = child(node, 2) {
            self.hoist_item(body)?;
        }
        self.fx_scope_hoisted(si);
        self.body_scope = body_scope;
        self.function_scope = function_scope;
        Ok(())
    }

    fn hoist_call(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0] = reference, children[1] = params
        if let Some(reference) = child_node(node, 0) {
            if reference.token == Token::Access {
                if let Some(sym) = child_sym(reference, 0) {
                    if sym == "eval" {
                        self.scope_eval(self.scope);
                        if let Some(fs) = self.function_scope {
                            let fptr = self.scopes[fs].node_ptr;
                            self.add_extra(fptr, flags::ARGUMENTS | SCOPE_EVAL);
                        }
                        if let Some(env) = self.environment_node {
                            self.add_extra(env, SCOPE_EVAL);
                        }
                        // params->flags |= mxEvalParametersFlag — coder use.
                    }
                }
            }
        }
        if let Some(reference) = child(node, 0) {
            self.hoist_item(reference)?;
        }
        if let Some(params) = child(node, 1) {
            self.hoist_item(params)?;
        }
        Ok(())
    }

    fn hoist_catch(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0] = parameter (or Null), children[1] = statement
        let has_param = matches!(child(node, 0), Some(Item::Node(_)));
        if has_param {
            let scope = self.scope_new(node, Token::Block);
            if let Some(param) = child(node, 0) {
                self.hoist_item(param)?;
            }
            let statement_scope = self.scope_new(node, Token::Block);
            if let Some(stmt) = child(node, 1) {
                self.hoist_item(stmt)?;
            }
            self.fx_scope_hoisted(statement_scope);
            self.fx_scope_hoisted(scope);
            self.node_scope.insert(node_ptr(node), (scope, Some(statement_scope)));
            // duplicate: a statementScope declare that also names a
            // parameter is a redeclaration error.
            let names: Vec<(Option<Sym>, u32)> = self.scopes[statement_scope]
                .declares
                .iter()
                .map(|d| (d.symbol.clone(), d.line))
                .collect();
            for (sym, line) in names {
                if let Some(s) = &sym {
                    if self.scope_get_declare(scope, s).is_some() {
                        return Err(err(line, "duplicate variable"));
                    }
                }
            }
        } else {
            let statement_scope = self.scope_new(node, Token::Block);
            if let Some(stmt) = child(node, 1) {
                self.hoist_item(stmt)?;
            }
            self.fx_scope_hoisted(statement_scope);
            self.node_scope.insert(node_ptr(node), (statement_scope, None));
        }
        Ok(())
    }

    fn hoist_coalesce(&mut self, node: &Node) -> Result<(), ParseError> {
        // early error: mixing ?? with && / || without parentheses
        if let Some(l) = child_node(node, 0) {
            if l.token == Token::And {
                return Err(err(node.line, "missing () around &&"));
            }
            if l.token == Token::Or {
                return Err(err(node.line, "missing () around ||"));
            }
        }
        if let Some(r) = child_node(node, 1) {
            if r.token == Token::And {
                return Err(err(node.line, "missing () around &&"));
            }
            if r.token == Token::Or {
                return Err(err(node.line, "missing () around ||"));
            }
        }
        self.hoist_children(node)
    }

    fn hoist_declare(&mut self, node: &Node) -> Result<(), ParseError> {
        let symbol = child_sym(node, 0).map(Sym::Named);
        let symbol = match symbol {
            Some(s) => s,
            None => return Ok(()),
        };
        let function_scope = self.function_scope.unwrap();
        let scope = self.scope.unwrap();
        if node.token == Token::Arg {
            if let Some(id) = self.scope_get_declare(function_scope, &symbol) {
                let dtok = self.declare_ref(function_scope, id).token;
                let fnf = self.scope_node_flags(function_scope);
                let dup_ctx = flags::ARROW | flags::ASYNC | flags::METHOD | flags::NOT_SIMPLE_PARAMETERS | flags::STRICT;
                if dtok == Token::Arg && (fnf & dup_ctx != 0) {
                    return Err(err(node.line, "duplicate argument"));
                }
            } else {
                let d = self.new_declare(function_scope, Token::Arg, Some(symbol), node.line);
                self.scope_add_declare(function_scope, d);
            }
        } else if node.token == Token::Const || node.token == Token::Let || node.token == Token::Using {
            let body_scope = self.body_scope.unwrap();
            let mut existing = self.scope_get_declare(scope, &symbol);
            if existing.is_none() && scope == body_scope {
                if let Some(id) = self.scope_get_declare(function_scope, &symbol) {
                    if self.declare_ref(function_scope, id).token != Token::Arg {
                        // arg-vs-lexical is a conflict; other tokens not
                    } else {
                        existing = Some(id);
                    }
                }
            }
            if existing.is_some() {
                return Err(err(node.line, "duplicate variable"));
            }
            let d = self.new_declare(scope, node.token, Some(symbol), node.line);
            self.scope_add_declare(scope, d);
        } else {
            // VAR
            let body_scope = self.body_scope.unwrap();
            self.hoist_var(&symbol, node.line, scope, body_scope, function_scope)?;
        }
        Ok(())
    }

    fn hoist_var(
        &mut self,
        symbol: &Sym,
        line: u32,
        start_scope: usize,
        body_scope: usize,
        function_scope: usize,
    ) -> Result<(), ParseError> {
        let mut scope = start_scope;
        let mut conflict: Option<u32> = None;
        while scope != body_scope {
            if let Some(id) = self.scope_get_declare(scope, symbol) {
                let dtok = self.declare_ref(scope, id).token;
                if matches!(dtok, Token::Const | Token::Let | Token::Using | Token::Define) {
                    conflict = Some(id);
                    break;
                }
            }
            scope = self.scopes[scope].parent.unwrap_or(body_scope);
        }
        if conflict.is_none() {
            if let Some(id) = self.scope_get_declare(scope, symbol) {
                let dtok = self.declare_ref(scope, id).token;
                if matches!(dtok, Token::Const | Token::Let | Token::Using) {
                    conflict = Some(id);
                }
            }
        }
        if conflict.is_some() {
            return Err(err(line, "duplicate variable"));
        }
        // add to bodyScope unless already an arg/var in functionScope
        let cover = self
            .scope_get_declare(function_scope, symbol)
            .map(|id| self.declare_ref(function_scope, id).token)
            .map(|t| t == Token::Arg || t == Token::Var)
            .unwrap_or(false);
        if !cover {
            let d = self.new_declare(body_scope, Token::Var, Some(symbol.clone()), line);
            self.scope_add_declare(body_scope, d);
        }
        // placeholder NoToken in every intermediate block scope
        let mut sc = start_scope;
        while sc != body_scope {
            let d = self.new_declare(sc, Token::NoToken, Some(symbol.clone()), line);
            self.scope_add_declare(sc, d);
            sc = self.scopes[sc].parent.unwrap_or(body_scope);
        }
        Ok(())
    }

    fn hoist_define(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0] = symbol, children[1] = initializer (function)
        let symbol = match child_sym(node, 0) {
            Some(s) => Sym::Named(s),
            None => return Ok(()),
        };
        let scope = self.scope.unwrap();
        let body_scope = self.body_scope.unwrap();
        let function_scope = self.function_scope.unwrap();
        let strict = self.node_flags(node) & SCOPE_STRICT != 0;
        if strict {
            if let Sym::Named(s) = &symbol {
                if s == "arguments" || s == "eval" || s == "yield" {
                    return Err(err(node.line, "invalid definition"));
                }
            }
        }
        if scope == body_scope && self.scopes[scope].token != Token::Module {
            if let Some(id) = self.scope_get_declare(body_scope, &symbol) {
                let dtok = self.declare_ref(body_scope, id).token;
                if dtok == Token::Const || dtok == Token::Let {
                    return Err(err(node.line, "duplicate variable"));
                }
            } else {
                let mut have = false;
                if function_scope != body_scope {
                    have = self.scope_get_declare(function_scope, &symbol).is_some();
                }
                if !have {
                    let d = self.new_declare(body_scope, Token::Define, Some(symbol.clone()), node.line);
                    self.scope_add_declare(body_scope, d);
                }
            }
            self.scope_add_define(body_scope, Some(symbol.clone()), node.line);
        } else {
            if self.scope_get_declare(scope, &symbol).is_some() {
                return Err(err(node.line, "duplicate variable"));
            }
            let d = self.new_declare(scope, Token::Define, Some(symbol.clone()), node.line);
            self.scope_add_declare(scope, d);
            self.scope_add_define(scope, Some(symbol.clone()), node.line);
        }
        // dispatch the initializer (function), with its self-symbol nulled
        // (fxDefineNodeHoist nulls initializer->symbol so the function's
        // named-expression self-binding is not created for a declaration).
        if let Some(init) = child_node(node, 1) {
            self.hoist_function_no_self(init)?;
        }
        Ok(())
    }

    /// Hoist a function node but suppress the named-expression self CONST
    /// (used for a function *declaration*'s initializer).
    fn hoist_function_no_self(&mut self, node: &Node) -> Result<(), ParseError> {
        if node.token != Token::Function && node.token != Token::Generator {
            return self.hoist_dispatch(node);
        }
        let function_scope = self.function_scope;
        let body_scope = self.body_scope;
        let si = self.scope_new(node, Token::Function);
        self.function_scope = Some(si);
        self.body_scope = None;
        self.node_scope.insert(node_ptr(node), (si, None));
        if let Some(params) = child(node, 1) {
            self.hoist_item(params)?;
        }
        let nf = self.node_flags(node);
        if (nf & (flags::ARGUMENTS | SCOPE_EVAL) != 0) && (nf & flags::ARROW == 0) {
            let d = self.new_declare(si, Token::Var, Some(Sym::Named("arguments".to_string())), node.line);
            self.scope_add_declare(si, d);
        }
        if let Some(body) = child(node, 2) {
            self.hoist_item(body)?;
        }
        self.fx_scope_hoisted(si);
        self.body_scope = body_scope;
        self.function_scope = function_scope;
        Ok(())
    }

    fn hoist_for(&mut self, node: &Node) -> Result<(), ParseError> {
        let si = self.scope_new(node, Token::Block);
        self.node_scope.insert(node_ptr(node), (si, None));
        for i in 0..4 {
            if let Some(c) = child(node, i) {
                self.hoist_item(c)?;
            }
        }
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_for_in_of(&mut self, node: &Node) -> Result<(), ParseError> {
        let si = self.scope_new(node, Token::Block);
        self.node_scope.insert(node_ptr(node), (si, None));
        for i in 0..3 {
            if let Some(c) = child(node, i) {
                self.hoist_item(c)?;
            }
        }
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_switch(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0] = expression, children[1] = items (list of Case)
        if let Some(expr) = child(node, 0) {
            self.hoist_item(expr)?;
        }
        let si = self.scope_new(node, Token::Block);
        self.node_scope.insert(node_ptr(node), (si, None));
        if let Some(items) = child(node, 1) {
            self.hoist_item(items)?;
        }
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_with(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0] = expression, children[1] = statement
        if let Some(expr) = child(node, 0) {
            self.hoist_item(expr)?;
        }
        let si = self.scope_new(node, Token::With);
        self.node_scope.insert(node_ptr(node), (si, None));
        self.scope_eval(self.scopes[si].parent);
        if let Some(stmt) = child(node, 1) {
            self.hoist_item(stmt)?;
        }
        self.fx_scope_hoisted(si);
        Ok(())
    }

    fn hoist_string(&mut self, _node: &Node) -> Result<(), ParseError> {
        // fxStringNodeHoist marks legacy-octal strings in strict scopes as
        // errors at code time; not part of the scope-shape contract.
        Ok(())
    }

    /// `fxImportNodeHoist` — each import specifier declares a module-scope
    /// `let` that is an immutable indirect binding (closure|useClosure);
    /// a bare `import "m"` declares one anonymous slot. The `from`/`with`
    /// re-export attributes are coder-side. Modules are strict, so
    /// importing `arguments`/`eval` is an early error.
    fn hoist_import(&mut self, node: &Node) -> Result<(), ParseError> {
        let scope = self.scope.unwrap();
        let strict = self.node_flags(node) & SCOPE_STRICT != 0;
        let specs = match child(node, 0) {
            Some(Item::List(v)) if !v.is_empty() => v.clone(),
            _ => {
                // bare `import "m"` — one anonymous indirect binding.
                let mut d = self.new_declare(scope, Token::Let, None, node.line);
                d.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
                self.scope_add_declare(scope, d);
                return Ok(());
            }
        };
        for spec in &specs {
            let Item::Node(spec) = spec else { continue };
            let local = child_sym(spec, 1).or_else(|| child_sym(spec, 0));
            let Some(local) = local else { continue };
            let sym = Sym::Named(local.clone());
            if strict && (local == "arguments" || local == "eval") {
                return Err(err(spec.line, "invalid import"));
            }
            if self.scope_get_declare(scope, &sym).is_some() {
                return Err(err(spec.line, "duplicate variable"));
            }
            let mut d = self.new_declare(scope, Token::Let, Some(sym), spec.line);
            d.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
            self.scope_add_declare(scope, d);
        }
        Ok(())
    }

    /// `fxExportNodeHoist` (the local-export half) — record each exported
    /// name in the export-link set, raising a duplicate-export early
    /// error. The `export … from` re-export indirection (which synthesizes
    /// indirect `let` bindings) is folded (see report).
    fn hoist_export(&mut self, node: &Node) -> Result<(), ParseError> {
        if matches!(child(node, 1), Some(Item::Node(_))) {
            // `export … from "m"` — folded.
            return Ok(());
        }
        if let Some(Item::List(specs)) = child(node, 0) {
            for spec in specs {
                let Item::Node(spec) = spec else { continue };
                // export name = asSymbol ? asSymbol : symbol
                let name = child_sym(spec, 1).or_else(|| child_sym(spec, 0));
                if let Some(name) = name {
                    let sym = Sym::Named(name);
                    if self.export_links.contains(&sym) {
                        return Err(err(spec.line, "duplicate export"));
                    }
                    self.export_links.push(sym);
                }
            }
        }
        Ok(())
    }
}

// ============================== bind pass ==============================

impl Scoper {
    // ---- binder frame counters (fxBinderPush/PopVariables) ----
    fn push_variables(&mut self, count: i32) {
        self.scope_level += count;
        if self.scope_maximum < self.scope_level {
            self.scope_maximum = self.scope_level;
        }
    }
    fn pop_variables(&mut self, count: i32) {
        self.scope_level -= count;
    }

    /// `fxScopeBinding` — enter a scope and reserve its declare slots.
    fn fx_scope_binding(&mut self, si: usize) {
        self.scope = Some(si);
        let count = self.scopes[si].declare_count;
        self.push_variables(count);
    }

    /// `fxScopeBound` — close a scope: eval/module/program closure marking,
    /// then the closure-slot and declare-slot frame arithmetic.
    fn fx_scope_bound(&mut self, si: usize) {
        let tok = self.scopes[si].token;
        let eval = self.scopes[si].flags & SCOPE_EVAL != 0;
        if eval {
            for d in &mut self.scopes[si].declares {
                d.flags |= dflags::CLOSURE;
            }
        }
        if tok == Token::Module {
            for d in &mut self.scopes[si].declares {
                if d.flags & dflags::DISPOSABLE == 0 {
                    d.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
                }
            }
        } else if tok == Token::Program {
            for d in &mut self.scopes[si].declares {
                d.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
            }
        }
        let closure = self.scopes[si].closure_count;
        let declare = self.scopes[si].declare_count;
        self.scope_level += closure;
        self.scope_maximum += closure;
        self.pop_variables(declare);
        self.scope = self.scopes[si].parent;
    }

    fn record_access(&mut self, symbol: &str, line: u32, resolved: Option<(usize, u32)>) {
        self.accesses.push(AccessRecord { symbol: symbol.to_string(), line, resolved });
    }

    /// `fxNodeDispatchBind`.
    fn bind_dispatch(&mut self, node: &Node) -> Result<(), ParseError> {
        match node.token {
            Token::Program => self.bind_program(node),
            Token::Module => self.bind_module(node),
            Token::Block | Token::Body => self.bind_block(node),
            Token::Function | Token::Generator => self.bind_function(node),
            Token::Access => self.bind_access(node),
            Token::Arg | Token::Var | Token::Let | Token::Const | Token::Using => self.bind_declare_node(node),
            Token::Define => self.bind_define(node),
            Token::Assign => self.bind_assign(node),
            Token::Binding => self.bind_binding(node),
            Token::Catch => self.bind_catch(node),
            Token::For => self.bind_for(node),
            Token::ForIn | Token::ForOf | Token::ForAwaitOf => self.bind_for_in_of(node),
            Token::Switch => self.bind_switch(node),
            Token::With => self.bind_with(node),
            Token::Try => self.bind_try(node),
            Token::Array => self.bind_array(node),
            Token::ArrayBinding => self.bind_array_binding(node),
            Token::Object => self.bind_object(node),
            Token::ObjectBinding => self.bind_object_binding(node),
            Token::Params => self.bind_params(node),
            Token::ParamsBinding => self.bind_params_binding(node),
            Token::Spread => self.bind_spread(node),
            Token::Delegate => self.bind_delegate(node),
            Token::Template => self.bind_template(node),
            Token::This | Token::Target => self.bind_this_target(node),
            Token::Super => self.bind_super(node),
            Token::Increment | Token::Decrement => self.bind_postfix(node),
            Token::Export => self.bind_export(node),
            // fold: Field / PrivateMember / Class — deferred.
            _ => self.bind_children(node),
        }
    }

    fn bind_children(&mut self, node: &Node) -> Result<(), ParseError> {
        for item in &node.children {
            self.bind_item(item)?;
        }
        Ok(())
    }
    fn bind_item(&mut self, item: &Item) -> Result<(), ParseError> {
        match item {
            Item::Node(n) => self.bind_dispatch(n),
            Item::List(v) => {
                for it in v {
                    self.bind_item(it)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn scope_of(&self, node: &Node) -> (usize, Option<usize>) {
        *self.node_scope.get(&node_ptr(node)).expect("scope for node")
    }

    fn bind_program(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        if let Some(body) = child(node, 0) {
            self.bind_item(body)?;
        }
        self.fx_scope_bound(si);
        self.scope_counts.insert(si, self.scope_maximum);
        Ok(())
    }

    fn bind_module(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        if let Some(body) = child(node, 0) {
            self.bind_item(body)?;
        }
        self.fx_scope_bound(si);
        self.scope_counts.insert(si, self.scope_maximum);
        Ok(())
    }

    fn bind_block(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        let disp = self.scopes[si].disposable_count > 0;
        if disp {
            self.push_variables(2);
        }
        if let Some(stmt) = child(node, 0) {
            self.bind_item(stmt)?;
        }
        if disp {
            self.pop_variables(2);
        }
        self.fx_scope_bound(si);
        Ok(())
    }

    fn bind_function(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        let level = self.scope_level;
        let maximum = self.scope_maximum;
        self.scope_level = 0;
        self.scope_maximum = 0;
        self.fx_scope_binding(si);
        if let Some(params) = child(node, 1) {
            self.bind_item(params)?;
        }
        if let Some(body) = child(node, 2) {
            self.bind_item(body)?;
        }
        self.fx_scope_bound(si);
        self.scope_counts.insert(si, self.scope_maximum);
        self.scope_maximum = maximum;
        self.scope_level = level;
        Ok(())
    }

    fn bind_access(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(sym) = child_sym(node, 0) {
            let scope = self.scope.unwrap();
            let resolved = self.scope_lookup(scope, &Sym::Named(sym.clone()), node.line, false, false);
            self.record_access(&sym, node.line, resolved);
        }
        Ok(())
    }

    /// `fxDeclareNodeBind` — a declaration node in the tree resolves its
    /// own symbol (so the coder learns its slot).
    fn bind_declare_node(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(sym) = child_sym(node, 0) {
            let scope = self.scope.unwrap();
            let resolved = self.scope_lookup(scope, &Sym::Named(sym.clone()), node.line, false, false);
            self.record_access(&sym, node.line, resolved);
        }
        Ok(())
    }

    fn bind_define(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(sym) = child_sym(node, 0) {
            let scope = self.scope.unwrap();
            let resolved = self.scope_lookup(scope, &Sym::Named(sym.clone()), node.line, false, false);
            self.record_access(&sym, node.line, resolved);
        }
        if let Some(init) = child(node, 1) {
            self.bind_item(init)?;
        }
        Ok(())
    }

    fn bind_assign(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0]=reference, children[1]=value
        if let Some(reference) = child(node, 0) {
            self.bind_item(reference)?;
        }
        if let Some(value) = child(node, 1) {
            self.bind_item(value)?;
        }
        Ok(())
    }

    fn bind_binding(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0]=target, children[1]=initializer
        if let Some(target) = child(node, 0) {
            self.bind_item(target)?;
        }
        if let Some(init) = child(node, 1) {
            self.bind_item(init)?;
        }
        Ok(())
    }

    fn bind_catch(&mut self, node: &Node) -> Result<(), ParseError> {
        let (scope, statement_scope) = self.scope_of(node);
        let has_param = matches!(child(node, 0), Some(Item::Node(_)));
        if has_param {
            let st = statement_scope.unwrap();
            self.fx_scope_binding(scope);
            if let Some(param) = child(node, 0) {
                self.bind_item(param)?;
            }
            self.fx_scope_binding(st);
            let disp = self.scopes[st].disposable_count > 0;
            if disp {
                self.push_variables(2);
            }
            if let Some(stmt) = child(node, 1) {
                self.bind_item(stmt)?;
            }
            if disp {
                // NOTE: XS's fxCatchNodeBind pushes (not pops) here too;
                // transliterated faithfully.
                self.push_variables(2);
            }
            self.fx_scope_bound(st);
            self.fx_scope_bound(scope);
        } else {
            // `scope` holds the statementScope when there is no parameter.
            self.fx_scope_binding(scope);
            let disp = self.scopes[scope].disposable_count > 0;
            if disp {
                self.push_variables(2);
            }
            if let Some(stmt) = child(node, 1) {
                self.bind_item(stmt)?;
            }
            if disp {
                self.push_variables(2);
            }
            self.fx_scope_bound(scope);
        }
        Ok(())
    }

    fn bind_for(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        let disp = self.scopes[si].disposable_count > 0;
        if disp {
            self.push_variables(2);
        }
        for i in 0..4 {
            if let Some(c) = child(node, i) {
                self.bind_item(c)?;
            }
        }
        if disp {
            self.pop_variables(2);
        }
        self.fx_scope_bound(si);
        Ok(())
    }

    fn bind_for_in_of(&mut self, node: &Node) -> Result<(), ParseError> {
        let (si, _) = self.scope_of(node);
        self.push_variables(6);
        self.fx_scope_binding(si);
        for i in 0..3 {
            if let Some(c) = child(node, i) {
                self.bind_item(c)?;
            }
        }
        self.fx_scope_bound(si);
        self.pop_variables(6);
        Ok(())
    }

    fn bind_switch(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(expr) = child(node, 0) {
            self.bind_item(expr)?;
        }
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        let disp = self.scopes[si].disposable_count > 0;
        if disp {
            self.push_variables(2);
        }
        if let Some(items) = child(node, 1) {
            self.bind_item(items)?;
        }
        if disp {
            self.pop_variables(2);
        }
        self.fx_scope_bound(si);
        Ok(())
    }

    fn bind_with(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(expr) = child(node, 0) {
            self.bind_item(expr)?;
        }
        let (si, _) = self.scope_of(node);
        self.fx_scope_binding(si);
        if let Some(stmt) = child(node, 1) {
            self.bind_item(stmt)?;
        }
        self.fx_scope_bound(si);
        Ok(())
    }

    fn bind_try(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(3);
        for i in 0..3 {
            if let Some(c) = child(node, i) {
                self.bind_item(c)?;
            }
        }
        self.pop_variables(3);
        Ok(())
    }

    fn bind_array(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(1);
        let spread = node.flags & flags::SPREAD != 0;
        if spread {
            self.push_variables(2);
        }
        self.bind_children(node)?;
        if spread {
            self.pop_variables(2);
        }
        self.pop_variables(1);
        Ok(())
    }

    fn bind_array_binding(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(6);
        self.bind_children(node)?;
        self.pop_variables(6);
        Ok(())
    }

    fn bind_object(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(1);
        self.bind_children(node)?;
        self.pop_variables(1);
        Ok(())
    }

    fn bind_object_binding(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(2);
        self.bind_children(node)?;
        self.pop_variables(2);
        Ok(())
    }

    fn bind_params(&mut self, node: &Node) -> Result<(), ParseError> {
        let spread = node.flags & flags::SPREAD != 0;
        if spread {
            self.push_variables(1);
            self.bind_children(node)?;
            self.pop_variables(1);
        } else {
            self.bind_children(node)?;
        }
        Ok(())
    }

    fn bind_params_binding(&mut self, node: &Node) -> Result<(), ParseError> {
        // The getter/setter/arity early errors and arguments mapping are a
        // coder concern; the scope-shape contract only needs the items
        // bound, so distribute them (the arity checks live in the parser
        // and coder children).
        self.bind_children(node)
    }

    fn bind_spread(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(1);
        self.bind_children(node)?;
        self.pop_variables(1);
        Ok(())
    }

    fn bind_delegate(&mut self, node: &Node) -> Result<(), ParseError> {
        self.push_variables(5);
        if let Some(expr) = child(node, 0) {
            self.bind_item(expr)?;
        }
        self.pop_variables(5);
        Ok(())
    }

    fn bind_template(&mut self, node: &Node) -> Result<(), ParseError> {
        // children[0]=reference (Null for untagged), children[1]=items
        let tagged = matches!(child(node, 0), Some(Item::Node(_)));
        if tagged {
            self.push_variables(2);
            self.bind_children(node)?;
            self.pop_variables(2);
        } else {
            self.bind_children(node)?;
        }
        Ok(())
    }

    fn bind_this_target(&mut self, _node: &Node) -> Result<(), ParseError> {
        self.scope_arrow(self.scope);
        Ok(())
    }

    fn bind_super(&mut self, node: &Node) -> Result<(), ParseError> {
        self.scope_arrow(self.scope);
        // children[0]=params (class instanceInit is folded)
        if let Some(params) = child(node, 0) {
            self.bind_item(params)?;
        }
        Ok(())
    }

    fn bind_postfix(&mut self, node: &Node) -> Result<(), ParseError> {
        if let Some(left) = child(node, 0) {
            self.bind_item(left)?;
        }
        self.push_variables(1);
        self.pop_variables(1);
        Ok(())
    }

    /// `fxExportNodeBind` (the local-export half) — resolve each exported
    /// local name and mark its declaration a closure|useClosure indirect
    /// binding, or raise `unknown variable` if it does not resolve. A
    /// re-export (`export … from`) is bound at load time, not here.
    fn bind_export(&mut self, node: &Node) -> Result<(), ParseError> {
        if matches!(child(node, 1), Some(Item::Node(_))) {
            return Ok(());
        }
        let scope = self.scope.unwrap();
        let specs: Vec<(String, u32)> = match child(node, 0) {
            Some(Item::List(v)) => v
                .iter()
                .filter_map(|it| match it {
                    Item::Node(spec) => child_sym(spec, 0).map(|s| (s, spec.line)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for (name, line) in specs {
            let sym = Sym::Named(name.clone());
            let resolved = self.scope_lookup(scope, &sym, line, false, false);
            match resolved {
                Some((si, id)) => {
                    let d = self.declare_mut(si, id);
                    d.flags |= dflags::CLOSURE | dflags::USE_CLOSURE;
                    self.record_access(&name, line, Some((si, id)));
                }
                None => return Err(err(line, "unknown variable")),
            }
        }
        Ok(())
    }
}

// ============================== dump ==============================

impl ScopeTree {
    /// Depth of scope `si` from the root, for indentation.
    fn depth(&self, mut si: usize) -> usize {
        let mut d = 0;
        while let Some(p) = self.scopes[si].parent {
            d += 1;
            si = p;
        }
        d
    }

    fn declare_pos(&self, si: usize, id: u32) -> Option<usize> {
        self.scopes[si].declares.iter().position(|x| x.id == id)
    }

    /// Render the scope tree and access log as a stable, readable dump —
    /// the fixture contract for the coder child. Scopes are labelled by
    /// arena index (`s0`, `s1`, …, creation order), indented by nesting;
    /// each declare shows its list position (the coder's slot order),
    /// kind, name, and flags; the access log shows each identifier's
    /// resolution.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for si in 0..self.scopes.len() {
            let sc = &self.scopes[si];
            let indent = "  ".repeat(self.depth(si));
            out.push_str(&format!("{}s{} {}", indent, si, scope_kind(sc.token)));
            if sc.flags & SCOPE_STRICT != 0 {
                out.push_str(" strict");
            }
            if sc.flags & SCOPE_EVAL != 0 {
                out.push_str(" eval");
            }
            if let Some(count) = self.scope_counts.get(&si) {
                out.push_str(&format!(" scopeCount={}", count));
            }
            out.push_str(&format!(" declareCount={}", sc.declare_count));
            if sc.closure_count != 0 {
                out.push_str(&format!(" closureCount={}", sc.closure_count));
            }
            if sc.arrow_default {
                out.push_str(" arrow-default");
            }
            out.push('\n');
            for (pos, d) in sc.declares.iter().enumerate() {
                out.push_str(&format!("{}  d{} {}", indent, pos, decl_kind(d.token)));
                match &d.symbol {
                    Some(Sym::Named(s)) => out.push_str(&format!(" {}", s)),
                    Some(Sym::Anon(n)) => out.push_str(&format!(" <anon{}>", n)),
                    None => out.push_str(" <null>"),
                }
                out.push_str(&decl_flags(d.flags));
                if let Some((asi, aid)) = d.alias {
                    let pos = self.declare_pos(asi, aid).unwrap_or(0);
                    out.push_str(&format!(" -> s{}:d{}", asi, pos));
                }
                out.push('\n');
            }
            for de in &sc.defines {
                let name = match &de.symbol {
                    Some(Sym::Named(s)) => s.clone(),
                    Some(Sym::Anon(n)) => format!("<anon{}>", n),
                    None => "<null>".to_string(),
                };
                out.push_str(&format!("{}  define {}\n", indent, name));
            }
        }
        if !self.accesses.is_empty() {
            out.push_str("--- accesses ---\n");
            for a in &self.accesses {
                match a.resolved {
                    Some((si, id)) => {
                        let pos = self.declare_pos(si, id).unwrap_or(0);
                        out.push_str(&format!("{} -> s{}:d{}\n", a.symbol, si, pos));
                    }
                    None => out.push_str(&format!("{} -> global\n", a.symbol)),
                }
            }
        }
        out
    }
}

/// The scope-kind spelling used in the dump.
fn scope_kind(token: Token) -> &'static str {
    match token {
        Token::Program => "PROGRAM",
        Token::Eval => "EVAL",
        Token::Module => "MODULE",
        Token::Function => "FUNCTION",
        Token::Block => "BLOCK",
        Token::With => "WITH",
        _ => node_name(token),
    }
}

/// The declare-kind spelling used in the dump.
fn decl_kind(token: Token) -> &'static str {
    match token {
        Token::Arg => "ARG",
        Token::Var => "VAR",
        Token::Let => "LET",
        Token::Const => "CONST",
        Token::Using => "USING",
        Token::Define => "DEFINE",
        Token::Private => "PRIVATE",
        Token::Specifier => "SPECIFIER",
        Token::NoToken => "alias",
        _ => node_name(token),
    }
}

fn decl_flags(flags: u32) -> String {
    let mut s = String::new();
    if flags & dflags::CLOSURE != 0 {
        s.push_str(" closure");
    }
    if flags & dflags::USE_CLOSURE != 0 {
        s.push_str(" useClosure");
    }
    if flags & dflags::DISPOSABLE != 0 {
        s.push_str(" disposable");
    }
    s
}

#[cfg(test)]
mod tests;
