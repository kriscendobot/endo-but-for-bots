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

// === Stage-3b: keyed-collection iteration (child 1/9 remainder) ===
// The iteration protocol built on the stage-3 array iterators: Map/Set
// entries/keys/values, forEach, and for-of / spread. Bit-exact (completion
// value AND computron count) against the pin 48ee02d8cfe0. The iterator
// creation cluster, the per-entries-yield pair construction, and the forEach
// per-entry call frame are each modeled to the exact allocation/frame cost.

// --- Map.prototype.forEach (fx_Map_prototype_forEach: (value, key, map)) ---
var m=new Map(); m.set(1,2); var r=0; m.forEach(function(v,k){r+=v;}); r
var m=new Map(); m.set(1,10); m.set(2,20); m.set(3,30); var r=0; m.forEach(function(v){r+=v;}); r
var m=new Map(); m.set("a",1); m.set("b",2); var ks=""; m.forEach(function(v,k){ks+=k;}); ks
var m=new Map(); m.set(1,2); var self=0; m.forEach(function(v,k,mm){self=(mm===m)?1:0;}); self
var m=new Map(); var r=0; m.forEach(function(v){r+=v;}); r
var m=new Map(); m.set(1,5); var t={n:100}; var got=0; m.forEach(function(v){got=this.n;},t); got

// --- Set.prototype.forEach (fx_Set_prototype_forEach: (value, value, set)) ---
var s=new Set(); s.add(1); var r=0; s.forEach(function(v){r+=v;}); r
var s=new Set(); s.add(1); s.add(2); s.add(3); var r=0; s.forEach(function(v){r+=v;}); r
var s=new Set(); s.add(4); var same=0; s.forEach(function(v,k){same=(v===k)?1:0;}); same
var s=new Set(); s.add(2); var self=0; s.forEach(function(v,k,ss){self=(ss===s)?1:0;}); self

// --- Map keys / values / entries iterators (fxNewMapIteratorInstance) ---
var m=new Map(); m.set(1,2); var it=m.keys(); it.next().value
var m=new Map(); m.set(1,2); var it=m.values(); it.next().value
var m=new Map(); m.set(1,2); var it=m.entries(); it.next().value[0]
var m=new Map(); m.set(1,2); var it=m.entries(); it.next().value[1]
var m=new Map(); m.set(7,8); var it=m.keys(); it.next(); it.next().done
var m=new Map(); m.set(1,2); m.set(3,4); var it=m.values(); it.next(); it.next().value
var m=new Map(); m.set(1,2); m.set(3,4); m.set(5,6); var it=m.keys(); var t=0; var r=it.next(); while(!r.done){t+=r.value; r=it.next();} t

// --- Set values / keys (Set.keys === Set.values) / entries iterators ---
var s=new Set(); s.add(9); var it=s.values(); it.next().value
var s=new Set(); s.add(9); var it=s.keys(); it.next().value
var s=new Set(); s.add(5); var it=s.entries(); it.next().value[0]
var s=new Set(); s.add(5); var it=s.entries(); it.next().value[1]
var s=new Set(); s.add(1); s.add(2); var it=s.values(); it.next(); it.next().done

// --- for-of over Map (Symbol.iterator = entries) and Set (= values) ---
var s=new Set(); s.add(1); s.add(2); s.add(3); var t=0; for(var x of s){t+=x;} t
var m=new Map(); m.set(1,10); m.set(2,20); var t=0; for(var e of m){t+=e[1];} t
var m=new Map(); m.set(1,10); m.set(2,20); var t=0; for(var e of m){t+=e[0];} t
var s=new Set(); for(var i=0;i<10;i++){s.add(i);} var t=0; for(var x of s){t+=x;} t

// --- spread over Map and Set ([...coll]) ---
var s=new Set(); s.add(5); s.add(6); var a=[...s]; a[1]
var s=new Set(); s.add(5); s.add(6); var a=[...s]; a.length
var m=new Map(); m.set(1,2); var a=[...m]; a[0][0]
var m=new Map(); m.set(1,2); var a=[...m]; a[0][1]

// --- Map.prototype.clear / Set.prototype.clear (fxClearEntries) ---
var m=new Map(); m.set(1,2); m.clear(); m.size
var m=new Map(); m.set(1,2); m.set(3,4); m.clear(); m.has(1)
var m=new Map(); for(var i=0;i<8;i++){m.set(i,i);} m.clear(); m.size
var m=new Map(); m.clear(); m.size
var m=new Map(); m.set(1,2); m.set(3,4); m.clear(); m.set(5,6); m.get(5)
var s=new Set(); s.add(1); s.add(2); s.clear(); s.size
var s=new Set(); s.add(1); s.clear(); s.has(1)
var s=new Set(); for(var i=0;i<16;i++){s.add(i);} s.clear(); s.size
var s=new Set(); for(var i=0;i<20;i++){s.add(i);} s.clear(); var t=0; for(var x of s){t=t+1;} t
