# ADR-001: FITS File Support via fitsio Crate

## Status

Accepted

## Context

Rusty Photon is an astrophotography application that needs to read and write FITS (Flexible Image Transport System) files. FITS is the standard file format used in astronomy for storing images, tables, and metadata.

We needed to decide how to implement FITS file support in the workspace.

## Options Considered

### Option 1: Custom FFI Bindings

Build custom Rust bindings directly to NASA's CFITSIO C library.

**Pros:**
- Full control over the API surface
- Could optimize for specific use cases

**Cons:**
- Significant development effort
- Need to maintain FFI safety
- No clear benefit over existing solutions

### Option 2: Wrapper Crate

Create a thin wrapper crate around an existing FITS library to provide async compatibility.

**Pros:**
- Could provide a cleaner async API
- Centralized error handling

**Cons:**
- Unnecessary abstraction layer
- Async compatibility can be achieved via `spawn_blocking` at call sites
- Additional maintenance burden

### Option 3: Direct Dependency on fitsio Crate (Chosen)

Use the existing `fitsio` crate (v0.21) as a direct workspace dependency.

**Pros:**
- Well-maintained crate (latest release Sept 2025)
- Wraps NASA's CFITSIO library with safe Rust API
- Good Linux/macOS support (Tier 1)
- Minimal integration effort
- Active community and documentation

**Cons:**
- Limited Windows support (Tier 3/MSYS2)
- Synchronous API (requires `spawn_blocking` for async contexts)
- Dependency on system CFITSIO library

## Decision

We chose **Option 3: Direct dependency on fitsio crate**.

The `fitsio` crate provides a mature, well-tested wrapper around CFITSIO with good platform support for our primary targets (Linux and macOS). The synchronous API is acceptable since blocking I/O can be offloaded to a thread pool using `tokio::task::spawn_blocking` where needed.

## Consequences

### CI/CD Changes

All CI workflows must install the CFITSIO system library:
- Ubuntu: `sudo apt-get install -y libcfitsio-dev`
- macOS: `brew install cfitsio`
- Windows: Limited support, skipped in CI or requires MSYS2

### Usage Pattern

Services using fitsio in async contexts should wrap blocking calls:

```rust
use fitsio::FitsFile;

async fn read_fits_header(path: &str) -> Result<HeaderValue, Error> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        let mut fptr = FitsFile::open(&path)?;
        let hdu = fptr.primary_hdu()?;
        // ... read data
        Ok(result)
    }).await?
}
```

### Platform Support

- **Linux**: Full support (Tier 1)
- **macOS**: Full support (Tier 1)
- **Windows**: Limited support (Tier 3) - requires MSYS2 or vendored builds

## References

- [fitsio crate](https://crates.io/crates/fitsio)
- [NASA CFITSIO](https://heasarc.gsfc.nasa.gov/fitsio/)
- [FITS Standard](https://fits.gsfc.nasa.gov/fits_standard.html)
