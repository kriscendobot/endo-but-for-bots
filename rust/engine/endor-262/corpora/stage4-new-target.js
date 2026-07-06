// Stage-4 child 2/8 (the landed slice): `new.target` (the XS_CODE_TARGET
// opcode) with real semantics. Each line is one program, bit-exact (result
// AND computron) against the C-XS pin. `new.target` is the target constructor
// when the running frame was entered as a construct (`mxFrameHasTarget`) and
// `undefined` otherwise; endor reads it from the frame's (cur_target, cur_func)
// pair — for a `new f()` the target IS the invoked constructor (there is no
// Reflect.construct / super() retargeting in the covered grammar). The wider
// `class` family (definition/methods/extends/super) is a reported scope fold,
// self-naming honest skips.

// --- new.target inside a constructor call is the constructor ---
var t; function F(){ t = new.target; } new F(); t === F;
var t; function F(){ t = new.target; } new F(); typeof t;
function F(){ return typeof new.target; } var r = new F(); typeof r;

// --- new.target inside a plain call is undefined ---
var t; function F(){ t = new.target; } F(); typeof t;
var t; function F(){ t = new.target; } F(); t === undefined;
function F(){ return new.target === undefined; } F();
function F(){ return typeof new.target; } var r = F(); r;

// --- the factory guard idiom: branch on whether `new` was used ---
function F(){ if (new.target === undefined) { return 99; } this.x = 1; } F();
var o; function F(){ if (new.target === undefined) { return 99; } this.x = 7; } o = new F(); o.x;
function F(){ return new.target ? 1 : 2; } F();
var o; function F(){ this.k = new.target ? 7 : 0; } o = new F(); o.k;

// --- new.target === F self-identity, both call shapes ---
function F(){ return new.target === F; } new F();
function F(){ return new.target === F; } F();

// --- a closure-captured constructor still sees new.target ---
var t; function make(){ function G(){ t = new.target; } return G; } var g = make(); new g(); t === g;

// --- new.target is per-frame: a construct then a plain call ---
var t; function F(){ t = new.target; } new F(); F(); t === undefined;
