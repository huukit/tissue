//! #213: the committed man page `docs/man/tissue.1` must match a fresh render
//! of the clap CLI definition (CI enforces the same via `git diff --exit-code`
//! after regenerating with the gen-man example).

#[test]
fn man_page_matches_clap_definition_213() {
    let mut buf = Vec::new();
    clap_mangen::Man::new(tissue_cli::cli::command())
        .render(&mut buf)
        .expect("render man page");
    let rendered = String::from_utf8(buf).expect("man page is utf-8");
    let committed_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/man/tissue.1");
    let committed =
        std::fs::read_to_string(committed_path).expect("docs/man/tissue.1 is committed");
    assert_eq!(
        rendered, committed,
        "docs/man/tissue.1 is stale — regenerate with `cargo run -p tissue-cli --example gen-man` (from src/)"
    );
}

#[test]
fn man_page_renders_name_synopsis_and_subcommands_213() {
    let mut buf = Vec::new();
    clap_mangen::Man::new(tissue_cli::cli::command())
        .render(&mut buf)
        .expect("render man page");
    let page = String::from_utf8(buf).expect("utf-8");
    assert!(page.contains(".TH tissue 1"), "header: {page}");
    assert!(page.contains(".SH NAME"), "NAME section");
    assert!(page.contains(".SH SYNOPSIS"), "SYNOPSIS section");
    for sub in [
        "init", "create", "list", "show", "sync", "propose", "approve",
    ] {
        assert!(page.contains(sub), "subcommand {sub} documented");
    }
}
