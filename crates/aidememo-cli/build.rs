use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let canonical_dir = manifest_dir.join("../../aidememo-skill");
    let bundled_dir = manifest_dir.join("assets/aidememo-skill");

    for name in ["SKILL.md", "REFERENCE.md"] {
        let canonical = canonical_dir.join(name);
        if !canonical.exists() {
            continue;
        }

        println!("cargo:rerun-if-changed={}", canonical.display());
        let canonical_bytes = std::fs::read(&canonical).unwrap_or_else(|error| {
            eprintln!("failed to read {}: {error}", canonical.display());
            std::process::exit(1);
        });
        let bundled = bundled_dir.join(name);
        let bundled_bytes = std::fs::read(&bundled).unwrap_or_else(|error| {
            eprintln!("failed to read {}: {error}", bundled.display());
            std::process::exit(1);
        });
        if canonical_bytes != bundled_bytes {
            eprintln!(
                "{} must match {} before packaging aidememo-cli",
                bundled.display(),
                canonical.display()
            );
            std::process::exit(1);
        }
    }
}
