//! The abstract-syntax-tree model, mirroring XS's `txNode` kinds and
//! layouts from `c/moddable/xs/sources/xsScript.h` at the oracle pin
//! (design § roadmap row 5; Design Decisions 4 and 5).
//!
//! This is **not** a "nicer" AST: the scoper and coder downstream are
//! held to byte-identity against C-XS, and the exact tree shapes — which
//! node kind XS synthesizes for each construct, the order of a node's
//! child slots, and the parser flags stamped onto a node — are the spec.
//! So a node here carries the same `token` (its `description->token`),
//! the same `line`, the same `flags` bits, and its children in XS's own
//! slot order.
//!
//! XS builds the tree on a pointer stack (`fxPushNode` / `fxPopNode` /
//! `fxPushNodeStruct`, `xsSyntaxical.c`). A stack slot — and therefore a
//! node's child slot — is one of four things, captured by [`Item`]: a
//! real node, a bare symbol (XS's `fxPushSymbol`, aliased into a
//! `txNode*` slot), a `NULL` placeholder (`fxPushNULL`), or a node list
//! (`fxPushNodeList`, a `txNodeList` with the `XS_NO_TOKEN` description).
//! We model those four cases explicitly rather than reproduce the C
//! pointer-aliasing trick.

use crate::token::Token;

/// The parser-flag bits XS stamps onto nodes (`xsScript.h` `enum`). Only
/// the bits the expression grammar sets are named here; the rest arrive
/// with the scoper/coder stages. Values are XS's exact bit positions so
/// a node's `flags` word matches C-XS.
pub mod flags {
    /// `mxCFlag` — host (`@`) mode. (bit 0)
    pub const C: u32 = 1 << 0;
    /// `mxEvalFlag`. (bit 2)
    pub const EVAL: u32 = 1 << 2;
    /// `mxStrictFlag`. (bit 4)
    pub const STRICT: u32 = 1 << 4;
    /// `mxSuperFlag`. (bit 5)
    pub const SUPER: u32 = 1 << 5;
    /// `mxTargetFlag`. (bit 6)
    pub const TARGET: u32 = 1 << 6;
    /// `mxArgumentsFlag`. (bit 7)
    pub const ARGUMENTS: u32 = 1 << 7;
    /// `mxArrowFlag`. (bit 8)
    pub const ARROW: u32 = 1 << 8;
    /// `mxAsyncFlag`. (bit 9)
    pub const ASYNC: u32 = 1 << 9;
    /// `mxAwaitingFlag`. (bit 10)
    pub const AWAITING: u32 = 1 << 10;
    /// `mxBaseFlag`. (bit 11)
    pub const BASE: u32 = 1 << 11;
    /// `mxDerivedFlag`. (bit 17)
    pub const DERIVED: u32 = 1 << 17;
    /// `mxElisionFlag`. (bit 18)
    pub const ELISION: u32 = 1 << 18;
    /// `mxExpressionNoValue`. (bit 19)
    pub const EXPRESSION_NO_VALUE: u32 = 1 << 19;
    /// `mxForFlag`. (bit 20)
    pub const FOR: u32 = 1 << 20;
    /// `mxGeneratorFlag`. (bit 21)
    pub const GENERATOR: u32 = 1 << 21;
    /// `mxGetterFlag`. (bit 22)
    pub const GETTER: u32 = 1 << 22;
    /// `mxMethodFlag`. (bit 23)
    pub const METHOD: u32 = 1 << 23;
    /// `mxSetterFlag`. (bit 25)
    pub const SETTER: u32 = 1 << 25;
    /// `mxShorthandFlag`. (bit 26)
    pub const SHORTHAND: u32 = 1 << 26;
    /// `mxSpreadFlag`. (bit 27)
    pub const SPREAD: u32 = 1 << 27;
    /// `mxStaticFlag`. (bit 28)
    pub const STATIC: u32 = 1 << 28;
    /// `mxYieldFlag`. (bit 30)
    pub const YIELD: u32 = 1 << 30;
    /// `mxYieldingFlag`. (bit 31)
    pub const YIELDING: u32 = 1 << 31;

    /// The subset XS's `fxPushNodeStruct` copies from `parser->flags`
    /// onto every freshly built node (`mxStrictFlag | mxGeneratorFlag |
    /// mxAsyncFlag`).
    pub const INHERITED: u32 = STRICT | GENERATOR | ASYNC;
}

/// A leaf value a node can carry in place of (or beside) children — the
/// payload of XS's value-bearing node structs.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No payload (the common case; all children).
    None,
    /// `txIntegerNode.value`.
    Integer(i32),
    /// `txNumberNode.value`.
    Number(f64),
    /// `txStringNode.value` (cooked). Length is `value.chars().count()`
    /// on the endor side (UTF-8, not CESU-8).
    Str(String),
    /// `txBigIntNode` — the scanned literal (digits + radix).
    BigInt(crate::lexer::BigIntLiteral),
}

/// One stack slot / child slot: the four things XS can push.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A real AST node.
    Node(Box<Node>),
    /// A bare symbol (`fxPushSymbol`).
    Symbol(String),
    /// A `NULL` placeholder (`fxPushNULL`).
    Null,
    /// A node list (`fxPushNodeList`), its elements in source order.
    List(Vec<Item>),
}

/// An AST node: XS's `sxNode` common part (`description->token`, `line`,
/// `flags`) plus its child slots and any leaf payload.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The node kind — `description->token` in C-XS.
    pub token: Token,
    /// 1-based source line, XS's `node->line`.
    pub line: u32,
    /// The `node->flags` word (see [`flags`]).
    pub flags: u32,
    /// Child slots, in XS's field order (`children[0]` is the
    /// first-pushed / deepest-on-stack slot).
    pub children: Vec<Item>,
    /// Leaf payload for value-bearing nodes.
    pub value: Value,
}

impl Node {
    /// A childless node of `token` at `line` with no flags.
    pub fn leaf(token: Token, line: u32) -> Node {
        Node { token, line, flags: 0, children: Vec::new(), value: Value::None }
    }
}

/// The display name XS gives a node kind (`txNodeDescription.name` in
/// `gxTokenDescriptions`, `xsTree.c`). Used by the fixture dump. Tokens
/// that are never node kinds map to their own debug name; the ones that
/// matter are the node-producing tokens, whose names match C-XS exactly
/// (including XS's spellings `Exponent`, `PrivateIdenfifier`).
pub fn node_name(token: Token) -> &'static str {
    use Token::*;
    match token {
        Access => "Access",
        Add => "Add",
        AddAssign => "AddAssign",
        And => "And",
        AndAssign => "AndAssign",
        Arg => "Arg",
        Arguments => "Arguments",
        Array => "Array",
        ArrayBinding => "ArrayBinding",
        Assign => "Assign",
        Await => "Await",
        Bigint => "BigInt",
        Binding => "Binding",
        BitAnd => "BitAnd",
        BitAndAssign => "BitAndAssign",
        BitNot => "BitNot",
        BitOr => "BitOr",
        BitOrAssign => "BitOrAssign",
        BitXor => "BitXor",
        BitXorAssign => "BitXorAssign",
        Call => "Call",
        Chain => "Chain",
        Class => "Class",
        Coalesce => "Coalesce",
        CoalesceAssign => "CoalesceAssign",
        Const => "Const",
        Current => "Current",
        Decrement => "Decrement",
        Define => "Define",
        Delegate => "Delegate",
        Delete => "Delete",
        Divide => "Divide",
        DivideAssign => "DivideAssign",
        Equal => "Equal",
        Exponentiation => "Exponent",
        ExponentiationAssign => "ExponentAssign",
        Expressions => "Expressions",
        False => "False",
        Field => "Field",
        Function => "Function",
        Generator => "Generator",
        Getter => "Getter",
        Host => "Host",
        Import => "Import",
        ImportCall => "ImportCall",
        ImportMeta => "ImportMeta",
        In => "In",
        Increment => "Increment",
        Instanceof => "Instanceof",
        Integer => "Integer",
        Let => "Let",
        Member => "Member",
        MemberAt => "MemberAt",
        Minus => "Minus",
        Modulo => "Modulo",
        ModuloAssign => "ModuloAssign",
        More => "More",
        MoreEqual => "MoreEqual",
        Multiply => "Multiply",
        MultiplyAssign => "MultiplyAssign",
        LeftShift => "LeftShift",
        LeftShiftAssign => "LeftShiftAssign",
        Less => "Less",
        LessEqual => "LessEqual",
        New => "New",
        Not => "Not",
        NotEqual => "NotEqual",
        Null => "Null",
        Number => "Number",
        Object => "Object",
        ObjectBinding => "ObjectBinding",
        Option => "Option",
        Or => "Or",
        OrAssign => "OrAssign",
        Params => "Params",
        ParamsBinding => "ParamsBinding",
        Plus => "Plus",
        PrivateIdentifier => "PrivateIdenfifier",
        PrivateMember => "PrivateMember",
        PrivateProperty => "PrivateProperty",
        Property => "Property",
        PropertyAt => "PropertyAt",
        PropertyBinding => "PropertyBinding",
        PropertyBindingAt => "PropertyBindingAt",
        QuestionMark => "QuestionMark",
        Regexp => "Regexp",
        RestBinding => "RestBinding",
        SignedRightShift => "SignedRightShift",
        SignedRightShiftAssign => "SignedRightShiftAssign",
        SkipBinding => "SkipBinding",
        Spread => "Spread",
        StrictEqual => "StrictEqual",
        StrictNotEqual => "StrictNotEqual",
        String => "String",
        Subtract => "Subtract",
        SubtractAssign => "SubtractAssign",
        Super => "Super",
        Target => "Target",
        Template => "Template",
        TemplateMiddle => "TemplateItem",
        This => "This",
        True => "True",
        Typeof => "Typeof",
        Undefined => "Undefined",
        UnsignedRightShift => "UnsignedRightShift",
        UnsignedRightShiftAssign => "UnsignedRightShiftAssign",
        Void => "Void",
        Yield => "Yield",
        _ => "?",
    }
}

/// The flag bits worth surfacing in a fixture dump, and their short
/// spellings. Order is deterministic (low bit first).
const DUMP_FLAGS: &[(u32, &str)] = &[
    (flags::STRICT, "strict"),
    (flags::SUPER, "super"),
    (flags::TARGET, "target"),
    (flags::ARGUMENTS, "arguments"),
    (flags::ARROW, "arrow"),
    (flags::ASYNC, "async"),
    (flags::AWAITING, "awaiting"),
    (flags::DERIVED, "derived"),
    (flags::ELISION, "elision"),
    (flags::EXPRESSION_NO_VALUE, "novalue"),
    (flags::GENERATOR, "generator"),
    (flags::GETTER, "getter"),
    (flags::METHOD, "method"),
    (flags::SETTER, "setter"),
    (flags::SHORTHAND, "shorthand"),
    (flags::SPREAD, "spread"),
    (flags::STATIC, "static"),
];

/// Render a stack item as a stable, one-line-per-node S-expression, the
/// **dump** side of the crate's dump-and-compare fixture tests. The
/// format is endor's own (it is NOT XS's debug `mxTreePrint`, which is
/// not part of the byte-identity bar); it exists to pin the tree *shape*
/// — node kind, flags, child order — that the coder will later depend
/// on. Deterministic and free of addresses so fixtures are stable.
pub fn dump(item: &Item) -> String {
    let mut out = String::new();
    dump_item(item, &mut out);
    out
}

fn dump_item(item: &Item, out: &mut String) {
    match item {
        Item::Null => out.push_str("()"),
        Item::Symbol(s) => {
            out.push_str("#");
            out.push_str(s);
        }
        Item::List(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                dump_item(it, out);
            }
            out.push(']');
        }
        Item::Node(node) => dump_node(node, out),
    }
}

fn dump_node(node: &Node, out: &mut String) {
    out.push('(');
    out.push_str(node_name(node.token));
    for (bit, name) in DUMP_FLAGS {
        if node.flags & bit != 0 {
            out.push(' ');
            out.push(':');
            out.push_str(name);
        }
    }
    match &node.value {
        Value::None => {}
        Value::Integer(v) => {
            out.push_str(" ");
            out.push_str(&v.to_string());
        }
        Value::Number(v) => {
            out.push_str(" ");
            out.push_str(&dump_number(*v));
        }
        Value::Str(s) => {
            out.push_str(" ");
            out.push_str(&dump_string(s));
        }
        Value::BigInt(b) => {
            out.push_str(" ");
            out.push_str(&b.digits);
            out.push_str("n/");
            out.push_str(&b.radix.to_string());
        }
    }
    for child in &node.children {
        out.push(' ');
        dump_item(child, out);
    }
    out.push(')');
}

fn dump_number(v: f64) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v < 0.0 { "-Infinity".to_string() } else { "Infinity".to_string() }
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

fn dump_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
