use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tetra-entities should live under crates/tetra-entities")
        .to_path_buf()
}

#[test]
fn external_dashboard_asset_manifest_is_coherent() {
    let root = workspace_root();
    let index_path = root.join("dashboard/index.html");
    let app_path = root.join("dashboard/assets/app.js");
    let css_path = root.join("dashboard/assets/styles.css");

    let index = std::fs::read_to_string(&index_path).expect("dashboard index.html should exist");
    let app = std::fs::read_to_string(&app_path).expect("dashboard app.js should exist");
    let css = std::fs::read_to_string(&css_path).expect("dashboard styles.css should exist");

    assert!(
        index.contains(r#"<link rel="stylesheet" href="/assets/styles.css">"#),
        "index.html must reference the deploy-copied stylesheet"
    );
    assert!(
        index.contains(r#"<script src="/assets/app.js" defer></script>"#),
        "index.html must reference the deploy-copied application script"
    );
    assert!(
        app.contains(r#"fetch("/api/system""#),
        "external dashboard must keep using the core-owned system API"
    );
    assert!(
        app.contains(r#"/ws`"#),
        "external dashboard must keep using the core-owned WebSocket endpoint"
    );
    assert!(
        !css.contains("letter-spacing: -"),
        "dashboard CSS should not use negative letter spacing"
    );
}
