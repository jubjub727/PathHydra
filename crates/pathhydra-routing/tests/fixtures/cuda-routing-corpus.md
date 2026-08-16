# Deterministic accelerator agreement corpus

This pre-release corpus is deliberately in the current public API shape and is
updated in place. The executable form is `pathhydra-cuda/tests/agreement.rs`.

## Directed zero-cycle, parallel relation, missing and duplicate destinations

- Nodes in stable order: `node-0` through `node-5`.
- Relations: `road` multiplier `1.0`; `rail` multiplier `0.25`.
- Directed base weights: `0->1 road 1.0`, `1->2 road 0.0`, `2->1 road 0.0`,
  `2->3 road 0.2`, `0->3 rail 0.8`, `3->4 rail 0.05`, `0->4 road 1.0`,
  and parallel `0->4 rail 0.6`.
- Origin: `node-0`.
- Destination order: `node-4`, `node-3`, missing ID, duplicate `node-4`,
  isolated `node-5`, origin `node-0`.
- CPU states: exact, exact, missing, exact, unreachable, exact zero.
- Interesting because it combines direction, zero-cycle termination, context
  winner changes, parallel edges, duplicate expansion, isolation, and origin
  handling in one inspectable fixture.

Both CUDA frontier and delta-stepping compare every exact distance bit with the
CPU response and require the same canonical profile and destination states.
