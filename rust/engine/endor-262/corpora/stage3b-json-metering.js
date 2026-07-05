// Stage-3b (json-metering) — structured JSON.stringify corpus: object and
// array values, bit-exact (result AND computron) against the C-XS oracle at the
// pin `48ee02d8cfe0`. The serializer builds into the unmetered C-malloc buffer;
// what meters is the recursive `fxStringifyJSONProperty` per-node cost (the
// keys-list instance and its per-key AT slots, the per-iteration bodies, and
// each key's `fxPushKeyString` chunk) plus the final result `fxNewChunk`. Every
// constant is a whole number of `mxMeterOne` steps plus the exact
// `fxNewSlot`/`fxNewChunk` allocations the pin makes — decomposed, not fitted
// (interp.rs § JSON_STRINGIFY_* ).
//
// A callable value (function) and a `toJSON`/wrapper/replacer/space corner
// self-name an honest NAMED skip (never a wrong value, never a divergence).

// empty containers — the node-enter cost, no keys/elements.
JSON.stringify({})
JSON.stringify([])

// flat objects — per-key body + AT slot + key chunk + the leaf.
JSON.stringify({a:1})
JSON.stringify({a:1,b:2})
JSON.stringify({a:1,b:2,c:3})
JSON.stringify({name:"John",age:30})
JSON.stringify({t:true,f:false,n:null})

// key-length dependence — the fxPushKeyString chunk rounds to 8-byte alignment.
JSON.stringify({abcdefgh:1})
JSON.stringify({abcdefghijklmnop:1})

// flat arrays — per-element body + the leaf.
JSON.stringify([1])
JSON.stringify([1,2,3])
JSON.stringify([true,false,null])
JSON.stringify(["a","bb","ccc"])
JSON.stringify([1,2,3,4,5,6,7,8,9,10])

// undefined slots — an undefined object value is elided; an undefined/hole
// array element serializes as null. The iteration body still meters.
JSON.stringify({a:undefined})
JSON.stringify({a:1,b:undefined,c:3})
JSON.stringify([undefined,null,1])
JSON.stringify([1,,3])

// nesting — the recursion composes; a child node pays its own enter cost.
JSON.stringify({a:{}})
JSON.stringify({a:[]})
JSON.stringify([{}])
JSON.stringify([[]])
JSON.stringify({a:{b:1}})
JSON.stringify([[1]])
JSON.stringify({a:1,b:{c:2}})
JSON.stringify([1,[2,3],4])
JSON.stringify({x:[1,{y:2}],z:"hi"})
JSON.stringify({nested:{deep:{arr:[1,2,{k:"v"}]}}})

// strings inside containers — the JSON escaper over element/value strings.
JSON.stringify({s:"a\"b",t:"tab\there"})
JSON.stringify(["line\nbreak","ret\rurn"])

// a stringify of a computed structure (the value comes from prior ops).
JSON.stringify({sum:1+2,txt:"a".concat("b")})

// === JSON.parse (fx_JSON_parse tokenizer + per-node allocation metering) ===
// The parse path calls no mxMeter; every unit is the native frame residual
// plus the exact fxNewSlot/fxNewChunk allocations. Bit-exact (result AND
// computron) against the pin: the setup residual, the string tokenizer chunk,
// fxNewArrayInstance's two slots + one linked slot per element + the
// fxCacheArray item chunk, and fxNewObjectInstance's slot + the per-member body
// + key intern + key chunk.

// primitives — the value-independent setup; a string adds its tokenizer chunk.
JSON.parse("1")
JSON.parse("-42")
JSON.parse("0")
JSON.parse("1.5")
JSON.parse("1e3")
JSON.parse("3.14159e-2")
JSON.parse("3000000000")
JSON.parse("true")
JSON.parse("false")
JSON.parse("null")
JSON.parse("\"hello\"")
JSON.parse("\"\"")
JSON.parse("\"tab\\tnl\\nq\\\"\"")
JSON.parse("\"\\u0041\\u00e9\"")

// arrays — instance slots + per-element linked slot + the fxCacheArray chunk.
JSON.parse("[]")
JSON.parse("[1]")
JSON.parse("[1,2,3,4,5]")
JSON.parse("[true,false,null]")
JSON.parse("[\"a\",\"bb\",\"ccc\"]")
JSON.parse(" [ 1 , 2 , 3 ] ")
JSON.parse("[[],[1],[2,3]]")
JSON.parse("[1,[2,[3,[4]]]]")

// objects — instance slot + per-member body + key intern + key chunk.
JSON.parse("{}")
JSON.parse("{\"a\":1}")
JSON.parse("{\"a\":1,\"b\":2,\"c\":3}")
JSON.parse("{\"name\":\"John\",\"age\":30,\"ok\":true}")
JSON.parse("{\"longkeyname\":1}")
JSON.parse("{\"a\":[1,2],\"b\":{\"c\":3}}")
JSON.parse("{\"nested\":{\"deep\":{\"x\":[1,2,3]}}}")

// parse results observed by value (property access / length / arithmetic).
JSON.parse("[10,20,30]").length
JSON.parse("{\"x\":42}").x
JSON.stringify(JSON.parse("[1,2,3]"))
