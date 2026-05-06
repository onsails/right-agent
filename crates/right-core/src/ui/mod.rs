//! Brand-conformant CLI presentation primitives.
//!
//! All atoms (`▐`, `▐✓`, semantic glyphs), splash, section headers, and
//! recap blocks live here. Three theme tiers: `Color`, `Mono`, `Ascii`.
//! See `docs/brand-guidelines.html` and `docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md`.

pub mod atoms;
pub mod error;
pub mod header;
pub mod line;
pub mod prompts;
pub mod recap;
pub mod splash;
pub mod theme;
pub mod writer;

pub use atoms::{Glyph, Rail};
pub use error::BlockAlreadyRendered;
pub use header::section;
pub use line::{Block, Line, status};
pub use prompts::install_global as install_prompt_render_config;
pub use recap::Recap;
pub use splash::splash;
pub use theme::{Theme, detect};
pub use writer::{stderr, stdout};
