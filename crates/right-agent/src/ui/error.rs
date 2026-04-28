//! Sentinel error: the caller already rendered a brand block (e.g. dependency
//! probe). The runner should exit nonzero without `miette` re-printing.
//!
//! Pattern at the call site:
//!
//! ```ignore
//! match probe(...) {
//!     Ok(()) => {}
//!     Err(e) if e.is::<BlockAlreadyRendered>() => std::process::exit(1),
//!     Err(e) => return Err(e),
//! }
//! ```

use std::fmt;

/// Sentinel: the caller has already rendered a brand-conformant rail block to
/// stdout/stderr explaining the failure, and miette should NOT add its own
/// "Error: ..." formatting on top. The CLI dispatcher catches this variant
/// and exits with code 1 silently.
#[derive(Debug, miette::Diagnostic)]
pub struct BlockAlreadyRendered;

impl fmt::Display for BlockAlreadyRendered {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result { Ok(()) }
}

impl std::error::Error for BlockAlreadyRendered {}
