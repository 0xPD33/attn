# <Feature Name>

Date: <YYYY-MM-DD>
Status: Planned

## Context

Why this change is being made — the problem or need it addresses, what prompted it, the intended outcome. 1–2 paragraphs. Should be readable by someone who hasn't been in the conversation that led here.

## Goal

One paragraph describing what we're building and the success criteria. Concrete enough that "is this done?" has a clear answer.

## Non-goals

Explicit list of what we are NOT doing in this work. Prevents scope creep and clarifies the boundary. Each item is a thing someone might reasonably expect us to do but we explicitly aren't.

- Not doing X because Y
- Not doing Z (deferred to future plan)

## Confirmed parameters

Concrete numbers and decisions that define the design space. These are the values everyone has agreed on already — implementation can trust them without re-asking.

- **Quantitative budgets**: world size, fps target, max latency, memory ceiling
- **Library/tech choices**: chosen with one-line rationale
- **Backwards-compat constraints**: what must stay working

## Hard constraints

Things that MUST be true for the implementation to be acceptable. Violating any of these means the plan failed.

- **File scope**: which files can/cannot be touched
- **API contracts**: what existing systems depend on
- **Performance**: minimum acceptable targets
- **Build/test**: what must pass before declaring a phase done

## Tech picks

Specific libraries, patterns, or APIs chosen, with one-line rationale each. Skip libraries that are already in use unless they're being used in a new way.

- **`lib-name@version`** — what for, why this choice
- **Built-in `Foo`** — for X, vs alternative Y because Z

## Architecture overview

Where things live. File tree of new and modified files, plus the entry-point signature.

```
src/<area>/
  newFile.ts                  // what it does
  subfolder/
    anotherFile.ts            // what it does
modified:
  existingFile.ts             // what changes
```

Entry-point signature:
```ts
export function entryPoint(opts: EntryOpts): EntryReturn;
```

---

## Phase 1 — <Phase Name>

**Goal:** One sentence describing what this phase produces.

### Files to create
- `path/to/newFile.ts` — what it contains
- `path/to/another.ts` — what it contains

### Files to modify
- `path/to/existing.ts` — what changes here and why

### Key signatures
```ts
export interface Foo {
  bar: number;
  baz(x: number): boolean;
}
export function createFoo(opts: FooOpts): Foo;
```

### Acceptance
- Concrete observable behavior 1 (e.g., "running `bun run dev` and visiting `/?world=v2` shows X")
- Concrete observable behavior 2
- Build/test gate (e.g., "`bunx tsc --noEmit` clean")

### Risks
- **Risk**: short description. **Mitigation**: how we handle it.

---

## Phase 2 — <Phase Name>

(repeat the same structure for each phase)

---

## Phase N — <Final Phase Name>

(...)

---

## Performance budget

(Include this section only if performance matters for this feature.)

| Subsystem | Resource | Budget |
|---|---|---|
| ... | ... | ... |

## Open questions / decisions deferred to implementation

Numbered list of things we genuinely don't know yet, with brief context. The implementing agent can decide these on the fly.

1. Should X use approach A or B? Decide after profiling.
2. Default value for Y — start with N, tune based on observation.

## Rollback plan

How to safely back out the change if it turns out worse than what it replaces.

1. Step-by-step rollback procedure
2. Files to revert
3. Config flags to flip

## Critical files for implementation

The 3–5 most important files an implementing agent should focus on first. These are the files where the bulk of the design lives.

- `path/to/entry.ts` — entry point and orchestration
- `path/to/core.ts` — main algorithm
- `path/to/wiring.ts` — how it plugs into existing systems
