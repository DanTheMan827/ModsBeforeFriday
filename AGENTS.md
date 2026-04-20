# MBF Core/Agent Integration Notes

This repository keeps a strict split:

- `mbf-core`: portable request/response protocol and pure dispatch logic.
- `mbf-agent`: native host implementation that owns all external effects.
- `mbf-site`: external frontend (do not modify from Rust refactor tasks).

## Consuming the mbf-core C API (general guidance)

When adding or consuming a C ABI surface for `mbf-core`, keep it thin and behavior-preserving:

1. Export only FFI-safe types (`repr(C)` structs, primitive integers, pointers, lengths).
2. Keep exported functions non-generic and panic-safe (`catch_unwind` at boundary).
3. Pass protocol payloads as UTF-8 JSON bytes or explicit `(ptr,len)` buffers.
4. Return explicit status/error codes plus owned output buffers.
5. Define ownership clearly:
   - caller allocates input
   - callee allocates returned buffers
   - provide a `free_*` function for every returned owned allocation
6. Keep ABI wrappers as adapters only; business logic stays in `mbf-core` runtime/model modules.
7. Ensure wrappers remain compatible with both native and wasm builds (no OS/runtime assumptions in `mbf-core`).

## Implementing the host layer (general guidance)

`mbf-core` routes requests through the host interface (`mbf_core::runtime::Host`).
If a future task asks for a host implementation:

1. Implement the trait in `mbf-agent` (or target runtime crate), not in `mbf-core`.
2. Map each request variant to existing behavior-preserving handlers.
3. Keep all external effects in the host implementation:
   - filesystem
   - process execution
   - networking
4. Preserve streaming behavior for large IO; avoid unnecessary full buffering.
5. Do not add OS detection or OS APIs inside `mbf-core`.
6. Keep request/response schema unchanged unless the task explicitly requires protocol changes.
7. Validate with targeted checks (`cargo check/test`) for changed crates and wasm checks for `mbf-core` when possible.
