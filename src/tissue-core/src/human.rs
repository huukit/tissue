//! Human format (`issue.md` / `release.md`): YAML front matter + Markdown body.
//!
//! Contract: `operations/product-owner/SPEC-human-format.md`. Implemented in #10.

#[derive(Debug, Clone, PartialEq)]
pub struct IssueFile {
    pub front_matter: serde_yaml::Value,
    pub body: String,
}
