// Stage-3b (bigint) curated corpus: the BigInt primitive per the pin — the
// value Kind + [sign][little-endian u32 limbs] digit-chunk representation,
// XS_CODE_BIGINT_1/2 literals (fxNewBigInt + fxNewChunk(size*4)), the metered
// arithmetic + - * (the mxBigInt_meter digit step over the trimmed result
// size and the allocation-faithful result chunk at XS's pre-trim size), unary
// minus (fxBigInt_neg), strict/loose equality including BigInt-vs-Number
// (fxNumberToBigInt), relational order, `typeof "bigint"`, and decimal
// completion rendering (no `n` suffix). Bit-exact (completion value AND
// computron count) against the C-XS oracle at the pin 48ee02d8cfe0. One JS
// program per line; the last expression is the completion value.

// --- literals + decimal completion rendering (no `n` suffix) ---
0n
1n
42n
255n
65535n
4294967295n
4294967296n
9007199254740991n
9007199254740993n
123456789012345678901234567890n
1000000000n
999999999999n

// --- typeof "bigint" ---
typeof 0n
typeof 42n
typeof 123456789012345678901234567890n

// --- unary minus (fxBigInt_neg); -0n normalizes to +0n ---
-1n
-42n
-9007199254740993n
-0n
- -5n

// --- addition (uadd: max+1 limbs) / carry across the limb boundary ---
2n + 3n
0n + 0n
100n + 250n
4294967295n + 1n
4294967295n + 4294967295n
9007199254740993n + 1n
18446744073709551615n + 1n
123456789012345678901234567890n + 987654321098765432109876543210n

// --- addition with mixed signs (usub path, max limbs) ---
100n + -30n
-100n + 30n
-100n + -30n
5n + -5n

// --- subtraction ---
100n - 7n
7n - 100n
0n - 0n
5n - 5n
4294967296n - 1n
18446744073709551616n - 1n
-100n - -30n
100n - -30n
1000000000000000000000n - 1n

// --- multiplication (umul: a.size + b.size limbs) ---
2n * 3n
10n * 20n
0n * 12345n
-3n * -4n
-3n * 4n
4294967296n * 4294967296n
999999999999999999999n * 999999999999999999999n
123456789n * 987654321n

// --- strict equality / inequality (same-type sign+magnitude) ---
5n === 5n
5n === 6n
5n !== 6n
-5n === -5n
-5n === 5n
0n === 0n
-0n === 0n
9007199254740993n === 9007199254740993n
9007199254740993n === 9007199254740994n

// --- strict equality across types is always false ---
5n === 5
5n === "5"
5n !== 5

// --- loose equality with a Number (fxNumberToBigInt) ---
5n == 5
5 == 5n
5n == 6
5n != 5
0n == 0
-5n == -5
4294967296n == 4294967296
9007199254740992n == 9007199254740992
5n == 5.5
5n != 5.5
5n == 4.999

// --- relational order (both BigInt) ---
5n < 6n
6n < 5n
5n <= 5n
6n > 5n
5n > 6n
6n >= 6n
6n >= 7n
-5n < 3n
-5n < -3n
-3n < -5n
0n <= 0n
4294967296n > 4294967295n
123456789012345678901234567890n > 999999999n

// --- variables + accumulation (the metering hot path) ---
var x=5n; x
var x=5n; var y=7n; x + y
var x=10n; var y=3n; x * y - y
var a=2n; a + a + a + a
var n=1n; n = n + n; n = n + n; n = n + n; n
var big=9007199254740991n; big + big
var s=0n; s = s + 100n; s = s + 200n; s
var p=1n; p = p * 10n; p = p * 10n; p = p * 10n; p
