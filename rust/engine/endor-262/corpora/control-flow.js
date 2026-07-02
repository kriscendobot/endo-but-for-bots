// Stage-1 corpus: branch-driven control flow via the conditional
// operator and short-circuit chains (the branch opcodes: BRANCH,
// BRANCH_ELSE, BRANCH_IF).
1 < 2 ? 10 : 20
2 < 1 ? 10 : 20
0 ? 1 : 2
1 ? 1 : 2
1 < 2 ? 3 < 4 ? 5 : 6 : 7
5 > 3 ? 5 - 3 : 3 - 5
(1 && 2) ? 100 : 200
(0 || 5) ? 100 : 200
true ? false ? 1 : 2 : 3
1 + (2 < 3 ? 10 : 20)
void 0
void 0 ? 1 : 2
(1 < 2 && 2 < 3) ? 42 : 0
(1 > 2 || 2 > 1) ? 7 : 8
2 * (3 > 2 ? 4 : 5)
