pub mod budget;
pub mod chain;
pub mod check;
pub mod claudemd;
pub mod error;
pub mod markdown;
pub mod model;
pub mod ratchet;
pub mod usage;
pub mod workflows;

// Re-export key types for downstream consumers
pub use check::{CheckConfig, CheckContext, Checker, FsSource, Report, SkillSource};
pub use error::{CheckKind, LintError, ParseCheckKindError};
pub use model::{SkillEntry, SkillFrontmatter, SkillMap, SkillMetadata};
