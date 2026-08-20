from pathlib import Path

path = Path("crates/aidememo-server/src/main.rs")
text = path.read_text()
old = "#[derive(Clone, Copy, ValueEnum)]\nenum ArtifactBackendArg {"
new = "#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]\nenum ArtifactBackendArg {"
if text.count(old) != 1:
    raise SystemExit(f"expected one ArtifactBackendArg derive, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
print("PostgreSQL profile enum comparison fix applied")
