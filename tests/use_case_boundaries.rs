mod guardrail_support;

use std::path::Path;

use guardrail_support::{collect_rust_files, production_tokens};

const FORBIDDEN_PATTERNS: &[&str] = &["clap::", "crate::cli::", "crate::output::", "crate::mcp::"];

fn assert_missing(path: &Path, forbidden: &str) {
    let production = production_tokens(path);
    assert!(
        !production.contains(forbidden),
        "{} must not import {}",
        path.display(),
        forbidden
    );
}

#[test]
fn use_cases_do_not_depend_on_transport_or_presentation_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("use_cases");
    let files = collect_rust_files(&root);
    for expected in ["build_project.rs", "result.rs", "workspace_lock.rs"] {
        assert!(
            files
                .iter()
                .any(|path| path.file_name().is_some_and(|name| name == expected)),
            "expected recursive scan to include src/use_cases/{expected}"
        );
    }

    for file in &files {
        for forbidden in FORBIDDEN_PATTERNS {
            assert_missing(file, forbidden);
        }
    }
}
