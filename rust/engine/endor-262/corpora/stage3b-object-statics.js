// Stage-3b child 5/9: the global runtime string->id intern table +
// Object statics / verifyProperty machinery. Each line is one program,
// bit-exact (result AND computron) against the C-XS pin.

// --- hasOwnProperty over the global intern table ---
// An own key (always a program symbol) is found: true.
var o = {a:1}; o.hasOwnProperty("a");
var o = {a:1,b:2}; o.hasOwnProperty("a") && o.hasOwnProperty("b");
// A genuinely-novel key misses the table, interning one metered key slot,
// and is not an own property: false.
var o = {a:1}; o.hasOwnProperty("b");
var o = {a:1}; o.hasOwnProperty("zzz");
// A well-known inherited name (a boot default key) interns without
// allocating and is correctly not an OWN property: false.
var o = {a:1}; o.hasOwnProperty("toString");
var o = {a:1}; o.hasOwnProperty("valueOf");
var o = {a:1}; o.hasOwnProperty("hasOwnProperty");
// The intern table is global and persistent: a repeated novel key does not
// re-allocate a second key slot.
var o = {a:1}; o.hasOwnProperty("zzz"); o.hasOwnProperty("zzz");
// Two distinct novel keys each intern their own slot.
var o = {a:1}; o.hasOwnProperty("zzz"); o.hasOwnProperty("yyy");
// A present own key that is only reachable by a computed string.
var o = {foo:10, bar:20}; o.hasOwnProperty("foo");
var o = {foo:10, bar:20}; o.hasOwnProperty("baz");

// --- Object.keys: own enumerable string keys as a fresh Array ---
// The empty object yields an empty array.
Object.keys({}).length;
// Keys are returned in property-creation order.
var k = Object.keys({a:1}); k[0];
var k = Object.keys({a:1,b:2}); k[0] + k[1];
var k = Object.keys({a:1,b:2,c:3}); k.length;
var k = Object.keys({first:1, second:2, third:3}); k[0] + "," + k[1] + "," + k[2];
// The result is a real Array (has array methods / length semantics).
var k = Object.keys({x:1,y:2,z:3}); k.length;
// The intern table is unaffected by key-name length (the interned key
// string is referenced, not re-allocated) — metering is name-length neutral.
var k = Object.keys({longkeyname:1, s:2}); k[0];
// Object.keys and hasOwnProperty compose over the same object.
var o = {p:1, q:2}; Object.keys(o).length === 2 && o.hasOwnProperty("p");

// --- Object.getOwnPropertyDescriptor: the verifyProperty machinery ---
// A present ordinary data property yields a full data descriptor.
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.value;
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.writable;
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.enumerable;
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.configurable;
// Fields render in XS's value/writable/enumerable/configurable order.
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"a"); var s=""; s+=d.value; s+=d.writable; s+=d.enumerable; s+=d.configurable; s;
// A literal's own data property is writable, enumerable, and configurable.
var o={a:7}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.value===7 && d.writable===true && d.enumerable===true && d.configurable===true;
// An absent key yields undefined (the interned-but-not-own case).
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"b"); typeof d;
var o={a:1}; var d=Object.getOwnPropertyDescriptor(o,"zzz"); d===undefined;
// A string-valued property's descriptor references the value (no re-copy).
var o={a:"hello"}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.value;
// The verifyProperty shape: hasOwnProperty + descriptor attributes agree.
var o={k:42}; var d=Object.getOwnPropertyDescriptor(o,"k"); o.hasOwnProperty("k") && d.enumerable && d.configurable && d.writable;
// --- computed string member access o[k] via the interning AT opcode ---
// A computed key that names a present program symbol reads the own value; the
// key resolves through the intern table without allocating (a program symbol).
var o={a:1}; var k="a"; o[k];
var o={foo:10,bar:20}; var k="bar"; o[k];
var o={a:1,b:2,c:3}; var k="c"; o[k];
// A genuinely-novel computed key misses the table, interns exactly one key
// slot, and reads undefined (the absent-own, no-inherited case).
var o={a:1}; var k="zzz"; o[k];
var o={a:1}; var k="zzz"; typeof o[k];
var o={a:1}; var k="missing"; o[k]===undefined;
// The intern table is persistent: a repeated novel computed key allocates no
// second slot, and two distinct novel keys each intern their own.
var o={a:1}; var k="zzz"; o[k]; o[k];
var o={a:1}; var j="zzz"; var m="yyy"; typeof o[j]; typeof o[m];
// The computed key and a static access agree on a present property.
var o={p:7}; var k="p"; o[k]===o.p;
// A computed novel key composes with hasOwnProperty over the same name.
var o={a:1}; var k="zzz"; o[k]===undefined && o.hasOwnProperty(k)===false;
// --- the `in` operator over the intern table (sound false-answers) ---
// A present own key is `true` (the existing own-hit case).
var o={a:1}; "a" in o;
var o={foo:10,bar:20}; "bar" in o;
// A genuinely-novel key is a sound `false` — it can be no inherited built-in
// (absent from XS's boot key table), so `in` walks to null and interns one
// key slot.
var o={a:1}; "zzz" in o;
var o={a:1}; "missing" in o;
var o={a:1,b:2}; ("a" in o) && !("c" in o);
// A novel key `in` composes with hasOwnProperty over the same absent name.
var o={a:1}; ("zzz" in o)===false && o.hasOwnProperty("zzz")===false;
