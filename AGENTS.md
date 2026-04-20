# MBF Core/Agent Integration Notes

## Crate Responsibilities

### `mbf-core` — Portable Execution Engine

- Owns **all business logic**: request handling, mod management, patching, downgrading, etc.
- Must compile to both **native** and **`wasm32-unknown-unknown`** without modification.
- Must be exportable as a **C ABI** (`cdylib`) for use from C/C++ or other runtimes.
- **Must NOT** directly access the filesystem, network, or spawn processes.
- **Must NOT** call `std::fs`, `std::net`, `std::process::Command`, or any OS/networking library.
- **Must NOT** use `cfg!(target_os = …)` or any OS-detection logic.
- **Must NOT** depend on `mbf-agent`.
- Interacts with the outside world **exclusively** through the `Host` trait.
- Pure computation libraries (`mbf-zip`, `mbf-axml`) are acceptable dependencies.

### `mbf-agent` — Native Runtime Host

- Implements the `mbf_core::runtime::Host` trait with actual OS calls.
- Owns **all external effects**: filesystem I/O, process execution, HTTP networking.
- Is a thin adapter — no business logic lives here.
- Preserves existing CLI behaviour exactly.

### `mbf-site` — TypeScript Frontend

- **Must never be modified** from Rust refactor tasks.
- Treated as a black-box integration; the JSON protocol is its only interface.

---

## The `Host` Trait

`Host` is the **only** channel through which `mbf-core` accesses external effects.  
It provides low-level, symmetric IO primitives — not high-level operations.

### Filesystem

```rust
fn read_file(&mut self, path: &str) -> Result<Vec<u8>>;
fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()>;
fn file_exists(&mut self, path: &str) -> bool;
fn remove_file(&mut self, path: &str) -> Result<()>;
fn create_dir_all(&mut self, path: &str) -> Result<()>;
fn remove_dir_all(&mut self, path: &str) -> Result<()>;
fn copy_file(&mut self, from: &str, to: &str) -> Result<()>;
fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>>;
```

### Networking

```rust
fn http_get(&mut self, url: &str) -> Result<Vec<u8>>;
fn http_get_file(&mut self, url: &str, dest_path: &str) -> Result<Option<String>>;
```

### Process execution

```rust
fn run_command(&mut self, cmd: &str, args: &[&str]) -> Result<CommandOutput>;
```

### Configuration

```rust
fn get_config(&self) -> &CoreConfig;
```

`CoreConfig` holds all platform-specific paths (qmods dir, modloader dir, APK ID, etc.) and
is populated by `mbf-agent` from `AgentParameters` before any request is handled.

### Rules

- Every external interaction in `mbf-core` must call through one of the methods above.
- `mbf-core` must not distinguish semantically between filesystem, network, and process — they are all symmetric effect sources/sinks.
- `mbf-agent`'s `AgentHost` implements each method using `std::fs`, `ureq`, and `std::process::Command` respectively.

---

## C API

`mbf-core` exposes a C ABI so it can be consumed from C, C++, or any FFI-capable language.

### Registration (one-time setup)

The host is registered **once** by passing a vtable of function pointers:

```c
mbf_core_register_host(const MbfHostVtable *vtable);
```

`MbfHostVtable` is a `repr(C)` struct containing:
- `ctx: *mut void` — opaque caller context passed to every callback.
- One `unsafe extern "C" fn` pointer per `Host` primitive (read_file, write_file, http_get, run_command, etc.).

After registration, `mbf-core` stores the vtable globally. All subsequent calls use it.

### Dispatching a request

```c
int mbf_core_handle_request(
    const uint8_t *request_json,
    size_t         request_len,
    MbfBuffer     *out_response   // caller provides pointer; callee fills
);
```

- `request_json` — UTF-8 JSON bytes for a `RequestEnum` value.
- Returns `0` on success; `out_response` contains JSON-encoded `Response`.
- Returns non-zero on error; `out_response` contains a UTF-8 error message.
- The caller must free `out_response` with `mbf_core_free_buffer`.

### Memory ownership

```c
// Free a buffer that mbf-core allocated and returned.
void mbf_core_free_buffer(MbfBuffer buf);

// Allocate a buffer for the host to fill and return to mbf-core via a callback.
MbfBuffer mbf_core_alloc_buffer(size_t len);
```

- `mbf-core` allocates all response buffers; the caller frees them with `mbf_core_free_buffer`.
- Host callbacks allocate their output using `mbf_core_alloc_buffer`; `mbf-core` frees them after reading.
- No buffer may outlive the call in which it was created without explicit transfer of ownership.

### ABI safety rules

1. Only `repr(C)` structs, primitive integers, and raw pointers cross the ABI boundary.
2. No Rust generics or trait objects in exported functions.
3. All exported functions are wrapped in `std::panic::catch_unwind` — panics must never cross the boundary.
4. `MbfHostVtable` function pointers are nullable (`Option<unsafe extern "C" fn …>`); unimplemented methods return an error code.
5. The WASM build does not assume threading unless explicitly enabled.

---

## Current Implementation State

| Area | Status |
|---|---|
| Protocol models (`mbf-core::models`) | ✅ Done — `request` and `response` modules |
| High-level `Host` trait + `handle_request` dispatcher | ✅ Done — interim; see note below |
| `AgentHost` in `mbf-agent` implementing current trait | ✅ Done |
| WASM build (`wasm32-unknown-unknown`) | ✅ Passes `cargo check` |
| Low-level IO primitive `Host` trait | ⏳ Pending — trait must be replaced with primitives above |
| Business logic moved from `mbf-agent` to `mbf-core` | ⏳ Pending — handlers, patching, mod_man, downgrading |
| `AgentHost` reduced to OS-primitive adapter | ⏳ Pending — follows logic migration |
| C API (`mbf-core::ffi`) | ⏳ Pending — `mbf_core_register_host`, `mbf_core_handle_request`, `mbf_core_free_buffer` |

> **Note on the interim Host trait:** `mbf_core::runtime::Host` currently has one method per
> high-level request variant (e.g. `get_mod_status`, `patch`). This is a temporary bridge that
> keeps the agent working while the logic migration is in progress. Once all handler logic is in
> `mbf-core`, this trait must be replaced with the low-level IO primitives described above and
> `mbf-agent` reduced to implementing those primitives only.

---

## Hard Constraints (Non-Negotiable)

- `mbf-site` must never be modified.
- `mbf-core` must never import `std::fs`, `std::net`, `std::process`, `ureq`, `reqwest`, `hyper`, or any OS/networking library.
- `mbf-core` must never use `cfg!(target_os = …)`.
- All new logic belongs in `mbf-core`; `mbf-agent` is an adapter only.
- No panics across FFI boundaries.
- No architectural paradigms (plugin systems, ECS, event buses) — keep it minimal.
