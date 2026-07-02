// Stage-1 corpus: arithmetic. One program per line; the completion
// value is the (single) expression. Kept inside the arithmetic /
// logic / branch / stack opcode subset so the bar is bit-exact
// (result, computron) agreement with the C-XS oracle.
1
0
-5
1 + 2
1 + 2 * 3
(1 + 2) * 3
10 - 4
2 * 3 * 4
100 / 4
7 % 3
7 % -3
-7 % 3
1 - 2 - 3
2 * -3
1000000 * 1000000
2147483647 + 1
-2147483648 - 1
1.5 + 2.5
0.1 + 0.2
10 / 3
3 / 0
-3 / 0
0 / 0
1.5 * 2
5 % 0
- (3 + 4)
+ 7
1 + 2 + 3 + 4 + 5
((2 + 3) * (4 - 1)) % 7
2 * 2 * 2 * 2 * 2
