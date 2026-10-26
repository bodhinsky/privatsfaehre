//! `privatsphaere` command-line tool.

mod config;
mod guard;
mod inputs;
mod label;
mod term;

use clap::{Parser, Subcommand};
use dataset::eval::{self, EvalReport};
use detect::{EntityType, Verdict};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "privatsphaere",
    version,
    about = "Local-only PII and secrets guard (pseudonymization, not anonymization)."
)]
struct Cli {
    /// Engine configuration (TOML with a [detect] table).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Disable ANSI colours.
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Watch the clipboard and pseudonymize it after confirmation (review mode).
    ///
    /// Special-category data (GDPR Art. 9/10) is replaced by a notice without
    /// asking. Copied text with placeholders from this session can be restored.
    /// The vault lives in memory only and is wiped on exit.
    Guard {
        /// Polling interval in milliseconds.
        #[arg(long, default_value_t = 250)]
        interval_ms: u64,
        /// Append decisions to this local JSONL file (never original values).
        #[arg(long)]
        audit_log: Option<PathBuf>,
    },
    /// Print a `[ner]` config section with the SHA-256 pins for a GLiNER model directory.
    NerConfig {
        /// Directory with model.onnx, tokenizer.json and gliner_config.json.
        dir: PathBuf,
    },
    /// Show what would be redacted or blocked (review-mode preview).
    Scan {
        /// Input file (any supported format: txt, md, pdf, docx, odt, pptx, eml, mbox,
        /// xlsx, ods, csv, json, html, rtf, …); stdin if omitted.
        file: Option<PathBuf>,
        /// Column/field holding the text in CSV, spreadsheet and JSON inputs.
        #[arg(long)]
        column: Option<String>,
        /// Print JSON instead of the highlighted preview.
        #[arg(long)]
        json: bool,
        /// Also list every raw candidate with its score.
        #[arg(long)]
        candidates: bool,
    },
    /// Interactively label candidate spans and append them to a dataset.
    Label {
        /// Files or directories. .txt/.md: records separated by `---`, inline [[TYPE|text]] allowed.
        /// Also: pdf, docx, odt, pptx, odp, rtf, html, eml, mbox, xlsx, xls, ods, csv, tsv, json, jsonl.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Dataset file to append to.
        #[arg(long, short)]
        dataset: PathBuf,
        /// Source label for all records (default: derived from the file format).
        #[arg(long)]
        source: Option<String>,
        /// Column/field holding the text in CSV, spreadsheet and JSON inputs.
        #[arg(long)]
        column: Option<String>,
        /// Your name/initials, stored with each record.
        #[arg(long)]
        labeler: Option<String>,
        /// Show candidates with at least this score (0 = everything).
        #[arg(long, default_value_t = 0.0)]
        min_score: f32,
        /// Label records again even if their id is already in the dataset.
        #[arg(long)]
        relabel: bool,
    },
    /// Evaluate the engine against labeled data.
    Eval {
        /// Dataset files or directories.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Write the full report as JSON.
        #[arg(long)]
        json_out: Option<PathBuf>,
        /// Include error cases (with text context) in the JSON report.
        #[arg(long)]
        json_errors: bool,
        /// Baseline report; fail if Art. 9/10 recall dropped.
        #[arg(long)]
        compare: Option<PathBuf>,
        /// Minimum overlap recall, e.g. HEALTH=0.95 (repeatable).
        #[arg(long, value_parser = parse_target)]
        min_recall: Vec<(EntityType, f64)>,
        /// TOML file with `[watch_mode.min_recall]` targets (see accuracy-targets.toml).
        #[arg(long)]
        targets: Option<PathBuf>,
        /// Number of FP/FN examples to print.
        #[arg(long, default_value_t = 30)]
        errors: usize,
    },
    /// Convert markup files ([[TYPE|text]]) into a JSONL dataset.
    ImportMarkup {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long, short)]
        out: PathBuf,
        /// Mark the records as synthetic (safe to publish).
        #[arg(long)]
        synthetic: bool,
        #[arg(long, default_value = "markup")]
        source: String,
    },
    /// Export spans as CSV for spreadsheet review.
    ExportCsv {
        dataset: Vec<PathBuf>,
        #[arg(long, short)]
        out: PathBuf,
        #[arg(long, default_value = ";")]
        delimiter: char,
    },
    /// Apply an edited CSV back to a dataset file (rewritten atomically).
    ImportCsv {
        csv: PathBuf,
        #[arg(long, short)]
        dataset: PathBuf,
        #[arg(long, default_value = ";")]
        delimiter: char,
    },
    /// Dataset statistics.
    Stats {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Check that dataset files parse and offsets match.
    Validate {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

fn parse_target(s: &str) -> Result<(EntityType, f64), String> {
    let (e, v) = s.split_once('=').ok_or("expected ENTITY=VALUE")?;
    let e: EntityType = e.parse().map_err(|e| format!("{e}"))?;
    let v: f64 = v.parse().map_err(|_| format!("bad number `{v}`"))?;
    if !(0.0..=1.0).contains(&v) {
        return Err("value must be in [0, 1]".into());
    }
    Ok((e, v))
}

type Res = Result<ExitCode, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn delimiter(c: char) -> Result<u8, String> {
    u8::try_from(c).ok().filter(u8::is_ascii).ok_or_else(|| "delimiter must be ASCII".to_owned())
}

fn run(cli: Cli) -> Res {
    let color = !cli.no_color && term::color_supported();
    let engine = || config::engine(cli.config.as_deref());
    match cli.cmd {
        Cmd::Guard { interval_ms, audit_log } => {
            let audit = audit_log.as_deref().map(session::AuditLog::open).transpose()?;
            let session = session::Session::new(engine()?, clip::SystemClipboard::new()?, audit)?;
            let mut g = guard::Guard {
                session,
                console: guard::Terminal::new(),
                color,
                interval: Duration::from_millis(interval_ms.max(50)),
            };
            g.run()?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::NerConfig { dir } => {
            let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
            let hash = |f: &str| {
                let p = dir.join(f);
                gliner::sha256_file(&p).map_err(|e| format!("{}: {e}", p.display()))
            };
            let (model, tok) = (hash("model.onnx")?, hash("tokenizer.json")?);
            if !dir.join("gliner_config.json").is_file() {
                return Err(format!("{}: gliner_config.json missing", dir.display()).into());
            }
            println!("[ner]\nmodel_dir = {:?}\nmodel_sha256 = \"{model}\"\ntokenizer_sha256 = \"{tok}\"", dir);
            println!("# onnxruntime = \"/path/to/libonnxruntime.so\"  # default: ORT_DYLIB_PATH or the loader path");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Scan { file, column, json, candidates } => {
            let texts = match file {
                Some(p) => inputs::scan_texts(&p, &ingest::Options { column })?,
                None => {
                    let mut text = String::new();
                    io::stdin().read_to_string(&mut text)?;
                    vec![("stdin".to_owned(), text)]
                }
            };
            let engine = engine()?;
            let mut blocked = false;
            let many = texts.len() > 1;
            for (origin, text) in &texts {
                if many && !json {
                    println!("\n════ {origin} ════");
                }
                blocked |= scan(&engine, text, json, candidates, color)? == Verdict::Block;
            }
            Ok(if blocked { ExitCode::from(4) } else { ExitCode::SUCCESS })
        }
        Cmd::Label { inputs, dataset: ds, source, column, labeler, min_score, relabel } => {
            let opts = ingest::Options { column };
            label::run(&engine()?, &inputs, &ds, source.as_deref(), &opts, labeler, min_score, relabel, color)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Eval { paths, json_out, json_errors, compare, mut min_recall, targets, errors } => {
            if let Some(t) = targets {
                min_recall.extend(config::targets(&t)?);
            }
            let records = dataset::load(&paths)?;
            let mut report = eval::evaluate(&engine()?, &records)?;
            println!("{}", eval::render_text(&report, errors));
            let mut failures = eval::check_min_recall(&report, &min_recall);
            if let Some(base) = compare {
                let baseline: EvalReport = serde_json::from_str(&fs::read_to_string(&base)?)?;
                failures.extend(eval::compare_special_recall(&baseline, &report));
            }
            if let Some(out) = json_out {
                if !json_errors {
                    report.errors.clear();
                }
                fs::write(out, serde_json::to_string_pretty(&report)?)?;
            }
            if failures.is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!("Accuracy gate failed:");
                for f in &failures {
                    eprintln!("  {f}");
                }
                Ok(ExitCode::from(3))
            }
        }
        Cmd::ImportMarkup { files, out, synthetic, source } => {
            let mut n = 0;
            for f in &files {
                let default_source =
                    f.file_stem().and_then(|s| s.to_str()).filter(|_| source == "markup").unwrap_or(&source);
                for r in dataset::markup::parse(&fs::read_to_string(f)?, default_source, synthetic)
                    .map_err(|e| format!("{}: {e}", f.display()))?
                {
                    dataset::append(&out, &r)?;
                    n += 1;
                }
            }
            eprintln!("Appended {n} records to {}", out.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::ExportCsv { dataset: ds, out, delimiter: d } => {
            let records = dataset::load(&ds)?;
            dataset::csv_io::export(&records, fs::File::create(&out)?, delimiter(d)?)?;
            eprintln!("Wrote {} spans to {}", records.iter().map(|r| r.spans.len()).sum::<usize>(), out.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::ImportCsv { csv, dataset: ds, delimiter: d } => {
            let mut records = dataset::load(std::slice::from_ref(&ds))?;
            let stats = dataset::csv_io::import(&mut records, fs::File::open(&csv)?, delimiter(d)?)?;
            dataset::write_all(&ds, &records)?;
            eprintln!(
                "Updated {}, added {}, deleted {} spans in {}",
                stats.updated,
                stats.added,
                stats.deleted,
                ds.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Stats { paths, json } => {
            let s = eval::stats(&dataset::load(&paths)?);
            if json {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                println!("records: {} (complete {}, synthetic {})", s.records, s.complete, s.synthetic);
                println!("spans by verdict: {:?}", s.by_verdict);
                println!("records by source: {:?}", s.by_source);
                println!("gold spans by entity:");
                for (e, n) in &s.gold_by_entity {
                    let mark = if e.is_special_category() { " ⛔" } else { "" };
                    println!("  {e:<24} {n:>5}{mark}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Validate { paths } => {
            let files = dataset::jsonl_files(&paths)?;
            let mut n = 0;
            for f in &files {
                n += dataset::read_jsonl(f)?.len();
            }
            println!("OK: {n} records in {} files", files.len());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn scan(
    engine: &detect::Engine,
    text: &str,
    json: bool,
    candidates: bool,
    color: bool,
) -> Result<Verdict, Box<dyn std::error::Error>> {
    let analysis = engine.analyze(text)?;
    let verdict = analysis.verdict();
    let mut vault = vault::Vault::new();
    let spans = analysis.detections.iter().map(|d| vault::Span { start: d.start, end: d.end, kind: d.entity.as_str() });
    let pseudonymized = vault.pseudonymize(text, spans)?;
    let cands = if candidates { engine.candidates(text)? } else { Vec::new() };

    if json {
        let det = |d: &detect::Detection| {
            serde_json::json!({
                "start": d.start, "end": d.end, "entity": d.entity, "score": d.score,
                "recognizer": d.recognizer, "text": d.text(text),
            })
        };
        let out = serde_json::json!({
            "verdict": verdict,
            "detections": analysis.detections.iter().map(det).collect::<Vec<_>>(),
            "blocking": analysis.blocking.iter().map(det).collect::<Vec<_>>(),
            "pseudonymized": if verdict == Verdict::Block { None } else { Some(&pseudonymized) },
            "candidates": cands.iter().map(det).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        let mut marks: Vec<term::Mark> =
            analysis.detections.iter().map(|d| term::Mark::new(d.start, d.end, false)).collect();
        marks.extend(analysis.blocking.iter().map(|d| term::Mark::new(d.start, d.end, true)));
        println!("── Original (highlighted) ──\n{}\n", term::highlight(text, &marks, color));
        for d in analysis.blocking.iter().chain(&analysis.detections) {
            let art = d.entity.gdpr_article().map(|a| format!(" [{a}]")).unwrap_or_default();
            println!("  {:<22} {:.2} {:<26} {:?}{art}", d.entity.as_str(), d.score, d.recognizer, d.text(text));
        }
        match verdict {
            Verdict::Block => {
                println!(
                    "\n⛔ BLOCKED: special-category data (GDPR Art. 9/10) detected. This text must not be pasted."
                );
            }
            Verdict::Clean => println!("\n✓ Nothing detected."),
            Verdict::Redact => println!("\n── Pseudonymized ──\n{pseudonymized}"),
        }
        if candidates {
            println!("\n── All candidates ──");
            for d in &cands {
                println!("  {:<22} {:.2} {:<26} {:?}", d.entity.as_str(), d.score, d.recognizer, d.text(text));
            }
        }
    }
    Ok(verdict)
}
