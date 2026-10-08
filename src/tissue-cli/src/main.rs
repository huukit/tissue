//! Tissue CLI: thin layer over `tissue-core`. Commands land in #13.

fn main() {
    println!("tissue {}", tissue_core::VERSION);
}
