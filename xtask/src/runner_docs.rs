//! `cargo xtask runner-docs`: LAB's document reader inside the runner image.
//!
//! LAB's `scripts/read_doc.py` parses .docx, .pdf, .pptx and .xlsx files with `podman run`
//! ("sandbox" mode, the one harness2 uses), and the runner image answers that call with its
//! podman stand-in (`deploy/docker/podman-standin.sh`). This reads one document of each kind
//! the LAB checkout ships, plus a generated PDF with known text (no LAB task has shipped a PDF
//! yet), inside the runner image the way a container-isolated rollout does, and requires each
//! result to be non-empty and identical to `read_doc.py`'s in-process ("host") mode. Run it
//! after a harness2 bump (the stand-in refuses a changed `podman` call) and after runner image
//! changes.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Where container jobs see the harness2 home, and LAB inside it.
const HOME_MOUNT: &str = "/h2w/harness2";
const LAB: &str = "/h2w/harness2/dependencies/harvey-labs";
const FIXTURES_MOUNT: &str = "/h2w/fixtures";

/// The generated PDF's text, one line each; ASCII, so the PDF needs no font encoding work.
const PDF_LINES: &[&str] = &[
    "Harness2 Workbench runner check",
    "Closing date: March 14, 2026",
    "Purchase price: $4,600,000 (subordinated note: Aldersgate)",
];

/// Read the documents and compare the two modes; fails listing every document that differs.
pub(crate) fn check(root: &Path, harness2_home: &str, image: &str, runtime: &str) -> Result<()> {
    let home = Path::new(harness2_home)
        .canonicalize()
        .with_context(|| format!("harness2 home {harness2_home} not found"))?;
    let lab = home.join("dependencies/harvey-labs");
    for need in [
        "opencode/scripts/read_doc.py",
        "sandbox/parsers/parse_doc.py",
        ".venv/bin/python",
    ] {
        // symlink_metadata: the environment's `python` links to an interpreter that exists
        // only inside the runner image.
        if std::fs::symlink_metadata(lab.join(need)).is_err() {
            bail!(
                "{} is missing: set LAB up in a harness2 home first (the `setup` job of a \
                 Workbench with container isolation, whose environment lives under {HOME_MOUNT})",
                lab.join(need).display()
            );
        }
    }
    let fixtures = root.join("target/runner-docs");
    if fixtures.exists() {
        std::fs::remove_dir_all(&fixtures).context("could not clear target/runner-docs")?;
    }
    std::fs::create_dir_all(&fixtures).context("could not create target/runner-docs")?;
    for p in [&home, &fixtures] {
        let s = p.to_string_lossy();
        if s.contains([':', ',']) {
            bail!("{s}: a path with ':' or ',' cannot be bind-mounted");
        }
    }
    std::fs::write(fixtures.join("generated.pdf"), pdf(PDF_LINES))
        .context("could not write the generated PDF")?;
    for ext in ["docx", "pdf", "pptx", "xlsx"] {
        match first_with_ext(&lab.join("tasks"), ext)? {
            Some(found) => {
                std::fs::copy(&found, fixtures.join(format!("lab.{ext}")))
                    .with_context(|| format!("could not copy {}", found.display()))?;
                eprintln!(
                    "{ext}: {}",
                    found.strip_prefix(&lab).unwrap_or(&found).display()
                );
            }
            None if ext == "pdf" => eprintln!("pdf: no LAB task ships one; generated.pdf only"),
            None => bail!("no .{ext} file under {}", lab.join("tasks").display()),
        }
    }

    let mut names: Vec<String> = std::fs::read_dir(&fixtures)?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for name in &names {
        let sandbox = read(runtime, image, &home, &fixtures, "sandbox", name)?;
        let host = read(runtime, image, &home, &fixtures, "host", name)?;
        let verdict = match (&sandbox, &host) {
            (Err(e), _) => Some(format!("sandbox mode failed: {e}")),
            (_, Err(e)) => Some(format!("host mode failed: {e}")),
            (Ok(s), _) if s.trim().is_empty() => Some("no text".to_owned()),
            (Ok(s), Ok(h)) if s != h => Some("differs from host mode".to_owned()),
            (Ok(s), _) if name == "generated.pdf" => PDF_LINES
                .iter()
                .find(|l| !s.contains(*l))
                .map(|l| format!("lacks the line {l:?}")),
            _ => None,
        };
        match verdict {
            None => eprintln!(
                "ok    {name}: {} characters, identical in both modes",
                sandbox.as_ref().map_or(0, String::len)
            ),
            Some(why) => {
                eprintln!("FAIL  {name}: {why}");
                failed.push(name.clone());
            }
        }
    }
    if !failed.is_empty() {
        bail!("LAB could not read {} in {image}", failed.join(", "));
    }
    eprintln!(
        "runner-docs: {} documents read through the podman stand-in",
        names.len()
    );
    Ok(())
}

/// `read_doc.py <file>` in the runner image, as a container-isolated LAB rollout runs it: no
/// network, no capabilities, LAB's interpreter first on PATH. `Ok(stdout)` on success,
/// `Err(stderr)` when the reader fails; an `Err` of the outer result means the runtime itself
/// could not run.
fn read(
    runtime: &str,
    image: &str,
    home: &Path,
    fixtures: &Path,
    mode: &str,
    name: &str,
) -> Result<std::result::Result<String, String>> {
    let out = Command::new(runtime)
        .args(["run", "--rm", "--network", "none", "--cap-drop", "ALL"])
        .args(["--security-opt", "no-new-privileges", "--user", "0:0"])
        .arg("-v")
        .arg(format!("{}:{HOME_MOUNT}:ro", home.display()))
        .arg("-v")
        .arg(format!("{}:{FIXTURES_MOUNT}:ro", fixtures.display()))
        .args(["-e", &format!("LAB_BENCH_ROOT={LAB}")])
        .args(["-e", &format!("LAB_DOC_PARSER={mode}")])
        .args([
            "-e",
            &format!("PATH={LAB}/.venv/bin:/usr/local/bin:/usr/bin:/bin"),
        ])
        .arg(image)
        .args(["python3", &format!("{LAB}/opencode/scripts/read_doc.py")])
        .arg(format!("{FIXTURES_MOUNT}/{name}"))
        .output()
        .with_context(|| format!("could not start {runtime}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    if out.status.success() {
        Ok(Ok(text(&out.stdout)))
    } else {
        let err = text(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        Ok(Err(format!("{}: {last}", out.status)))
    }
}

/// The first file with this extension under `dir`, in sorted path order.
fn first_with_ext(dir: &Path, ext: &str) -> Result<Option<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in
            std::fs::read_dir(&d).with_context(|| format!("could not read {}", d.display()))?
        {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found.into_iter().next())
}

/// A one-page PDF 1.4 with `lines` in Helvetica: objects and cross-reference table written by
/// hand, so the file is small and its text is known exactly. `lines` must be ASCII.
fn pdf(lines: &[&str]) -> Vec<u8> {
    let mut content = b"BT /F1 12 Tf 72 720 Td 16 TL".to_vec();
    for line in lines {
        let escaped = line
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        content.extend(format!(" ({escaped}) Tj T*").as_bytes());
    }
    content.extend(b" ET");
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend(&content);
    stream.extend(b"\nendstream");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
        stream,
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend(object);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Every cross-reference entry points at its object, and `startxref` at the table: the
    /// structure a PDF reader checks before it extracts any text.
    #[test]
    fn generated_pdf_is_well_formed() {
        let bytes = pdf(&["One (with parentheses)", "Two \\ backslash"]);
        let text = String::from_utf8(bytes).unwrap();
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|s| s.lines().next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(text[start..].starts_with("xref\n0 6\n"));
        let entries: Vec<usize> = text[start..]
            .lines()
            .skip(3)
            .take(5)
            .map(|l| l[..10].parse().unwrap())
            .collect();
        for (i, offset) in entries.iter().enumerate() {
            assert!(
                text[*offset..].starts_with(&format!("{} 0 obj\n", i + 1)),
                "object {}",
                i + 1
            );
        }
        let length: usize = text
            .split("/Length ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        // The stream holds exactly /Length bytes, then the keyword.
        let (_, body) = text.split_once(">>\nstream\n").unwrap();
        assert!(body[length..].starts_with("\nendstream\n"));
        assert!(text.contains("(One \\(with parentheses\\)) Tj"));
        assert!(text.contains("(Two \\\\ backslash) Tj"));
        assert!(text.ends_with("%%EOF\n"));
    }
}
