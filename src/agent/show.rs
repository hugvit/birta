//! `birta show`: a doc's exact bytes, as they are on disk or at a rev its review file
//! recorded. It only reads: no lock, no sync, no claim.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::{ExitCode, ShowArgs};
use crate::rev::Rev;
use crate::review::chain::Chain;
use crate::review::fold::{self, RoundKind};
use crate::review::log::Log;
use crate::review::ops::{Op, Parsed};
use crate::review::review_path_for;

/// Revs an unknown rev's error names, newest first.
const RECENT: usize = 5;
/// Hex digits a rev prefix needs, so a short one can't pick a rev by luck.
const MIN_PREFIX: usize = 6;

/// Which recorded bytes the target names, past its file.
enum Spec {
    Latest,
    Approved,
    Prefix(String),
}

struct Refusal {
    code: ExitCode,
    msg: String,
}

fn failed(msg: impl Into<String>) -> Refusal {
    Refusal {
        code: ExitCode::Error,
        msg: msg.into(),
    }
}

pub fn run(args: &ShowArgs) -> ExitCode {
    let (bytes, warning) = match show(&args.target) {
        Ok(found) => found,
        Err(Refusal { code, msg }) => {
            eprintln!("show: {msg}");
            return code;
        }
    };
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
    let mut out = io::stdout().lock();
    match out.write_all(&bytes).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::Ok,
        // The reader took what it wanted and left, as `| head` does
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::Ok,
        Err(e) => {
            eprintln!("show: writing stdout: {e}");
            ExitCode::Error
        }
    }
}

/// The bytes to print, and a warning for stderr.
fn show(target: &str) -> Result<(Vec<u8>, Option<String>), Refusal> {
    let (file, spec) = split(target)?;
    let name = file.display().to_string();
    let doc = canonical(&file).ok_or_else(|| failed(format!("not found: {name}")))?;
    let disk = read_file(&doc);
    let Some(spec) = spec else {
        return disk
            .map(|bytes| (bytes, None))
            .ok_or_else(|| failed(format!("not found: {name}")));
    };
    let parsed = Log::open(&review_path_for(&doc))
        .read()
        .map_err(|e| failed(format!("reading the review file: {e}")))?;
    let disk_rev = disk.as_deref().map(Rev::of);
    match spec {
        Spec::Latest => {
            let parsed = parsed.ok_or_else(|| no_log(&name))?;
            let chain = Chain::build(&parsed);
            let head = chain
                .head()
                .cloned()
                .ok_or_else(|| failed(format!("{name} has no recorded rev")))?;
            Ok((text_of(&chain, &head)?, None))
        }
        Spec::Approved => {
            let parsed = parsed.ok_or_else(|| no_log(&name))?;
            let rev = approved(&parsed)
                .ok_or_else(|| failed(format!("{name} has no approval standing")))?;
            let bytes = text_of(&Chain::build(&parsed), &rev)?;
            let warning = (disk_rev.as_ref() != Some(&rev)).then(|| {
                format!("{name} changed since it was approved; these are the approved bytes")
            });
            Ok((bytes, warning))
        }
        Spec::Prefix(prefix) => {
            let recorded = parsed.as_ref().map(recorded_revs).unwrap_or_default();
            let mut matches: Vec<&Rev> = recorded
                .iter()
                .chain(disk_rev.as_ref())
                .filter(|rev| rev.as_str().starts_with(&prefix))
                .collect();
            matches.sort_by_key(|rev| rev.as_str());
            matches.dedup();
            match matches[..] {
                [] => {
                    let recent: Vec<&str> = recorded
                        .iter()
                        .rev()
                        .take(RECENT)
                        .map(|rev| &rev.as_str()[..8])
                        .collect();
                    let known = if parsed.is_none() {
                        "and it has no review file".to_string()
                    } else {
                        format!("recent revs: {}", recent.join(" "))
                    };
                    Err(failed(format!("no rev {prefix} in {name}; {known}")))
                }
                [rev] if disk_rev.as_ref() == Some(rev) => Ok((disk.unwrap_or_default(), None)),
                [rev] => {
                    let parsed = parsed.ok_or_else(|| no_log(&name))?;
                    Ok((text_of(&Chain::build(&parsed), rev)?, None))
                }
                _ => {
                    let revs: Vec<&str> = matches.iter().map(|rev| rev.as_str()).collect();
                    Err(Refusal {
                        code: ExitCode::Quote,
                        msg: format!(
                            "rev {prefix} is ambiguous in {name}: {}; give more digits",
                            revs.join(" ")
                        ),
                    })
                }
            }
        }
    }
}

/// `FILE`, `FILE@REV`, `FILE@approved` or `FILE@latest`. A whole argument naming a file
/// is that file, and otherwise the rev follows the last `@`.
fn split(target: &str) -> Result<(PathBuf, Option<Spec>), Refusal> {
    if Path::new(target).symlink_metadata().is_ok() {
        return Ok((PathBuf::from(target), None));
    }
    let Some((file, rev)) = target.rsplit_once('@').filter(|(file, _)| !file.is_empty()) else {
        return Ok((PathBuf::from(target), None));
    };
    let spec = match rev {
        "latest" => Spec::Latest,
        "approved" => Spec::Approved,
        hex if (MIN_PREFIX..=40).contains(&hex.len())
            && hex.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Spec::Prefix(hex.to_ascii_lowercase())
        }
        _ => {
            return Err(Refusal {
                code: ExitCode::Usage,
                msg: format!(
                    "{rev:?} is not a rev: give {MIN_PREFIX} to 40 hex digits, approved or latest"
                ),
            });
        }
    };
    Ok((PathBuf::from(file), Some(spec)))
}

/// The doc's canonical path, which names its review file even when the doc is gone.
fn canonical(file: &Path) -> Option<PathBuf> {
    file.canonicalize().ok().or_else(|| {
        let name = file.file_name()?;
        let parent = match file.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        Some(parent.canonicalize().ok()?.join(name))
    })
}

/// A regular file's bytes.
fn read_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|_| std::fs::read(path).ok())
}

fn no_log(name: &str) -> Refusal {
    failed(format!("{name} has no review file"))
}

/// `rev`'s text as the chain rebuilds it, checked against the rev itself.
fn text_of(chain: &Chain, rev: &Rev) -> Result<Vec<u8>, Refusal> {
    chain
        .text(rev)
        .map(|text| text.as_bytes().to_vec())
        .filter(|bytes| Rev::of(bytes) == *rev)
        .ok_or_else(|| failed(format!("text for {rev} is not recoverable")))
}

/// The rev of the latest approve nobody withdrew.
fn approved(parsed: &Parsed) -> Option<Rev> {
    let n = fold::scan(parsed, 0)
        .rounds
        .iter()
        .rev()
        .find(|r| r.kind == RoundKind::Approve && r.withdrawn.is_none())?
        .n;
    parsed.lines.iter().find_map(|l| match &l.op {
        Op::Approve { rev, .. } if l.n == n => Some(rev.clone()),
        _ => None,
    })
}

/// Every rev the log records the doc at, once each, oldest first by last mention.
fn recorded_revs(parsed: &Parsed) -> Vec<Rev> {
    let mut last: HashMap<&Rev, usize> = HashMap::new();
    let named = parsed.lines.iter().flat_map(|line| match &line.op {
        Op::Snap { rev, .. } => vec![rev],
        Op::Edit { rev, to_rev, .. } | Op::External { rev, to_rev, .. } => vec![rev, to_rev],
        _ => Vec::new(),
    });
    for (i, rev) in named.enumerate() {
        last.insert(rev, i);
    }
    let mut revs: Vec<(&Rev, usize)> = last.into_iter().collect();
    revs.sort_by_key(|&(_, i)| i);
    revs.into_iter().map(|(rev, _)| rev.clone()).collect()
}
