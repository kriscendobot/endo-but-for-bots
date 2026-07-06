//! AST fixture tests for the expression grammar — the crate's
//! **dump-and-compare** bar. Each case parses an expression and asserts
//! its [`crate::ast::dump`] against a pinned S-expression that encodes
//! the XS tree shape (node kind, child order, flags) the byte-identity
//! coder downstream depends on. The dumps were read off
//! `c/moddable/xs/sources/xsSyntaxical.c` at the oracle pin, construct by
//! construct.

use crate::ast::dump;
use crate::parser::{ParseErrorKind, Parser};

/// Parse `src` as an assignment expression (sloppy mode) and dump it.
fn expr(src: &str) -> String {
    let mut p = Parser::new(src, false, false).unwrap_or_else(|e| panic!("lex {src:?}: {e}"));
    let item = p.parse_assignment_expression().unwrap_or_else(|e| panic!("parse {src:?}: {e}"));
    dump(&item)
}

/// Parse `src` as a comma expression (sloppy) and dump it.
fn comma(src: &str) -> String {
    let mut p = Parser::new(src, false, false).unwrap();
    let item = p.parse_comma_expression().unwrap_or_else(|e| panic!("parse {src:?}: {e}"));
    dump(&item)
}

/// A table of `(source, expected-dump)` pairs, asserted one by one.
fn check(cases: &[(&str, &str)]) {
    for (src, want) in cases {
        let got = expr(src);
        assert_eq!(&got, want, "\n  source:   {src}\n  expected: {want}\n  got:      {got}");
    }
}

#[test]
fn literals() {
    check(&[
        ("1", "(Integer 1)"),
        ("42", "(Integer 42)"),
        ("1.5", "(Number 1.5)"),
        ("3.14", "(Number 3.14)"),
        // XS canonicalizes any integer-valued literal — including `1e3`
        // — to XS_TOKEN_INTEGER (`fxGetNextNumber`), so this is an
        // Integer, not a Number.
        ("1e3", "(Integer 1000)"),
        ("0x10n", "(BigInt 10n/16)"),
        ("0b101n", "(BigInt 101n/2)"),
        ("\"hi\"", "(String \"hi\")"),
        ("''", "(String \"\")"),
        ("true", "(True)"),
        ("false", "(False)"),
        ("null", "(Null)"),
        ("this", "(This)"),
        ("x", "(Access #x)"),
    ]);
}

#[test]
fn member_and_call() {
    check(&[
        ("x.y", "(Member (Access #x) #y)"),
        ("x.y.z", "(Member (Member (Access #x) #y) #z)"),
        ("x[0]", "(MemberAt (Access #x) (Integer 0))"),
        ("x[a+b]", "(MemberAt (Access #x) (Add (Access #a) (Access #b)))"),
        ("x.#p", "(PrivateMember ##p (Access #x))"),
        ("f()", "(Call (Access #f) (Params []))"),
        ("f(1, 2)", "(Call (Access #f) (Params [(Integer 1) (Integer 2)]))"),
        ("f(...a)", "(Call (Access #f) (Params :spread [(Spread (Access #a))]))"),
        ("a.b(c)", "(Call (Member (Access #a) #b) (Params [(Access #c)]))"),
    ]);
}

#[test]
fn new_expressions() {
    check(&[
        ("new X", "(New (Access #X) (Params []))"),
        ("new X()", "(New (Access #X) (Params []))"),
        ("new X(1)", "(New (Access #X) (Params [(Integer 1)]))"),
        ("new a.b(1)", "(New (Member (Access #a) #b) (Params [(Integer 1)]))"),
        ("new X(1).y", "(Member (New (Access #X) (Params [(Integer 1)])) #y)"),
    ]);
}

#[test]
fn new_target_needs_function_context() {
    // At program top level, `mxTargetFlag` is unset, so `new.target` is
    // an early error — exactly as XS reports it.
    let mut p = Parser::new("new.target", false, false).unwrap();
    let err = p.parse_assignment_expression().unwrap_err();
    assert_eq!(err.kind, ParseErrorKind::Syntax);
    assert!(err.message.contains("new.target"), "{}", err.message);
}

#[test]
fn import_forms() {
    check(&[
        ("import.meta", "(ImportMeta)"),
        ("import(\"m\")", "(ImportCall (String \"m\") ())"),
        ("import(\"m\", o)", "(ImportCall (String \"m\") (Access #o))"),
    ]);
}

#[test]
fn optional_chaining() {
    check(&[
        ("a?.b", "(Chain (Member (Option (Access #a)) #b))"),
        ("a?.b.c", "(Chain (Member (Member (Option (Access #a)) #b) #c))"),
        ("a?.[0]", "(Chain (MemberAt (Option (Access #a)) (Integer 0)))"),
        ("a?.()", "(Chain (Call (Option (Access #a)) (Params [])))"),
        ("a?.#p", "(Chain (PrivateMember ##p (Option (Access #a))))"),
    ]);
}

#[test]
fn operators_precedence_and_associativity() {
    check(&[
        // multiplicative binds tighter than additive
        ("a+b*c", "(Add (Access #a) (Multiply (Access #b) (Access #c)))"),
        ("a*b+c", "(Add (Multiply (Access #a) (Access #b)) (Access #c))"),
        // exponentiation is right-associative
        ("a**b**c", "(Exponent (Access #a) (Exponent (Access #b) (Access #c)))"),
        // additive is left-associative
        ("a-b-c", "(Subtract (Subtract (Access #a) (Access #b)) (Access #c))"),
        ("a%b", "(Modulo (Access #a) (Access #b))"),
        ("a<<b", "(LeftShift (Access #a) (Access #b))"),
        ("a>>b", "(SignedRightShift (Access #a) (Access #b))"),
        ("a>>>b", "(UnsignedRightShift (Access #a) (Access #b))"),
        ("a<b", "(Less (Access #a) (Access #b))"),
        ("a<=b", "(LessEqual (Access #a) (Access #b))"),
        ("a instanceof b", "(Instanceof (Access #a) (Access #b))"),
        ("a in b", "(In (Access #a) (Access #b))"),
        ("a==b", "(Equal (Access #a) (Access #b))"),
        ("a!=b", "(NotEqual (Access #a) (Access #b))"),
        ("a===b", "(StrictEqual (Access #a) (Access #b))"),
        ("a!==b", "(StrictNotEqual (Access #a) (Access #b))"),
        ("a&b", "(BitAnd (Access #a) (Access #b))"),
        ("a^b", "(BitXor (Access #a) (Access #b))"),
        ("a|b", "(BitOr (Access #a) (Access #b))"),
        // && binds tighter than ||
        ("a&&b||c", "(Or (And (Access #a) (Access #b)) (Access #c))"),
        ("a??b", "(Coalesce (Access #a) (Access #b))"),
        // relational binds tighter than equality
        ("a<b==c", "(Equal (Less (Access #a) (Access #b)) (Access #c))"),
    ]);
}

#[test]
fn private_in() {
    // XS's descriptions table spells this node "PrivateIdenfifier"
    // (sic); the dump preserves XS's spelling for fidelity.
    check(&[("#x in obj", "(PrivateIdenfifier ##x (Access #obj))")]);
}

#[test]
fn unary_update() {
    check(&[
        ("!a", "(Not (Access #a))"),
        ("~a", "(BitNot (Access #a))"),
        ("+a", "(Plus (Access #a))"),
        ("-a", "(Minus (Access #a))"),
        ("typeof a", "(Typeof (Access #a))"),
        ("void 0", "(Void (Integer 0))"),
        ("delete a.b", "(Delete (Member (Access #a) #b))"),
        ("++a", "(Increment :novalue (Access #a))"),
        ("--a", "(Decrement :novalue (Access #a))"),
        ("a++", "(Increment (Access #a))"),
        ("a--", "(Decrement (Access #a))"),
    ]);
}

#[test]
fn conditional_and_assignment() {
    check(&[
        ("a?b:c", "(QuestionMark (Access #a) (Access #b) (Access #c))"),
        ("a=b", "(Assign (Access #a) (Access #b))"),
        // assignment is right-associative
        ("a=b=c", "(Assign (Access #a) (Assign (Access #b) (Access #c)))"),
        ("a+=b", "(AddAssign (Access #a) (Access #b))"),
        ("a-=b", "(SubtractAssign (Access #a) (Access #b))"),
        ("a*=b", "(MultiplyAssign (Access #a) (Access #b))"),
        ("a**=b", "(ExponentAssign (Access #a) (Access #b))"),
        ("a&&=b", "(AndAssign (Access #a) (Access #b))"),
        ("a||=b", "(OrAssign (Access #a) (Access #b))"),
        ("a??=b", "(CoalesceAssign (Access #a) (Access #b))"),
        ("a>>>=b", "(UnsignedRightShiftAssign (Access #a) (Access #b))"),
        // member and computed targets are valid references
        ("a.b=c", "(Assign (Member (Access #a) #b) (Access #c))"),
        ("a[i]=c", "(Assign (MemberAt (Access #a) (Access #i)) (Access #c))"),
    ]);
}

#[test]
fn comma_expressions() {
    assert_eq!(comma("a,b,c"), "(Expressions [(Access #a) (Access #b) (Access #c)])");
    // a lone expression through the comma entry is not wrapped
    assert_eq!(comma("a"), "(Access #a)");
}

#[test]
fn parenthesized() {
    check(&[
        ("(a)", "(Expressions [(Access #a)])"),
        ("(a,b)", "(Expressions [(Access #a) (Access #b)])"),
        ("(a+b)*c", "(Multiply (Expressions [(Add (Access #a) (Access #b))]) (Access #c))"),
    ]);
    // a parenthesized single reference is a valid assignment target
    // (fxCheckReference unwraps the cover).
    check(&[("(a)=b", "(Assign (Access #a) (Access #b))")]);
}

#[test]
fn array_literals() {
    check(&[
        ("[]", "(Array [])"),
        ("[1, 2]", "(Array [(Integer 1) (Integer 2)])"),
        ("[1, , 2]", "(Array [(Integer 1) (?) (Integer 2)])"),
        ("[, a]", "(Array [(?) (Access #a)])"),
        ("[1, ]", "(Array :elision [(Integer 1)])"),
        ("[...x]", "(Array :spread [(Spread (Access #x))])"),
        (
            "[1, , 2, ...x]",
            "(Array :spread [(Integer 1) (?) (Integer 2) (Spread (Access #x))])",
        ),
    ]);
}

#[test]
fn object_literals() {
    check(&[
        ("({})", "(Expressions [(Object [])])"),
        ("({a: 1})", "(Expressions [(Object [(Property #a (Integer 1))])])"),
        ("({b})", "(Expressions [(Object [(Property :shorthand #b (Access #b))])])"),
        (
            "({c = 2})",
            "(Expressions [(Object [(Property :shorthand #c (Binding (Access #c) (Integer 2)))])])",
        ),
        ("({[d]: 3})", "(Expressions [(Object [(PropertyAt (Access #d) (Integer 3))])])"),
        ("({\"s\": 1})", "(Expressions [(Object [(Property #s (Integer 1))])])"),
        ("({0: 1})", "(Expressions [(Object [(PropertyAt (Integer 0) (Integer 1))])])"),
        ("({...e})", "(Expressions [(Object [(Spread (Access #e))])])"),
    ]);
}

#[test]
fn templates() {
    check(&[
        ("`abc`", "(Template () [(TemplateItem (String \"abc\") (String \"abc\"))])"),
        (
            "`a${x}b`",
            "(Template () [(TemplateItem (String \"a\") (String \"a\")) (Access #x) (TemplateItem (String \"b\") (String \"b\"))])",
        ),
        (
            "tag`a${x}b`",
            "(Template (Access #tag) [(TemplateItem (String \"a\") (String \"a\")) (Access #x) (TemplateItem (String \"b\") (String \"b\"))])",
        ),
        (
            "f()`x`",
            "(Template (Call (Access #f) (Params [])) [(TemplateItem (String \"x\") (String \"x\"))])",
        ),
    ]);
}

#[test]
fn regexp_vs_divide() {
    check(&[
        // division: `/` in operator position
        ("a/b", "(Divide (Access #a) (Access #b))"),
        ("a/=b", "(DivideAssign (Access #a) (Access #b))"),
        // regexp literal: `/` in expression position, delimited by the
        // parser calling back into the lexer
        ("/re/gi", "(Regexp (String \"gi\") (String \"re\"))"),
        ("/a\\/b/", "(Regexp (String \"\") (String \"a\\\\/b\"))"),
    ]);
}

#[test]
fn deferred_constructs_report_unsupported_not_panic() {
    // Arrow / function / class expressions, object methods, and
    // destructuring assignment targets are deferred to the
    // statement-grammar child; they must classify as Unsupported, never
    // panic or mis-parse.
    for src in ["x => x", "(a, b) => a", "function () {}", "class {}", "({ m() {} })", "[a] = b"] {
        let mut p = Parser::new(src, false, false).unwrap();
        match p.parse_assignment_expression() {
            Err(e) => assert_eq!(e.kind, ParseErrorKind::Unsupported, "src {src:?}: {e}"),
            Ok(item) => panic!("expected Unsupported for {src:?}, got {}", dump(&item)),
        }
    }
}

#[test]
fn malformed_input_never_panics() {
    // The fuzz target (a later child) depends on this: every byte
    // sequence yields a Result, never a panic.
    for src in ["", "(", ")", "1 +", "a.", "a?.", "[", "{", "`", "/", "@#$", "1n.2", "a ** ** b"] {
        let mut p = match Parser::new(src, false, false) {
            Ok(p) => p,
            Err(_) => continue, // a lex error before the first token is fine
        };
        let _ = p.parse_comma_expression(); // Ok or Err, but no panic
    }
}

#[test]
fn parse_meter_accrues_per_token() {
    // The meter (endor's own frozen cost table) advances monotonically
    // across a parse; a larger expression costs at least as much.
    let mut small = Parser::new("a", false, false).unwrap();
    small.parse_comma_expression().unwrap();
    let mut big = Parser::new("a + b * (c - d)", false, false).unwrap();
    big.parse_comma_expression().unwrap();
    assert!(big.meter().computrons() >= small.meter().computrons());
    assert!(small.meter().computrons() > 0);
}

#[test]
fn await_in_module_context() {
    // In a module (async top-level), `await x` is a unary Await with the
    // awaiting flag threaded onto the parser state.
    // In module mode the parser carries mxStrictFlag|mxAsyncFlag, and
    // `fxPushNodeStruct` stamps those inherited bits onto every node it
    // builds — so the dump faithfully shows `:strict :async`.
    let mut p = Parser::new("await x", false, true).unwrap();
    let item = p.parse_assignment_expression().unwrap();
    assert_eq!(dump(&item), "(Await :strict :async (Access :strict :async #x))");
}

#[test]
fn no_reference_is_a_syntax_error() {
    // Assigning into a non-reference (a literal) is an early error, not
    // a panic.
    let mut p = Parser::new("1 = 2", false, false).unwrap();
    let err = p.parse_assignment_expression().unwrap_err();
    assert_eq!(err.kind, ParseErrorKind::Syntax);
}
