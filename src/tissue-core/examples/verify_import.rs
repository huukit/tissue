//! Verify that every `issue.md` under an issue repo parses via tissue-core.
//! Used to check imported data (#15); run after
//! `tools/import_github_issues.py` has produced an issue repo.
//!
//! Usage: `cargo run -p tissue-core --example verify_import -- <dir>`

use std::path::PathBuf;

fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: verify_import <issue-repo-root>");
    let dir = PathBuf::from(root).join(".tissue").join("issues");
    let dir = dir.as_path();
    let mut n = 0;
    let mut fail = 0;
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", dir.display());
        std::process::exit(1);
    }) {
        let p = entry.unwrap().path().join("issue.md");
        if !p.exists() {
            continue;
        }
        match tissue_core::human::IssueFile::parse(&std::fs::read_to_string(&p).unwrap()) {
            Ok(_) => n += 1,
            Err(e) => {
                fail += 1;
                eprintln!("FAIL {}: {e}", p.display());
            }
        }
    }
    println!("parsed ok: {n}, failed: {fail}");
    assert!(fail == 0, "{fail} issue file(s) failed to parse");
}
