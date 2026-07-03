// Stage-3 child-5 (collections) curated corpus: Map/Set/WeakMap/WeakSet, the
// XS exotic-collection hashing and entry-slot allocation shapes, metered
// faithfully — every entry allocation is `fxNewSlot`-visible and every rehash
// an `fxNewChunk`, so the computron count tracks the exact allocation
// sequence (xsMapSet.c calls no `mxMeter`). Bit-exact (completion value AND
// computron count) against the C-XS oracle at the pin 48ee02d8cfe0. One JS
// program per line; the last expression is the completion value.

// --- Map construction (fxNewMapInstance: 4 slots + address chunk) ---
new Map()
var m=new Map(); m
var m=new Map(); m.size

// --- Map.set / get / has (fxSetEntry new key: 3 slots + fxResizeEntries) ---
var m=new Map(); m.set(1,2); m.get(1)
var m=new Map(); m.set(1,2); m.has(1)
var m=new Map(); m.set(1,2); m.has(2)
var m=new Map(); m.set(1,2); m.get(9)
var m=new Map(); m.set(1,2); m.size
var m=new Map(); m.set("a",10); m.get("a")
var m=new Map(); m.set("hello",1); m.set("world",2); m.get("world")
var m=new Map(); m.set(true,1); m.set(false,2); m.get(false)

// --- set returns the map (chaining) ---
var m=new Map(); m.set(1,1).set(2,2).set(3,3); m.size
var m=new Map(); m.set(0,"z").get(0)

// --- in-place update (existing key: no allocation) ---
var m=new Map(); m.set(1,10); m.set(1,20); m.get(1)
var m=new Map(); m.set(1,10); m.set(1,20); m.size
var m=new Map(); m.set("k",1); m.set("k",2); m.set("k",3); m.get("k")

// --- growth across rehash boundaries (table 1->2->4->8->16 ...) ---
var m=new Map(); for(var i=0;i<8;i++){m.set(i,i);} m.size
var m=new Map(); for(var i=0;i<8;i++){m.set(i,i*2);} m.get(5)
var m=new Map(); for(var i=0;i<32;i++){m.set(i,i);} m.size
var m=new Map(); for(var i=0;i<50;i++){m.set(i,i+1);} m.get(49)
var m=new Map(); var t=0; for(var i=0;i<20;i++){m.set(i,i);} for(var j=0;j<20;j++){t+=m.get(j);} t

// --- key equality: SameValueZero (NaN equals NaN, -0 is +0) ---
var m=new Map(); m.set(NaN,7); m.get(NaN)
var m=new Map(); m.set(NaN,7); m.has(NaN)
var m=new Map(); m.set(-0,5); m.get(0)
var m=new Map(); m.set(0,5); m.get(-0)
var m=new Map(); m.set(-0,5); m.size

// --- delete (fxDeleteEntry + fxResizeEntries shrink) ---
var m=new Map(); m.delete(1)
var m=new Map(); m.set(1,2); m.delete(1)
var m=new Map(); m.set(1,2); m.delete(9)
var m=new Map(); m.set(1,2); m.delete(1); m.size
var m=new Map(); m.set(1,2); m.delete(1); m.has(1)
var m=new Map(); for(var i=0;i<10;i++){m.set(i,i);} for(var j=0;j<5;j++){m.delete(j);} m.size

// --- object keys (reference identity) ---
var a={}; var b={}; var m=new Map(); m.set(a,1); m.set(b,2); m.get(a)
var a={}; var m=new Map(); m.set(a,1); m.set(a,2); m.size
var a=[1,2]; var m=new Map(); m.set(a,"arr"); m.get(a)

// --- Set construction and add / has / size (fxSetEntry no pair: 2 slots) ---
new Set()
var s=new Set(); s.size
var s=new Set(); s.add(1); s.has(1)
var s=new Set(); s.add(1); s.has(2)
var s=new Set(); s.add(1); s.size
var s=new Set(); s.add(1).add(2).add(3); s.size
var s=new Set(); s.add("x"); s.add("y"); s.has("y")
var s=new Set(); s.add(1); s.add(1); s.add(1); s.size
var s=new Set(); s.add(NaN); s.add(NaN); s.size
var s=new Set(); s.add(-0); s.has(0)

// --- Set growth and delete ---
var s=new Set(); for(var i=0;i<16;i++){s.add(i);} s.size
var s=new Set(); for(var i=0;i<40;i++){s.add(i%13);} s.size
var s=new Set(); s.add(1); s.add(2); s.delete(1)
var s=new Set(); s.add(1); s.delete(9)
var s=new Set(); s.add(1); s.add(2); s.delete(1); s.size
var s=new Set(); for(var i=0;i<10;i++){s.add(i);} s.delete(5); s.has(5)

// --- WeakMap (fxNewWeakMapInstance: 2 slots; fxSetWeakEntry: 3 slots) ---
new WeakMap()
var o={}; var w=new WeakMap(); w.has(o)
var o={}; var w=new WeakMap(); w.set(o,5); w.get(o)
var o={}; var w=new WeakMap(); w.set(o,5); w.has(o)
var o={}; var w=new WeakMap(); w.set(o,5); w.set(o,9); w.get(o)
var o={}; var w=new WeakMap(); w.set(o,5); w.delete(o)
var o={}; var w=new WeakMap(); w.set(o,5); w.delete(o); w.has(o)
var a={}; var b={}; var w=new WeakMap(); w.set(a,1); w.set(b,2); w.get(b)

// --- WeakSet (fxSetWeakEntry: 3 slots) ---
new WeakSet()
var o={}; var s=new WeakSet(); s.has(o)
var o={}; var s=new WeakSet(); s.add(o); s.has(o)
var o={}; var s=new WeakSet(); s.add(o); s.add(o); s.has(o)
var o={}; var s=new WeakSet(); s.add(o); s.delete(o)
var o={}; var s=new WeakSet(); s.add(o); s.delete(o); s.has(o)
var a={}; var b={}; var s=new WeakSet(); s.add(a); s.has(b)
