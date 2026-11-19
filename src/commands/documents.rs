//! `modmed documents list|download` (documents/v1) over the patient's
//! `DocumentReference` attachments.
//!
//! A document is one attachment: `<DocumentReference id>` for the first,
//! `<DocumentReference id>#<n>` for the n-th after it. Its bytes are the
//! attachment's inline `data`, or the `Binary` on the practice's server its
//! `url` names; an attachment anywhere else is not listed (medical-common
//! DESIGN §1.7).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use pk_cli_core::transport::Transport;
use pk_cli_core::{block_on, output, CliError, Reason};
use pk_cli_documents::{
    fs_safe, verify_download, DownloadBatch, Paged, RangeArgs, SavedDocument, SkippedDocument,
};
use pk_med_fhir::FhirClient;
use pk_med_records::stream::skip;
use pk_med_records::Skip;
use serde_json::Value;

use super::{sign_in_again, to_json, transport, Ctx, BIN};
use crate::cli::{DocumentsCmd, PatientArgs};
use crate::export::{attachment, Attachment};

pub fn run(ctx: &Ctx, cmd: &DocumentsCmd) -> Result<(), CliError> {
    match cmd {
        DocumentsCmd::List { who, range } => {
            range.validate()?;
            let session = open(ctx, who)?;
            let found = list_in(ctx, &session.fhir, &session.patient, range)?;
            let mut page = Paged::new(
                "document",
                found.documents.into_iter().map(|d| d.document).collect(),
            );
            page.total = Some(page.items.len() as u64);
            if ctx.json() {
                output::json(&with_withheld(to_json(&page)?, &found.withheld)?);
            } else {
                page.emit(false);
            }
            Ok(())
        }
        DocumentsCmd::Download {
            id,
            all,
            output,
            who,
            range,
        } => {
            range.validate()?;
            if *all {
                if output.exists() && !output.is_dir() {
                    return Err(CliError::Usage(format!(
                        "--all saves into a directory; {} is a file",
                        output.display()
                    )));
                }
                download_all(ctx, who, range, output)
            } else {
                let id = id.as_deref().unwrap_or_default().trim();
                if id.is_empty() {
                    return Err(CliError::Usage(
                        "give a document id from `documents list`".into(),
                    ));
                }
                download_one(ctx, who, id, output)
            }
        }
    }
}

/// A signed-in patient's FHIR client.
struct Open<T: Transport> {
    fhir: FhirClient<T>,
    patient: String,
}

fn open(ctx: &Ctx, who: &PatientArgs) -> Result<Open<pk_cli_http::BlockingTransport>, CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let context = ctx.signed_in(&config, &profile, who)?;
    let live = ctx.live(&profile, &context)?;
    let t = transport()?;
    let source = live.clone();
    let (p, c) = (profile.clone(), context.clone());
    let fhir = FhirClient::new(t, &live.fhir_base(), BIN, env!("CARGO_PKG_VERSION"))?
        .bearer_with(move |refused| source.bearer(refused).map_err(|e| sign_in_again(e, &p, &c)));
    Ok(Open {
        fhir,
        patient: context.id,
    })
}

/// What `documents list` found: the documents, and what the server said it
/// left out of the searches.
pub struct Found {
    pub documents: Vec<Listed>,
    /// medical-common DESIGN §1.4's `forbidden` skips, which §1.7 carries as
    /// `withheld` in `document-list/v1` and `document-download-batch/v1`.
    pub withheld: Vec<Skip>,
}

/// `dto` with `withheld` (DESIGN §1.7), unless there is none. `documents/v1`
/// has no field for it; a reader that does not know it ignores it.
fn with_withheld(mut dto: Value, withheld: &[Skip]) -> Result<Value, CliError> {
    if !withheld.is_empty() {
        dto["withheld"] = to_json(&withheld)?;
    }
    Ok(dto)
}

/// Where a listed document's bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bytes {
    /// A Binary on the practice's server, by id.
    Binary(String),
    /// The attachment's base64 `data`.
    Inline(String),
}

/// A listed document and where its bytes are.
pub struct Listed {
    pub document: pk_cli_documents::Document,
    pub bytes: Bytes,
    pub content_type: Option<String>,
}

fn text(v: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|p| v.pointer(p).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The most of a document id a file name keeps.
const MAX_STEM: usize = 100;

/// A file name's part for a document id. A file name past 255 bytes cannot
/// be written, so a long id keeps its first characters and gains a hash of
/// the whole id, which keeps two long ids with the same start apart.
fn file_stem(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let safe = fs_safe(id);
    if safe.len() <= MAX_STEM {
        return safe;
    }
    let digest = Sha256::digest(id.as_bytes());
    let hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{hash}", &safe[..MAX_STEM - 17])
}

/// The file extension for a MIME type, and the `verify_download` label.
pub fn extension(content_type: Option<&str>) -> (&'static str, Option<&'static str>) {
    let ct = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match ct.as_str() {
        "application/pdf" => ("pdf", Some("pdf")),
        "text/html" => ("html", Some("html")),
        "text/rtf" | "application/rtf" => ("rtf", Some("rtf")),
        "text/plain" => ("txt", Some("txt")),
        "application/xml" | "text/xml" | "application/hl7-v3+xml" => ("xml", Some("xml")),
        "image/jpeg" => ("jpg", None),
        "image/png" => ("png", None),
        "image/tiff" => ("tif", None),
        "image/gif" => ("gif", None),
        _ => ("bin", None),
    }
}

/// The documents of one DocumentReference on the server at `base`.
pub fn documents_of(v: &Value, base: &str) -> Vec<Listed> {
    let Some(id) = v["id"].as_str() else {
        return Vec::new();
    };
    let date = text(v, &["/date", "/context/period/start"])
        .map(|d| d.chars().take(10).collect::<String>());
    let name = text(v, &["/description", "/type/text", "/type/coding/0/display"]);
    let category = text(
        v,
        &[
            "/category/0/text",
            "/category/0/coding/0/display",
            "/category/0/coding/0/code",
        ],
    );
    v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| &c["attachment"])
        .filter(|a| a.is_object())
        .filter_map(|a| {
            let bytes = match attachment(a, base) {
                Attachment::Binary(id) => Bytes::Binary(id),
                Attachment::Inline => Bytes::Inline(a["data"].as_str()?.to_string()),
                _ => return None,
            };
            Some((bytes, a))
        })
        .enumerate()
        .map(|(n, (bytes, a))| {
            let content_type = a["contentType"].as_str().map(str::to_string);
            let (ext, _) = extension(content_type.as_deref());
            let doc_id = if n == 0 {
                id.to_string()
            } else {
                format!("{id}#{}", n + 1)
            };
            let mut d = pk_cli_documents::Document::new(
                doc_id,
                name.clone()
                    .or_else(|| a["title"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "Document".into()),
            );
            d.date = date.clone();
            d.category = category.clone();
            d.file = Some(format!(
                "{}{}.{ext}",
                date.as_deref()
                    .map(|d| format!("{}_", fs_safe(d)))
                    .unwrap_or_default(),
                file_stem(&d.id)
            ));
            Listed {
                document: d,
                bytes,
                content_type,
            }
        })
        .collect()
}

fn in_range(date: Option<&str>, range: &RangeArgs) -> bool {
    let Some(d) = date else {
        return range.since.is_none() && range.until.is_none();
    };
    range.since.as_deref().is_none_or(|s| d >= s) && range.until.as_deref().is_none_or(|u| d <= u)
}

/// The patient's documents in `range`, newest first. The search is by
/// `patient`, bounded by `date` under `--since`; a server that refuses it
/// as a bad request is asked again by US Core's `clinical-note` category.
fn list_in<T: Transport>(
    ctx: &Ctx,
    fhir: &FhirClient<T>,
    patient: &str,
    range: &RangeArgs,
) -> Result<Found, CliError> {
    let base = fhir.base().to_string();
    let mut params = vec![("patient", patient.to_string())];
    if let Some(since) = &range.since {
        params.push(("date", format!("ge{since}")));
    }
    let found = match block_on(fhir.search("DocumentReference", &params)) {
        Ok(found) => found,
        Err(e) if e.reason() == Some(Reason::BadRequest) => {
            params.push(("category", "clinical-note".to_string()));
            block_on(fhir.search("DocumentReference", &params))?
        }
        Err(e) => return Err(e),
    };
    let mut withheld = Vec::new();
    for line in found.withheld() {
        ctx.note(format!(
            "warning: the practice left out some DocumentReference results: {line}"
        ));
        withheld.push(Skip::resource(
            "DocumentReference",
            "",
            skip::FORBIDDEN,
            line,
        ));
    }
    let mut out: Vec<Listed> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in &found.resources {
        if !seen.insert(r.id.clone()) {
            continue;
        }
        if let Ok(v) = r.value() {
            out.extend(documents_of(&v, &base));
        }
    }
    out.retain(|d| in_range(d.document.date.as_deref(), range));
    out.sort_by(|a, b| {
        b.document
            .date
            .cmp(&a.document.date)
            .then(a.document.id.cmp(&b.document.id))
    });
    if let Some(n) = range.limit {
        out.truncate(n as usize);
    }
    Ok(Found {
        documents: out,
        withheld,
    })
}

/// A document's bytes, checked to be what its type says. `Err(Ok(skip))` is
/// a document skipped (`code`, `reason`); `Err(Err(e))` a failure that ends
/// the command (the sign-in or the network).
fn fetch<T: Transport>(
    fhir: &FhirClient<T>,
    d: &Listed,
) -> Result<Vec<u8>, Result<(&'static str, String), CliError>> {
    let (data, content_type) = match &d.bytes {
        Bytes::Inline(data) => (data.clone(), d.content_type.clone()),
        Bytes::Binary(id) => {
            let b = block_on(fhir.binary(&format!("Binary/{id}"))).map_err(|e| {
                if (e.exit_code() == 3 && e.reason() != Some(Reason::Forbidden))
                    || e.reason() == Some(pk_med_fhir::UNREACHABLE)
                {
                    Err(e)
                } else {
                    Ok((
                        match e.exit_code() {
                            3 => skip::FORBIDDEN,
                            4 => skip::NOT_FOUND,
                            _ => skip::UPSTREAM,
                        },
                        e.message().to_string(),
                    ))
                }
            })?;
            let v = b.value().unwrap_or_default();
            let Some(data) = v["data"].as_str() else {
                return Err(Ok((skip::NO_FILE, "the Binary carries no data".into())));
            };
            (
                data.to_string(),
                v["contentType"]
                    .as_str()
                    .map(str::to_string)
                    .or(d.content_type.clone()),
            )
        }
    };
    let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .map_err(|e| Ok((skip::VERIFY_FAILED, format!("the data is not base64: {e}"))))?;
    match extension(content_type.as_deref()).1 {
        Some(label) => verify_download(&bytes, Some(label)),
        None if bytes.is_empty() => Err(CliError::Upstream("the document is empty".into())),
        None => Ok(()),
    }
    .map_err(|e| Ok((skip::VERIFY_FAILED, e.message().to_string())))?;
    Ok(bytes)
}

fn target(output: &Path, d: &Listed) -> PathBuf {
    if output.is_dir() {
        output.join(d.document.file.as_deref().unwrap_or("document.bin"))
    } else {
        output.to_path_buf()
    }
}

fn download_one(ctx: &Ctx, who: &PatientArgs, id: &str, output: &Path) -> Result<(), CliError> {
    let id = id.strip_suffix("#1").unwrap_or(id);
    let session = open(ctx, who)?;
    let docs = list_in(ctx, &session.fhir, &session.patient, &RangeArgs::default())?.documents;
    let Some(d) = docs.iter().find(|d| d.document.id == id) else {
        return Err(
            CliError::NotFound(format!("no document `{id}` for this patient"))
                .with_remedy(format!("list them with `{BIN} documents list`")),
        );
    };
    let bytes = fetch(&session.fhir, d).map_err(|e| match e {
        Ok((_, reason)) => CliError::Upstream(format!("document `{id}`: {reason}")),
        Err(e) => e,
    })?;
    let path = target(output, d);
    crate::fsutil::write_private(&path, &bytes)?;
    let saved =
        SavedDocument::from_document(&d.document, path.display().to_string(), bytes.len() as u64);
    pk_cli_documents::emit(&saved, ctx.json());
    Ok(())
}

fn download_all(
    ctx: &Ctx,
    who: &PatientArgs,
    range: &RangeArgs,
    dir: &Path,
) -> Result<(), CliError> {
    let session = open(ctx, who)?;
    let found = list_in(ctx, &session.fhir, &session.patient, range)?;
    crate::fsutil::create_private_dir(dir)?;
    let (mut saved, mut skipped) = (Vec::new(), Vec::new());
    for d in &found.documents {
        match fetch(&session.fhir, d) {
            Ok(bytes) => {
                let path = target(dir, d);
                crate::fsutil::write_private(&path, &bytes)?;
                saved.push(SavedDocument::from_document(
                    &d.document,
                    path.display().to_string(),
                    bytes.len() as u64,
                ));
            }
            Err(Ok((code, reason))) => {
                skipped.push(SkippedDocument::new(d.document.id.clone(), reason).with_code(code))
            }
            Err(Err(e)) => return Err(e),
        }
    }
    let batch = DownloadBatch::new(dir.display().to_string(), saved).with_skipped(skipped);
    if ctx.json() {
        output::json(&with_withheld(to_json(&batch)?, &found.withheld)?);
    } else {
        println!(
            "saved {} to {} ({} skipped)",
            match batch.count {
                1 => "1 document".to_string(),
                n => format!("{n} documents"),
            },
            batch.dir,
            batch.skipped.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.mmi.prod.fhir.ema-api.com/fhir/r4";

    #[test]
    fn withheld_is_added_to_the_output_only_when_there_is_some() {
        let dto = serde_json::json!({"schema": "document-list/v1", "items": []});
        assert_eq!(with_withheld(dto.clone(), &[]).unwrap(), dto);
        let skip = Skip::resource("DocumentReference", "", skip::FORBIDDEN, "left out");
        let out = with_withheld(dto, &[skip]).unwrap();
        assert_eq!(out["schema"], "document-list/v1");
        assert_eq!(
            out["withheld"],
            serde_json::json!([{"resource_type": "DocumentReference", "reason": "left out", "code": "forbidden"}])
        );
    }

    #[test]
    fn each_attachment_here_or_inline_is_a_document_named_safely() {
        let v: Value = serde_json::json!({
            "resourceType": "DocumentReference", "id": "doc-1", "date": "2026-01-06T10:00:00Z",
            "type": {"text": "Progress note"},
            "category": [{"coding": [{"code": "clinical-note"}]}],
            "content": [
                {"attachment": {"contentType": "application/pdf", "url": "Binary/b1"}},
                {"attachment": {"contentType": "text/plain", "data": "aGVsbG8="}},
                {"attachment": {"contentType": "text/html", "url": "https://elsewhere.example/n.html"}},
                {"attachment": {"contentType": "text/plain"}}
            ]
        });
        let docs = documents_of(&v, BASE);
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].document.id, "doc-1");
        assert_eq!(docs[0].bytes, Bytes::Binary("b1".into()));
        assert_eq!(docs[1].document.id, "doc-1#2");
        assert_eq!(docs[1].bytes, Bytes::Inline("aGVsbG8=".into()));
        assert_eq!(
            docs[0].document.file.as_deref(),
            Some("2026-01-06_doc-1.pdf")
        );
        assert_eq!(
            docs[1].document.file.as_deref(),
            Some("2026-01-06_doc-1_2.txt")
        );
        assert_eq!(docs[0].document.category.as_deref(), Some("clinical-note"));
        assert_eq!(docs[0].document.name, "Progress note");
    }

    #[test]
    fn a_long_id_keeps_its_start_and_a_hash_in_the_file_name() {
        assert_eq!(file_stem("doc-1#2"), "doc-1_2");
        let exact = "a".repeat(MAX_STEM);
        assert_eq!(file_stem(&exact), exact);
        let one = format!("{}x", "e".repeat(200));
        let two = format!("{}y", "e".repeat(200));
        let (a, b) = (file_stem(&one), file_stem(&two));
        assert_eq!(a.len(), MAX_STEM);
        assert!(a.starts_with(&"e".repeat(MAX_STEM - 17)), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn a_range_keeps_dated_documents_inside_it() {
        let range = RangeArgs {
            limit: None,
            since: Some("2026-01-01".into()),
            until: Some("2026-01-31".into()),
        };
        assert!(in_range(Some("2026-01-06"), &range));
        assert!(!in_range(Some("2025-12-31"), &range));
        assert!(!in_range(None, &range));
        assert!(in_range(None, &RangeArgs::default()));
    }
}
