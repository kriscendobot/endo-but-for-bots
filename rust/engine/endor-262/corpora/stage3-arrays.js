// Stage-3 child-3 (arrays) curated corpus: the Array exotic object and its
// index/length semantics, array literals with holes, computed element get/set,
// and the array-item chunk growth — all bit-exact (completion value AND
// computron count) against the C-XS oracle at the pin 48ee02d8cfe0. One JS
// program per line; the last expression is the completion value.

// --- array literals (fxNewArray + per-element NEW_PROPERTY_AT) ---
[]
[1]
[1,2,3]
[1,2,3,4,5]
[7,8,9,10,11,12]
["a","b","c"]
[true,false,null]
[1,"two",true,null]

// --- holes (a literal elision leaves a hole; length still spans them) ---
[1,,3]
[,1]
[1,,]
[,,3]

// --- nested arrays (toString joins recursively) ---
[[1],[2,3]]
[[1,2],[3,4]]
[[]]
[[1,[2,[3]]]]

// --- indexed reads (AT + GET_PROPERTY_AT over the item chunk) ---
[10,20,30][0]
[10,20,30][1]
[10,20,30][2]
[10,20,30][3]
var a=[10,20,30]; a[0]
var a=[10,20,30]; a[2]
var a=[10,20,30]; a[5]
[10,20][0]+[10,20][1]
[1,2,3][2]*10

// --- length reads (the array length accessor getter) ---
[].length
[1,2,3].length
var a=[1,2,3,4]; a.length
var a=[]; a.length

// --- indexed writes (SET_PROPERTY_AT: overwrite and grow) ---
var a=[5]; a[0]=9; a[0]
var a=[1,2,3]; a[1]=99; a[1]
var a=[1,2,3]; a[1]=99; a
var a=[]; a[0]=7; a.length
var a=[]; a[0]=7; a[0]
var a=[1,2,3]; a[5]=9; a.length
var a=[1,2,3]; a[5]=9; a[5]
var a=[1]; a[0]=a[0]+1; a[0]

// --- length writes (fxArraySetLength: grow with holes, shrink drops items) ---
var a=[1,2,3]; a.length=2; a
var a=[1,2,3]; a.length=1; a
var a=[1,2,3]; a.length=0; a.length
var a=[1,2,3]; a.length=5; a.length
var a=[1,2,3]; a.length=5; a

// --- length set that does not resize (isolates the create-vs-set cost the
//     fuzz arm exposed: a second length store meters nothing beyond dispatch) ---
var a=[]; a.length=0; a
var a=[]; a.length=0; a.length
var a=[1,2]; a.length=2; a
var a=[1,2,3]; a.length=3; a
var a=[4]; a[3]=5; a.length
var a=[4]; a[3]=5; a

// --- Array.prototype mutation methods (dense fast path, mxMeterSome-exact) ---
var a=[1,2]; a.push(3)
var a=[1,2]; a.push(3); a
var a=[1,2,3]; a.push(9)
var a=[1]; a.push(2,3)
var a=[1]; a.push(2,3); a.length
var a=[]; a.push(1)
var a=[]; a.push(1); a
var a=[1,2,3]; a.pop()
var a=[5]; a.pop()
var a=[]; a.pop()
var a=[1,2,3]; a.pop(); a
var a=[1,2,3]; a.pop(); a.length
var a=[1,2,3]; a.push(4); a.pop()
var a=[1,2,3]; a.indexOf(2)
var a=[1,2,3]; a.indexOf(9)
var a=[1,2,3]; a.indexOf(1)
var a=[1,2,3]; a.indexOf(3)
var a=[]; a.indexOf(1)
var a=[10,20,30]; a.indexOf(20)
var a=["x","y","z"]; a.indexOf("z")
var a=[1,2,3]; a.includes(2)
var a=[1,2,3]; a.includes(9)
var a=[1,2,3]; a.includes(1)
var a=[5,6,7]; a.includes(7)
var a=[]; a.includes(1)
var a=[1,2,3]; a.lastIndexOf(2)
var a=[1,2,3]; a.lastIndexOf(9)
var a=[1,2,3,2]; a.lastIndexOf(2)
var a=[1,2,3]; a.lastIndexOf(3)
var a=[1,2,3]; a.fill(0); a
var a=[1,2,3,4]; a.fill(0); a
var a=[1,2,3]; a.fill(9,1); a
var a=[1,2,3,4]; a.fill(7,1,3); a
var a=[1,2,3]; a.fill(5); a.length
var a=[1,2,3]; a.slice(1)
var a=[1,2,3,4]; a.slice(1,3)
var a=[1,2,3]; a.slice()
var a=[1,2,3]; a.slice(0,0)
var a=[5,6,7,8,9]; a.slice(2)
var a=[1,2,3]; a.slice(-1)
var a=[1,2,3]; a.slice(1).length
var a=[1,2,3]; a.slice(1)[0]
var a=[1,2,3]; a.join()
var a=[1,2,3,4]; a.join()
var a=[7]; a.join()
var a=[]; a.join()
var a=["x","y"]; a.join()
var a=[10,20,30]; a.join()
var a=[1,2,3]; a.join("-")
var a=[1,2,3]; a.join("--")
var a=[1,2,3]; a.at(0)
var a=[1,2,3]; a.at(1)
var a=[1,2,3]; a.at(-1)
var a=[1,2,3]; a.at(5)
var a=[1,2,3]; a.at(-5)
var a=[5,6,7,8]; a.at(2)
var a=[5,6,7,8]; a.at(-2)
var a=[1,2,3]; a.reverse(); a
var a=[1,2,3,4]; a.reverse(); a
var a=[1,2]; a.reverse(); a
var a=[1]; a.reverse(); a
var a=[]; a.reverse(); a
var a=[1,2,3,4,5]; a.reverse(); a
var a=[9,8,7]; a.reverse(); a[0]
var a=[1,2,3]; a.shift()
var a=[1,2,3]; a.shift(); a
var a=[1,2,3,4]; a.shift(); a
var a=[5]; a.shift(); a.length
var a=[]; a.shift()
var a=[2,3]; a.unshift(1)
var a=[2,3]; a.unshift(1); a
var a=[3]; a.unshift(1,2); a
var a=[]; a.unshift(9); a
var a=[1,2,3]; a.unshift(0); a
var a=[5]; a.unshift(1,2,3); a.length
var a=[1,2]; a.concat([3,4])
var a=[1,2]; a.concat(3)
var a=[1]; a.concat([2],[3])
var a=[]; a.concat([1,2])
var a=[1,2]; a.concat()
var a=[1]; a.concat(2,3)
var a=[1,2,3]; a.concat([4,5],6)
var a=[1]; a.concat([2,3],[4],5,6)
var a=[1,2]; a.concat([3,4]).length

// --- Array constructor + statics (fx_Array; call and construct forms) ---
Array()
Array(0)
Array(3)
Array(5)
Array(1,2)
Array(1,2,3)
new Array()
new Array(3)
new Array(1,2,3)
Array(1,2,3)[1]
var a=Array(1,2,3); a.push(4); a
var a=Array(2,4,6); a.length
Array.isArray([1,2])
Array.isArray([])
Array.isArray(5)
Array.isArray("x")
Array.isArray(Array(3))

// --- array iterators (values/keys/entries + next, reused result object) ---
var it=[1,2,3].values(); it.next().value
var it=[1,2,3].values(); it.next(); it.next().value
var it=[7,8].values(); it.next(); it.next().value
[1,2,3].values().next().done
[1,2,3].values().next().value
var it=[5].keys(); it.next().value
var it=[1,2].keys(); it.next(); it.next().value
[1,2,3].keys().next().value
var it=[1,2,3].entries(); it.next().value
var it=[5,6].entries(); it.next(); it.next().value
var it=[9].values(); it.next(); it.next().done
var it=[9].values(); it.next(); it.next().value
var it=[].values(); it.next().done
var it=[].keys(); it.next().done
var it=[1,2,3].keys(); it.next(); it.next(); it.next(); it.next().done
var it=[7,8,9].values(); it.next(); it.next(); it.next().value

// --- for-of over arrays (fxGetIterator + the values iterator protocol) ---
var s=0; for (var x of [1,2,3]) s=s+x; s
var s=0; for (var x of [10,20]) s=s+x; s
var s=0; for (var x of []) s=s+x; s
var s=0; for (var x of [5]) s=s+x; s
var p=1; for (var x of [1,2,3,4]) p=p*x; p
var s=""; for (var x of [1,2,3]) s=s+x; s
var n=0; for (var x of [7,7,7]) n=n+1; n
var s=0; for (var x of [1,2,3,4,5]) s=s+x; s

// --- spread (desugars to the for-of iterator loop appending each element) ---
[...[1,2,3]]
[...[]]
[...[9]]
[0,...[1,2],3]
var a=[5,6,7]; [...a]
[...[1,2,3]].length
[...[1,2,3]][2]
var a=[1,2]; var b=[...a]; b[0]
var a=[1,2,3]; [...a][2]

// --- for-in over objects and arrays (fx_Enumerator; XS enumeration order) ---
var s=""; for (var k in {a:1,b:2}) s=s+k; s
var s=""; for (var k in {x:1,y:2,z:3}) s=s+k; s
var s=""; for (var k in {}) s=s+k; s
var n=0; for (var k in {a:1,b:2,c:3}) n=n+1; n
var s=""; for (var k in [10,20,30]) s=s+k; s
var s=""; for (var k in [5]) s=s+k; s
var n=0; for (var k in []) n=n+1; n
var n=0; for (var k in [7,7,7,7,7]) n=n+1; n
var s=""; for (var k in {p:1,q:2,r:3,s:4,t:5}) s=s+k; s

// --- mixed literal + mutation ---
var a=[1,2,3]; a[0]=a[2]; a
var a=[0,0,0]; a[0]=1; a[2]=3; a
var a=[1,2,3,4,5]; a.copyWithin(0,3); a
var a=[1,2,3,4,5]; a.copyWithin(1,3); a
var a=[1,2,3]; a.copyWithin(0,1); a
var a=[1,2,3,4,5]; a.copyWithin(0,3,4); a
var a=[1,2,3]; a.copyWithin(2,0); a
var a=[1,2,3]; a.copyWithin(0,0); a
var a=[1,2,3,4]; a.copyWithin(1,2); a
var a=[1,2,3]; a.with(1,9); a
var a=[1,2,3]; a.with(1,9)[1]
var a=[1,2,3,4]; a.with(0,9)[0]
var a=[5]; a.with(0,9)[0]
var a=[1,2,3]; a.with(-1,9)[2]
var a=[1,2,3,4,5]; a.with(2,0); a
var s=0; [1,2,3].forEach(function(x){s=s+x}); s
var s=0; [10,20].forEach(function(x){s=s+x}); s
var s=0; [].forEach(function(x){s=s+x}); s
var s=0; [5].forEach(function(x){s=s+x}); s
var s=0; [1,2,3].forEach(function(x,i){s=s+i}); s
var p=1; [2,3,4].forEach(function(x){p=p*x}); p
var s=0; [1,2,3,4,5].forEach(function(x){s=s+x}); s
[1,2,3].map(function(x){return x*2}).join()
[1,2].map(function(x){return x+1}).join()
[].map(function(x){return x}).join()
[1,2,3].map(function(x){return x*2})[1]
[1,2,3].some(function(x){return x>2})
[1,2,3].some(function(x){return x>5})
[1,2,3].every(function(x){return x>0})
[1,2,3].every(function(x){return x>1})
[1,2,3].find(function(x){return x>1})
[1,2,3].find(function(x){return x>5})
[1,2,3].findIndex(function(x){return x>1})
[1,2,3].findIndex(function(x){return x>5})
[1,2,3,4].filter(function(x){return x>2}).join()
[1,2,3,4].filter(function(x){return x>0}).join()
[1,2,3,4].filter(function(x){return x>5}).length
var s=0; [1,2,3].map(function(x){return x*2}).forEach(function(y){s=s+y}); s
[1,2,3].reduce(function(a,x){return a+x})
[1,2,3].reduce(function(a,x){return a+x},10)
[1,2,3,4].reduce(function(a,x){return a+x})
[5].reduce(function(a,x){return a+x})
[1,2,3,4].reduce(function(a,x){return a+x},0)
[1,2,3].reduceRight(function(a,x){return a-x})
[1,2,3,4].reduceRight(function(a,x){return a+x})
[1,2,3].findLast(function(x){return x<3})
[1,2,3].findLast(function(x){return x<0})
[1,2,3].findLastIndex(function(x){return x<3})
[1,2,3].findLastIndex(function(x){return x<0})
[1,2,3,4,5].findLast(function(x){return x<4})
[1,2,3].toReversed().join()
[1,2,3,4].toReversed().join()
[5].toReversed()[0]
[].toReversed().length
var a=[1,2,3]; a.toReversed(); a
