pub mod check;
pub mod error;
pub mod model;

// Re-export key types for downstream consumers
pub use check::{CheckConfig, CheckContext, Checker, FsSource, Report, SkillSource};
pub use error::{CheckKind, LintError, ParseCheckKindError};
pub use model::{SkillEntry, SkillFrontmatter, SkillMap, SkillMetadata};
