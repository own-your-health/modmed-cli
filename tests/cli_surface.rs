//! Black-box tests of the binary's surface. None reaches the Keychain or
//! the network: the service is an empty test one, the config and the cache
//! are temp dirs whose config points the directory at a closed port on
//! 127.0.0.1 (or a stand-in serving synthetic pages), and every case renders
//! help or info, works on an empty profile, or is refused by argument
//! checks before any credential read.

mod support;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

/// A fresh scratch dir per call, its config pointing the directory at a
/// closed port. The counter, not the clock, makes it unique: parallel tests
/// read the same instant often enough on macOS.
fn tmp() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "modmed-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config/modmed")).unwrap();
    std::fs::write(
        dir.join("config/modmed/config.json"),
        r#"{"directory_url":"http://127.0.0.1:9/fhir/r4/Endpoint"}"#,
    )
    .unwrap();
    dir
}

fn modmed(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("modmed").unwrap();
    cmd.env("MODMED_KEYCHAIN_SERVICE", "piekstra.modmed.hermetic-tests")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("MODMED_CACHE_DIR", home.join("cache"))
        .env("HOME", home)
        .env_remove("MODMED_CONFIG")
        .env_remove("MODMED_PROFILE")
        .env_remove("NO_COLOR");
    cmd
}

fn json_of(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap_or_else(|e| {
        panic!(
            "not one JSON document ({e}): {}",
            String::from_utf8_lossy(out)
        )
    })
}

fn stdout_json(cmd: &mut Command) -> Value {
    let out = cmd.assert().success();
    json_of(&out.get_output().stdout)
}

#[test]
fn every_command_renders_help() {
    let home = tmp();
    for args in [
        vec!["--help"],
        vec!["orgs", "--help"],
        vec!["orgs", "refresh", "--help"],
        vec!["orgs", "list", "--help"],
        vec!["orgs", "search", "--help"],
        vec!["profiles", "--help"],
        vec!["profiles", "list", "--help"],
        vec!["profiles", "add", "--help"],
        vec!["profiles", "remove", "--help"],
        vec!["auth", "--help"],
        vec!["auth", "set-credential", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "status", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["patients", "--help"],
        vec!["patients", "list", "--help"],
        vec!["records", "--help"],
        vec!["records", "export", "--help"],
        vec!["documents", "--help"],
        vec!["documents", "list", "--help"],
        vec!["documents", "download", "--help"],
        vec!["api", "--help"],
        vec!["config", "--help"],
        vec!["config", "path", "--help"],
        vec!["config", "show", "--help"],
        vec!["config", "set", "--help"],
        vec!["config", "unset", "--help"],
        vec!["self-update", "--help"],
        vec!["completions", "--help"],
        vec!["info", "--help"],
    ] {
        modmed(&home)
            .args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn version_and_completions() {
    let home = tmp();
    modmed(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("modmed "));
    for shell in ["bash", "zsh", "fish"] {
        modmed(&home)
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("modmed"));
    }
}

#[test]
fn info_is_cli_info() {
    let home = tmp();
    let v = stdout_json(modmed(&home).arg("info"));
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "modmed");
    assert_eq!(v["auth"]["method"], "browser-session");
    assert_eq!(v["profiles"], json!(["health-records/v1", "documents/v1"]));
    // Login commands take --profile, so medrec passes one.
    assert!(v["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("login-profiles")));
    assert!(v.to_string().contains("piekstra/modmed-cli"));
    // No MCP surface.
    assert!(!v.to_string().contains("mcp"));
}

#[test]
fn orgs_come_from_the_directory_downloaded_once_a_week() {
    let home = tmp();
    let directory = support::directory::DirectoryServer::start();
    modmed(&home)
        .args(["config", "set", "directory_url", &directory.url()])
        .assert()
        .success();
    let v = stdout_json(modmed(&home).args(["orgs", "list", "--json"]));
    assert_eq!(v["schema"], "org-list/v1");
    let ids: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "democlinic.mmi",
            "exampledermatology.mmi",
            "exampleskin.mmi",
            "sampleeye.ef"
        ]
    );
    let first = &v["items"][1];
    assert_eq!(first["vendor"], "modmed");
    assert_eq!(
        first["fhir_base"],
        "https://exampledermatology.mmi.prod.fhir.ema-api.com/fhir/r4"
    );
    assert_eq!(first["location"], "Springfield, ST");
    // Two pages, read once: the next commands answer from the cache.
    assert_eq!(directory.requests(), 2);
    let v = stdout_json(modmed(&home).args(["orgs", "search", "springfield", "--json"]));
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let v = stdout_json(modmed(&home).args(["orgs", "list", "--state", "st", "--json"]));
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    let v = stdout_json(modmed(&home).args(["orgs", "list", "--limit", "1", "--json"]));
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    modmed(&home)
        .args(["orgs", "search", "somewhere else"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("fewer words"));
    assert_eq!(directory.requests(), 2);
    modmed(&home)
        .args(["orgs", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Example Dermatology Associates"));

    // A refresh before the week is up keeps the copy; --force downloads.
    let v = stdout_json(modmed(&home).args(["orgs", "refresh", "--json"]));
    assert_eq!(v["schema"], "org-refresh/v1");
    assert_eq!(v["refreshed"], false);
    assert_eq!(v["organizations"], 4);
    let v = stdout_json(modmed(&home).args(["orgs", "refresh", "--force", "--json"]));
    assert_eq!(v["refreshed"], true);
    assert_eq!(directory.requests(), 4);

    // When the directory fails, the old copy still answers, with a warning.
    directory.fail(true);
    let index = home.join("cache/orgs.json");
    let mut cached: Value =
        serde_json::from_str(&std::fs::read_to_string(&index).unwrap()).unwrap();
    cached["fetched_at"] = 0.into();
    std::fs::write(&index, cached.to_string()).unwrap();
    modmed(&home)
        .args(["orgs", "list", "--json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("using the copy from"));
    // Without a copy, it is an upstream error.
    std::fs::remove_file(&index).unwrap();
    modmed(&home).args(["orgs", "list"]).assert().code(5);
}

#[test]
fn the_directory_is_never_read_from_another_host() {
    let home = tmp();
    modmed(&home)
        .args([
            "config",
            "set",
            "directory_url",
            "https://example.com/Endpoint",
        ])
        .assert()
        .code(2);
    // The closed port tmp() configures: an upstream error, nothing cached.
    modmed(&home).args(["orgs", "list"]).assert().code(5);
    assert!(!home.join("cache/orgs.json").exists());
}

#[test]
fn a_login_command_needs_a_configured_profile_and_never_picks_one() {
    let home = tmp();
    // None configured: the remedy adds one.
    modmed(&home)
        .args(["auth", "status"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("modmed profiles add <NAME>"));
    let v = stdout_json(modmed(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v, json!({"schema": "profile-list/v1", "profiles": []}));
    let v = stdout_json(modmed(&home).args(["profiles", "add", "adult1", "--json"]));
    assert_eq!(
        v,
        json!({"schema": "profile-add/v1", "profile": "adult1", "added": true})
    );
    // One configured is still not picked for the caller.
    for args in [
        vec!["auth", "status"],
        vec!["auth", "logout"],
        vec!["auth", "login", "--org", "test"],
        vec!["patients", "list"],
        vec!["records", "export", "--ndjson"],
        vec!["documents", "list"],
        vec!["api", "GET", "Condition"],
    ] {
        modmed(&home)
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("configured: adult1"));
    }
    modmed(&home)
        .args(["auth", "status", "--profile", "adult2"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("modmed profiles add adult2"));
    // A command that does not run as a profile refuses --profile, so
    // `profiles remove` is never told two names.
    for args in [
        vec!["orgs", "list"],
        vec!["config", "show"],
        vec!["info"],
        vec![
            "auth",
            "set-credential",
            "--from-env",
            "MODMED_TEST_UNSET_VAR",
        ],
        vec!["profiles", "remove", "adult1", "--force"],
    ] {
        modmed(&home)
            .args(&args)
            .args(["--profile", "adult1"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("takes no --profile"));
    }
    for bad in ["Adult1", "_client", "a/b"] {
        modmed(&home)
            .args(["profiles", "add", bad])
            .assert()
            .code(2);
    }
    let v = stdout_json(modmed(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v["profiles"], json!([{"name": "adult1"}]));
}

#[test]
fn nothing_stored_reads_as_such_without_the_keychain() {
    let home = tmp();
    modmed(&home)
        .args(["profiles", "add", "adult1"])
        .assert()
        .success();
    let v = stdout_json(modmed(&home).args(["auth", "status", "--profile", "adult1", "--json"]));
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], true);
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["method"], "browser-session");
    assert_eq!(v["credential_in_keychain"], false);
    assert_eq!(v["profile"], "adult1");
    assert_eq!(v["clients"].as_array().unwrap().len(), 2);
    assert_eq!(v["patients"], json!([]));
    // MODMED_PROFILE stands in for --profile.
    modmed(&home)
        .env("MODMED_PROFILE", "adult1")
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not stored"));
    let v = stdout_json(modmed(&home).args(["auth", "logout", "--profile", "adult1", "--json"]));
    assert_eq!(
        v,
        json!({
            "schema": "auth-logout/v1",
            "profile": "adult1",
            "removed": [],
            "signed_out": [],
            "shared_kept_for": [],
            "vault_cleared": false,
        })
    );
    let v = stdout_json(modmed(&home).args(["patients", "list", "--profile", "adult1", "--json"]));
    assert_eq!(v["schema"], "patient-list/v1");
    assert_eq!(v["items"], json!([]));
    // With nothing signed in, a read is an auth error before any request.
    for args in [
        vec!["records", "export", "--ndjson"],
        vec!["documents", "list"],
        vec!["api", "GET", "Condition"],
    ] {
        modmed(&home)
            .args(&args)
            .args(["--profile", "adult1"])
            .assert()
            .code(3)
            .stderr(predicate::str::contains(
                "modmed auth login --profile adult1 --org <ORG>",
            ));
    }
    // Removing it asks; off a terminal without --force that is exit 6.
    modmed(&home)
        .args(["profiles", "remove", "adult1", "--json"])
        .assert()
        .code(6);
    let v = stdout_json(modmed(&home).args(["profiles", "remove", "adult1", "--force", "--json"]));
    assert_eq!(v["schema"], "profile-remove/v1");
    assert_eq!(v["profile"], "adult1");
    assert_eq!(v["sign_ins_removed"], 0);
    assert!(v["browser_profile_removed"].is_boolean());
    let v = stdout_json(modmed(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v["profiles"], json!([]));
    // Nothing was ever stored, so modmed made no vault.
    assert!(!home
        .join("config/modmed/vault.piekstra.modmed.hermetic-tests")
        .exists());
}

#[test]
fn bad_arguments_are_refused_before_anything_else() {
    let home = tmp();
    for (args, code) in [
        (vec!["nope"], 2),
        (vec!["orgs", "search", "  "], 2),
        (
            vec!["auth", "set-credential", "--stdin", "--from-env", "X"],
            2,
        ),
        (vec!["auth", "login"], 2),
        (vec!["auth", "login", "--org", " "], 2),
        (
            vec!["auth", "login", "--org", "test", "--label", "has space"],
            2,
        ),
        (vec!["auth", "login", "--org", "test", "--timeout", "0"], 2),
        (vec!["orgs", "list", "--state", "XYZ"], 2),
        (vec!["orgs", "list", "--limit", "0"], 2),
        (vec!["documents", "list", "--since", "2024-13-01"], 2),
        (
            vec![
                "documents",
                "list",
                "--since",
                "2024-02-01",
                "--until",
                "2024-01-01",
            ],
            2,
        ),
        (vec!["documents", "download"], 2),
        (vec!["documents", "download", "doc-1"], 2),
        (vec!["records", "export"], 2),
        (vec!["records", "export", "--ndjson", "--json"], 2),
        (vec!["records", "export", "--ndjson", "-o", "x"], 2),
        (
            vec!["records", "export", "--ndjson", "--since", "2024-13-01"],
            2,
        ),
        (vec!["api", "POST", "Patient"], 2),
        (vec!["config", "set", "nope", "x"], 2),
        (vec!["config", "set", "redirect_port", "80"], 2),
        (
            vec!["config", "set", "test_fhir_base", "http://example.com/fhir"],
            2,
        ),
        (
            vec![
                "config",
                "set",
                "test_fhir_base",
                "https://example.mmi.prod.fhir.ema-api.com/fhir/r4",
            ],
            2,
        ),
        (
            vec![
                "config",
                "set",
                "redirect_page",
                "http://example.github.io/cb",
            ],
            2,
        ),
        (vec!["profiles", "add", "Adult1"], 2),
        (vec!["profiles", "remove", "nobody", "--force"], 2),
    ] {
        modmed(&home).args(&args).assert().code(code);
    }
}

#[test]
fn a_malformed_pair_is_refused_without_echoing_it() {
    let home = tmp();
    let out = modmed(&home)
        .args(["auth", "set-credential", "--stdin", "--json"])
        .write_stdin("has space-secret-value-1")
        .assert()
        .code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).to_string()
        + &String::from_utf8_lossy(&out.get_output().stdout);
    assert!(!err.contains("secret-value-1"), "{err}");
    modmed(&home)
        .args([
            "auth",
            "set-credential",
            "--from-env",
            "MODMED_TEST_UNSET_VAR",
        ])
        .env_remove("MODMED_TEST_UNSET_VAR")
        .assert()
        .code(2);
}

#[test]
fn errors_are_one_json_document() {
    let home = tmp();
    let out = modmed(&home)
        .args(["config", "set", "nope", "x", "--json"])
        .assert()
        .code(2);
    let v = json_of(&out.get_output().stdout);
    assert_eq!(v["error"]["code"], "usage");
}

#[test]
fn config_round_trips() {
    let home = tmp();
    modmed(&home)
        .args(["config", "set", "redirect_port", "8123"])
        .assert()
        .success();
    let v = stdout_json(modmed(&home).args(["config", "show", "--json"]));
    assert_eq!(v["redirect_uri"], "http://127.0.0.1:8123/callback");
    let v = stdout_json(modmed(&home).args(["config", "path", "--json"]));
    let path = v["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("modmed/config.json"), "{path}");
    modmed(&home)
        .args(["config", "unset", "redirect_port"])
        .assert()
        .success();
    let v = stdout_json(modmed(&home).args(["config", "show", "--json"]));
    assert_eq!(v["redirect_uri"], "http://127.0.0.1:7470/callback");
    modmed(&home)
        .args([
            "config",
            "set",
            "redirect_page",
            "https://example.github.io/modmed-cli/callback",
        ])
        .assert()
        .success();
    let v = stdout_json(modmed(&home).args(["config", "show", "--json"]));
    assert_eq!(
        v["redirect_uri"],
        "https://example.github.io/modmed-cli/callback"
    );
}

#[test]
fn no_color_is_accepted() {
    let home = tmp();
    modmed(&home)
        .env("NO_COLOR", "1")
        .args(["profiles", "list"])
        .assert()
        .success();
}
