//! `modmed registration`: what to enter on ModMed's vendor dashboard to
//! register the household's own app (`registration/v1`), and whether its
//! client is stored yet. Reads the config and the vault's file names only:
//! no Keychain, no network.

use pk_cli_core::{output, CliError};
use serde_json::Value;

use super::{to_json, Ctx};
use crate::credentials;
use crate::registration::{registration, REDIRECT_PAGE};

pub fn run(ctx: &Ctx) -> Result<(), CliError> {
    let config = ctx.config()?;
    let redirect = config
        .redirect_page
        .clone()
        .unwrap_or_else(|| REDIRECT_PAGE.to_string());
    let mut payload = to_json(&registration(&redirect, config.redirect_page.is_some()))?;
    payload["clients"] = credentials::clients_stored(&ctx.vault);
    output::emit(ctx.json(), "registration", payload, render);
    Ok(())
}

fn render(v: &Value) {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    println!("Register your own app with ModMed: {}", s(&v["dashboard"]));
    println!("Step by step: {}", s(&v["doc"]));
    println!();
    println!("1. Sign-up (\"Register as New User\" on the sign-in page)");
    fields(&v["sign_up"]);
    println!();
    println!("2. App Details (\"Register New App\")");
    fields(&v["app"]);
    println!();
    println!("3. Scopes: check these, and nothing else");
    for g in v["scopes"].as_array().into_iter().flatten() {
        let check: Vec<String> = g["check"].as_array().into_iter().flatten().map(s).collect();
        let leave: Vec<String> = g["leave"].as_array().into_iter().flatten().map(s).collect();
        let check = if check.is_empty() {
            "none".to_string()
        } else {
            check.join(" ")
        };
        let leave = if leave.is_empty() {
            String::new()
        } else {
            format!(" (leave {} unchecked)", leave.join(", "))
        };
        println!("   {}: {check}{leave}", s(&g["heading"]));
    }
    println!();
    println!("4. Then");
    for (i, step) in v["then"].as_array().into_iter().flatten().enumerate() {
        println!("   {}. {}", i + 1, s(step));
    }
    println!();
    for c in v["clients"].as_array().into_iter().flatten() {
        println!(
            "Client ({}): {}",
            s(&c["environment"]),
            if c["stored"] == true {
                "stored"
            } else {
                "not stored"
            }
        );
    }
}

fn fields(list: &Value) {
    for f in list.as_array().into_iter().flatten() {
        let value = f["value"].as_str().unwrap_or_default();
        let value = if value.is_empty() { "(empty)" } else { value };
        match f["note"].as_str() {
            Some(note) => println!(
                "   {}: {value}  ({note})",
                f["label"].as_str().unwrap_or_default()
            ),
            None => println!("   {}: {value}", f["label"].as_str().unwrap_or_default()),
        }
    }
}
