# Decision 0006: Exact CUDA algorithms and eligibility

Status: accepted.

The CUDA reference is an exact frontier label-correcting kernel. The optimized
candidate is exact delta-stepping with a positive finite delta, light-edge
closure, retained current-bucket nodes, and heavy-edge relaxation. Both use
binary32 operands converted to binary64, a separate binary64 multiplication
and addition, strict improvements, and exact bit comparisons. Internal
positive infinity represents unvisited state and never becomes a public
logical distance.

CUDA eligibility is one origin, any destination multiset, a complete explicit
profile, distance-only output, unlimited examined-edge budget, current numeric
and tie policies, fixed-width representable counts, a matching fully resident
image, and a healthy scheduler/device. Paths and finite deterministic
examined-edge budgets use CPU under `CpuOnly`, `PreferCuda`, and `Auto`, and are
typed refusals under `RequireCuda`.

`Auto` is conservative: the RTX 3080 baseline shows the initial kernels do not
provide a stable universal crossover, so automatic routing remains on CPU.
`PreferCuda` is the explicit opt-in used for accelerator measurement and
operation. The frontier kernel remains available as the CUDA comparator when
delta-stepping is selected.
