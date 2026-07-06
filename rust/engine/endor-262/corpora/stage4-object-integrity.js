// Stage-4 child 1/8: the property-attribute integrity model + descriptor
// reflection (harden's prerequisite). Each line is one program, bit-exact
// (result AND computron) against the C-XS pin.

// --- Object.preventExtensions / isExtensible ---
// A fresh object is extensible.
Object.isExtensible({});
Object.isExtensible({a:1});
// preventExtensions returns the object and clears extensibility.
var o={}; Object.preventExtensions(o)===o;
var o={a:1}; Object.preventExtensions(o); Object.isExtensible(o);
// A non-extensible object rejects a NEW key in sloppy mode (silent no-op).
var o={a:1}; Object.preventExtensions(o); o.b=5; typeof o.b;
var o={a:1}; Object.preventExtensions(o); o.b=5; o.b===undefined;
// An existing writable property is still writable after preventExtensions.
var o={a:1}; Object.preventExtensions(o); o.a=9; o.a;
// A non-object argument passes through preventExtensions unchanged.
Object.isExtensible(5);
Object.isExtensible("str");

// --- Object.seal / isSealed ---
// An extensible object is not sealed.
Object.isSealed({});
Object.isSealed({a:1});
// seal returns the object; the sealed object reports isSealed true.
var o={a:1}; Object.seal(o)===o;
var o={a:1}; Object.seal(o); Object.isSealed(o);
var o={a:1,b:2}; Object.seal(o); Object.isSealed(o);
var o={a:1,b:2,c:3}; Object.seal(o); Object.isSealed(o);
// A sealed object is not extensible and its keys are non-configurable, but
// its data values remain writable (so not frozen).
var o={a:1}; Object.seal(o); Object.isExtensible(o);
var o={a:1}; Object.seal(o); Object.isFrozen(o);
var o={a:1}; Object.seal(o); o.a=7; o.a;
// A sealed key cannot be deleted (sloppy: delete yields false, key stays).
var o={a:1}; Object.seal(o); delete o.a;
var o={a:1}; Object.seal(o); delete o.a; o.a;
// A non-object argument is vacuously sealed.
Object.isSealed(1);

// --- Object.freeze / isFrozen ---
// An extensible object is not frozen.
Object.isFrozen({});
Object.isFrozen({a:1});
// freeze returns the object; the frozen object reports isFrozen true.
var o={a:1}; Object.freeze(o)===o;
var o={a:1}; Object.freeze(o); Object.isFrozen(o);
var o={a:1,b:2}; Object.freeze(o); Object.isFrozen(o);
var o={a:1,b:2,c:3}; Object.freeze(o); Object.isFrozen(o);
// A frozen object is also sealed and not extensible.
var o={a:1}; Object.freeze(o); Object.isSealed(o);
var o={a:1}; Object.freeze(o); Object.isExtensible(o);
// A frozen data property rejects a write in sloppy mode (silent no-op).
var o={a:1}; Object.freeze(o); o.a=2; o.a;
var o={a:1,b:2}; Object.freeze(o); o.a=9; o.b=9; o.a+o.b;
// A frozen object rejects a NEW key too (non-extensible).
var o={a:1}; Object.freeze(o); o.b=5; typeof o.b;
// A frozen key cannot be deleted.
var o={a:1}; Object.freeze(o); delete o.a; o.a;
// The descriptor of a frozen property reads back non-writable/non-configurable.
var o={a:1}; Object.freeze(o); var d=Object.getOwnPropertyDescriptor(o,"a"); d.writable===false && d.configurable===false && d.enumerable===true;
// A non-object argument is vacuously frozen.
Object.isFrozen(undefined);
Object.isFrozen(true);

// --- integrity composes with defineProperty attributes ---
// A defineProperty'd non-configurable property makes the object sealed once
// non-extensible.
var o={}; Object.defineProperty(o,"x",{value:1,writable:false,enumerable:true,configurable:false}); Object.preventExtensions(o); Object.isFrozen(o);
// A writable-but-non-configurable property is sealed but not frozen.
var o={}; Object.defineProperty(o,"x",{value:1,writable:true,enumerable:true,configurable:false}); Object.preventExtensions(o); Object.isSealed(o) && !Object.isFrozen(o);

// --- Object.values ---
Object.values({}).length;
Object.values({a:1})[0];
Object.values({a:1,b:2})[0] + Object.values({a:1,b:2})[1];
Object.values({a:1,b:2,c:3}).length;
var o={x:10,y:20}; var v=Object.values(o); v[0]+","+v[1];
// A non-enumerable property is excluded from values.
var o={a:1}; Object.defineProperty(o,"h",{value:9,writable:true,enumerable:false,configurable:true}); Object.values(o).length;

// --- Object.entries ---
Object.entries({}).length;
Object.entries({a:1})[0][0];
Object.entries({a:1})[0][1];
Object.entries({a:1,b:2}).length;
var e=Object.entries({first:1,second:2}); e[0][0]+"="+e[0][1];
// entries excludes a non-enumerable property.
var o={a:1}; Object.defineProperty(o,"h",{value:9,writable:true,enumerable:false,configurable:true}); Object.entries(o).length;

// --- Object.getOwnPropertyDescriptors ---
typeof Object.getOwnPropertyDescriptors({});
var d=Object.getOwnPropertyDescriptors({a:1}); d.a.value;
var d=Object.getOwnPropertyDescriptors({a:1}); d.a.writable && d.a.enumerable && d.a.configurable;
var d=Object.getOwnPropertyDescriptors({a:1,b:2}); d.b.value;
var o={}; Object.defineProperty(o,"x",{value:7,writable:false,enumerable:true,configurable:false}); var d=Object.getOwnPropertyDescriptors(o); d.x.writable===false && d.x.configurable===false;

// --- Object.prototype.propertyIsEnumerable ---
var o={a:1}; o.propertyIsEnumerable("a");
var o={a:1}; o.propertyIsEnumerable("b");
var o={a:1,b:2}; o.propertyIsEnumerable("a") && o.propertyIsEnumerable("b");
// A non-enumerable own property is not enumerable.
var o={a:1}; Object.defineProperty(o,"h",{value:9,writable:true,enumerable:false,configurable:true}); o.propertyIsEnumerable("h");
var o={a:1}; Object.defineProperty(o,"e",{value:9,writable:true,enumerable:true,configurable:true}); o.propertyIsEnumerable("e");
// An absent key is not enumerable.
var o={a:1}; o.propertyIsEnumerable("zzz");
