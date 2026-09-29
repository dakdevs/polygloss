//! `Polygloss`: the GPUI app. Opens nothing yet; `--version` prints the version.

fn main() {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("Polygloss {}", polygloss_core::VERSION);
    }
}
