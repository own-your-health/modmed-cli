//! The pages GitHub Pages publishes from `site/`: the forwarding page a
//! practice may redirect a sign-in to (`config set redirect_page`) must pass
//! the answer to the listener modmed opens, and its Content-Security-Policy
//! must still allow its own script and style after an edit.

use base64::Engine;
use sha2::{Digest, Sha256};

fn page(name: &str) -> String {
    std::fs::read_to_string(format!("{}/site/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn inner<'a>(html: &'a str, tag: &str) -> &'a str {
    let open = format!("<{tag}>");
    let start = html.find(&open).unwrap() + open.len();
    let end = html[start..].find(&format!("</{tag}>")).unwrap();
    &html[start..start + end]
}

fn csp_hash(text: &str) -> String {
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(text.as_bytes()))
    )
}

#[test]
fn the_callback_page_forwards_to_the_default_listener() {
    let html = page("callback.html");
    let listener = modmed::config::Config::default().loopback_uri();
    assert_eq!(listener, "http://127.0.0.1:7470/callback");
    let script = inner(&html, "script");
    assert!(
        script.contains(&format!("location.replace(\"{listener}\" + query)")),
        "{script}"
    );
}

#[test]
fn the_callback_page_s_policy_allows_exactly_its_own_script_and_style() {
    let html = page("callback.html");
    let policy = html
        .split("Content-Security-Policy\" content=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(
        policy.contains(&format!("script-src {}", csp_hash(inner(&html, "script")))),
        "the script changed: recompute its hash in {policy}"
    );
    assert!(
        policy.contains(&format!("style-src {}", csp_hash(inner(&html, "style")))),
        "the style changed: recompute its hash in {policy}"
    );
    assert!(policy.starts_with("default-src 'none'"), "{policy}");
}

#[test]
fn every_page_loads_nothing_from_elsewhere() {
    for name in ["index.html", "callback.html"] {
        let html = page(name);
        assert!(html.contains("default-src 'none'"), "{name}");
        assert!(!html.contains("<script src"), "{name}");
        assert!(!html.contains("<link"), "{name}");
        assert!(!html.contains("<img"), "{name}");
    }
}
