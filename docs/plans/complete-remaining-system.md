# Master Implementation Plan: Complete the Remaining PathHydra System

## Outcome

Implement the complete remaining PathHydra system by executing, in dependency
order, all work in:

1. [Plan 07: Parallel CUDA Execution and Request-Shape Closure](07-scalable-cuda-execution.md)
2. [Plan 08: Durable Operations and Store Observability](08-durable-operations-and-store-observability.md)
3. [Plan 09: Consumer-Ready Rust API and Canonical Encoding](09-consumer-ready-rust-api-and-canonical-encoding.md)
4. [Plan 10: System Conformance and Performance Closure](10-system-conformance-and-performance-closure.md)

This is the single orchestration plan for implementing all four individual
plans. A request to implement this file and commit the changes authorizes the
complete dependency graph above. Do not stop after implementing only one plan, one
subsystem, a representative subset, scaffolding, or the portions covered by
ordinary tests.

The terminal condition is the definition of done in this master plan, not the
creation of an intermediate commit or a report that substantial progress was
made.

## Authority

Read these sources before implementation:

1. repository `AGENTS.md`;
2. `PATHHYDRA_SYSTEM_SHAPE.md` in full;
3. [the remaining-system roadmap](remaining-system-roadmap.md);
4. Plans 07, 08, 09, and 10 in full;
5. the decisions and current reference documents required by each plan.

`PATHHYDRA_SYSTEM_SHAPE.md` is authoritative. The individual plans divide its
remaining work into implementable boundaries; they do not weaken or replace
it. If an individual plan conflicts with the original document, correct the
plan to preserve the original contract rather than shrinking implementation or
acceptance criteria. Material expansion beyond both the original document and
the selected plans requires user approval.

Before editing, inspect the current commit, complete worktree status, existing
warnings or failures, available CUDA hardware/toolchains, disk capacity, and
the completed Plan-06 baseline. Preserve unrelated user changes and never use a
destructive Git operation to obtain a clean tree.

## Non-negotiable continuation rule

Create one persistent goal for completing this master plan when the environment
provides a goal mechanism. Keep it active across long commands, subagent turns,
context compaction, and individual-plan commits. Do not mark it complete or
send the user a final response while safe in-scope work remains.

The following are progress points, not stopping conditions:

- completing or committing Plan 07, 08, or 09;
- passing ordinary tests while CUDA, sanitizer, benchmark, or scale gates have
  not run;
- implementing production code while required decisions, documentation,
  metrics, recovery rehearsals, or malformed-input tests remain;
- receiving a subagent report that does not cover every assigned acceptance
  criterion;
- reaching a large diff, a long runtime, a context boundary, or a convenient
  handoff point;
- discovering that existing code already satisfies part of a plan.

Continue automatically through every ready node in the dependency graph. Ask the user only
when completion requires new authority, an original-design choice lacks the
required current evidence and would materially change product scope, or an
external blocker remains after all safe in-scope alternatives are exhausted.
Never call a skipped or unavailable required gate a pass.

## Mandatory subagent organization

Use subagents throughout this master plan when the environment provides them.
Use all useful concurrency slots. During the first implementation phase, assign
separate leads to Plans 07 and 08 so their independent CUDA and store/operations
work advances concurrently. Keep another agent, or later reuse an agent from
the other track, independent of the implementation area it audits.

The coordinating agent owns the complete design, integration, verification,
and Git history. It must read every plan itself and must not delegate overall
understanding or the final completion decision.

For each individual plan, run these waves. Waves for independent plans may
overlap when ownership is explicit:

1. **Mapping wave**
   - Build a ledger containing every numbered section, required test,
     benchmark, decision, documentation item, and definition-of-done bullet.
   - Spawn subagents to inspect independent prerequisite/code areas and return
     exact file, API, test, and evidence gaps.
   - Establish a dependency graph and explicit file ownership before edits.
2. **Implementation wave**
   - Assign disjoint, bounded workstreams to subagents. Each assignment includes
     production behavior, tests, documentation, and concrete acceptance
     criteria rather than a vague request to "help with the plan."
   - The coordinator owns shared types, central integration, and any area where
     parallel edits would overlap.
   - Use follow-up assignments when a subagent returns partial work. Do not
     silently absorb missing requirements into a later plan.
3. **Verification wave**
   - Assign adversarial/fault tests, benchmarks and sanitizer runs,
     documentation/decision reconciliation, and public API/resource/safety
     review to independent agents as appropriate.
   - Wait for all running agents and commands. A commentary update does not end
     the stage.
4. **Completion-audit wave**
   - An agent that did not implement the audited area compares the final tree
     with the complete individual plan and the relevant original-design
     sections.
   - Repair every finding, rerun affected gates, and repeat the audit after any
     material repair.

All agents normally share the workspace. Do not assign overlapping files
without a communicated handoff. Subagents must not make repository commits
unless the coordinator explicitly assigns an isolated commit boundary. The
coordinator reviews every subagent diff and remains responsible for correctness.

Plans 07 and 08 both may touch engine configuration, health, diagnostics,
documentation, benchmarks, and workspace manifests. Before parallel edits,
assign those shared files to the coordinator or one named integration owner.
Plan leads should primarily own their disjoint CUDA and store/operations areas
and communicate required shared-shape changes rather than racing edits. If a
shared dependency makes a specific task unsafe to parallelize, serialize that
task without serializing the rest of the two plans.

## Evidence ledger

For every requirement in every individual plan, record one of:

- **implemented** with source and test owners;
- **verified existing** with named inspected code and freshly run evidence;
- **selected policy** with the compared alternatives, named current workloads,
  measurements, and accepted decision record;
- **explicitly out of scope** only when the original document or individual
  plan says so.

No requirement may remain `partial`, `future`, `TBD`, inferred from adjacent
behavior, or supported only by prose. Do not weaken the plan to make the ledger
green. Green tests do not compensate for a missing numbered section, and newly
written tests do not compensate for a test-only implementation.

Plans 07–09 must leave their component evidence in their named decisions,
tests, operations/reference documents, and performance reports. Plan 10 must
consolidate the final evidence into its required executable system-conformance
ledger.

## Stage 0: Validate the completed baseline

Before Plan 07 implementation:

- verify Plan 06 still meets its full definition of done;
- run its ordinary and CUDA workspace gates;
- run its CUDA agreement and sanitizer procedure on the supported device;
- inspect the recorded out-of-core scale evidence and confirm its executable
  command remains available;
- repair any regression before proceeding rather than making a later plan
  compensate for it.

The baseline validation is not permission to reimplement Plan 06 or broaden an
individual later plan into its lifecycle/cache/storage ownership.

## Phase A: Complete Plans 07 and 08 concurrently

After the baseline passes, start Plan 07 and Plan 08 together. Their product
boundaries are independent after Plan 06: Plan 07 owns accelerator execution
and request-shape policy, while Plan 08 owns RocksDB operations, maintenance,
backup/restore, and store observability.

Run separate requirement ledgers, workstream ownership, tests, evidence, and
completion audits for the two plans. Finishing one track does not require
waiting to begin unrelated work in the other track. Reuse the freed agents to
accelerate or independently audit the remaining track.

### Plan 07 track

Implement every section of Plan 07. Do not stop at parallel kernels: complete
the evidence-based target, profile, reset, batching, delta, automatic dispatch,
path, and finite-budget policies; diagnostics and admission; adversarial
agreement; sanitizer; and named performance evidence.

Plan 07 is complete only when its own definition of done and completion audit
pass and Plan 06 remains fully passing. Commit the coherent Plan-07 boundary,
or retain it as an explicitly identified integration commit if shared Plan-08
files make an isolated commit unsafe. Record the completed boundary and keep
the master goal active.

### Plan 08 track

Implement every section of Plan 08. This includes real shutdown ownership,
RocksDB checkpoint and validated offline restore, inspection tooling, structured
store/maintenance metrics, workload measurement and selected current store
configuration, bounded maintenance/disk behavior, the rebuild-versus-overlay
decision, and recovery rehearsals.

Do not implement an overlay inside Plan 08. If its evidence triggers the plan's
separate-overlay condition, that is a material scope branch requiring the
reviewed plan called for by Plan 08; do not falsely mark Plan 08 or this master
plan complete.

Plan 08 is complete only when its own definition of done and completion audit
pass and Plan 06 remains fully passing. Commit the coherent Plan-08 boundary,
or retain it as an explicitly identified integration commit if shared Plan-07
files make an isolated commit unsafe. Record the completed boundary and keep
the master goal active.

### Phase-A integration barrier

Do not begin production implementation of Plan 09 until both Plans 07 and 08
have passed their definitions of done and audits. At the barrier:

- integrate shared engine/configuration/health/diagnostic changes;
- run the combined ordinary and CUDA regression suites;
- reconcile any conflicting public shapes or documentation;
- ensure both plan ledgers and decisions remain complete after integration;
- create a combined Plan-07/08 integration commit when their shared-file changes
  could not be separated safely.

Read-only Plan-09 mapping and encoding/process-boundary benchmark preparation
may begin while Phase A is finishing, but its facade and DTO implementation
must consume the finalized post-barrier shapes.

## Phase B: Complete Plan 09

Implement every section of Plan 09 after consuming the finalized capabilities,
health, operations, shutdown, and error shapes from Plans 07–08. Actually run
the required process-boundary and encoding comparisons before selecting them.
Implement the selected facade, bounded DTOs and decoder, lossless canonical
encoding, subgraph round trip, request identity/cancellation, error taxonomy,
capabilities, integration harness, malformed-input corpus, and API cleanup.

If evidence selects a hosted boundary outside Plan 09's implementation scope,
follow the plan's reviewed-transport branch rather than quietly substituting an
in-process facade or adding an improvised service.

Plan 09 is complete only when its own definition of done and completion audit
pass and Plans 06–08 remain fully passing. Commit the coherent Plan-09 boundary,
record the commit, and continue directly to Phase C.

## Phase C: Complete Plan 10

Implement every section of Plan 10 as system closure, not as a place to waive
unfinished component work. Return component defects to their owning code and
repair them. Complete the executable conformance ledger, all remaining decision
records, deterministic generated verification, numeric/name/lifecycle/resource
campaigns, corruption/crash/backup/restore equivalence, benchmark harness,
observability reconciliation, dependency/license inventory, API/panic/safety
audit, and documentation reconciliation.

Run the full real-device CUDA, sanitizer, release benchmark, and at-least-12-GiB
out-of-core scale matrix required by Plan 10. Wait for long-running generation,
benchmark, and recovery commands. Do not replace measurements with estimates or
an earlier plan's historical result when Plan 10 requires a rerun after the
final implementation.

Plan 10 is complete only when its own definition of done and independent audit
pass, every conformance-ledger row is resolved, and the complete system matches
the original document.

## Verification floor

Run every command required by the individual plans. At minimum, the final tree
must pass:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps

cargo check --workspace --all-targets --features cuda
cargo clippy --workspace --all-targets --features cuda -- -D warnings
cargo test --workspace --features cuda
```

Also run the complete generated agreement corpus, subprocess crash/recovery
campaign, checkpoint/restore rehearsal, canonical malformed/round-trip corpus,
CUDA memcheck and racecheck, named release benchmark suites, 12-GiB out-of-core
scale suite, dependency/license audit, and conformance-ledger stale-reference
check required by Plan 10.

Record exact commands, date, machine/toolchain, feature flags, CUDA device and
driver, configurations, correctness status, and important measurements in the
designated evidence documents. Timing is accepted only after the corresponding
result agrees with its correctness oracle.

## Review and commit discipline

Before each individual-plan commit:

1. inspect the complete diff and `git status`;
2. run `git diff --check` and the affected ordinary/CUDA gates;
3. confirm new formats and public behavior are documented;
4. confirm no unrelated user files, generated databases, large bundles, or
   benchmark scratch are staged;
5. obtain the independent completion-audit result;
6. commit with a message naming the completed product boundary.

The master plan, roadmap, and Plans 07–10 are task inputs. If they are untracked
when work begins, include them in repository history without staging unrelated
user files. Logical per-plan commits are preferred for reviewability, but an
intermediate commit is never a terminal condition.

After the final Plan-10 repair, rerun the complete final matrix, inspect the
aggregate commit range and final worktree, and create the final coherent commit
if any closure changes remain.

## Definition of done

This master plan is complete only when:

- Plans 07, 08, 09, and 10 each satisfy every numbered section and their full
  definitions of done;
- every relevant statement and deliberately open decision in
  `PATHHYDRA_SYSTEM_SHAPE.md` has implemented, selected-policy, or explicit
  out-of-scope evidence in the final conformance ledger;
- Plan 06 and all earlier behavior remain passing;
- required ordinary, CUDA, sanitizer, adversarial, crash/recovery,
  backup/restore, encoding, benchmark, scale, dependency, and documentation
  gates have actually passed;
- independent subagent audits have no unresolved findings;
- current code, decisions, fixtures, reference documentation, operational
  procedures, and performance reports describe one coherent pre-release
  system;
- all intended changes are committed in reviewable commits and remaining
  worktree changes are confirmed unrelated user files;
- no implementation requirement has been deferred beyond this master plan.

Only then may the persistent goal be marked complete and a final response sent
to the user. The final response must list the commit range, the four completed
plan boundaries, the decisive verification and scale evidence, and any
unrelated pre-existing worktree files left untouched.
