//! #213: render `tissue(1)` from the clap CLI definition (clap_mangen) so the
//! man page can never drift from the CLI. Dev-only tool — not shipped in the
//! release binary (examples build only on demand, never into it). Run with
//! `cargo run -p tissue-cli --example gen-man`. Default output:
//! docs/man/tissue.1 at the repo root.

use std::path::PathBuf;

use clap::Parser;

/// Render tissue(1) from the clap definition.
#[derive(Parser)]
#[command(name = "gen-man", about, long_about)]
struct Args {
    /// Output path for the rendered man page.
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let out = args.out.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/man/tissue.1")
    });
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create man page directory");
    }
    let mut buf = Vec::new();
    clap_mangen::Man::new(tissue_cli::cli::command())
        .render(&mut buf)
        .expect("render man page");
    std::fs::write(&out, &buf).expect("write man page");
    eprintln!("wrote {}", out.display());
}
