//! The doc as it stood at each rev the fold asks about, worked out as cheaply as the
//! log allows: a note's lines are carried through the recorded hunks, and a text is
//! rebuilt or parsed only when that fails.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::rev::Rev;
use crate::review::chain::{Chain, Mapped, Step, map_line};
use crate::review::doc::{Doc, LineIndex};
use crate::review::hunk;
use crate::review::ladder::{self, Located};
use crate::review::ops::{Anchor, Hunk, Scope};
use crate::review::state::AnchorStatus;

pub(super) struct Texts<'a> {
    pub chain: &'a Chain,
    /// The current text.
    pub head: &'a Doc,
    texts: HashMap<Rev, Option<Arc<str>>>,
    docs: HashMap<Rev, Option<Arc<Doc>>>,
    lines: HashMap<Rev, Option<Arc<LineIndex>>>,
    /// Notes placed on the current text, by line.
    located: HashMap<u32, Located>,
    /// A note's span at a rev, by note line.
    spans: HashMap<(u32, Rev), Option<(u32, u32)>>,
    /// Chain steps by the log line that made them, leaving out broken ones.
    steps: HashMap<u32, usize>,
}

impl<'a> Texts<'a> {
    pub fn new(chain: &'a Chain, head: &'a Doc) -> Self {
        let steps = (0..chain.history().len())
            .filter(|&i| !chain.broke(i))
            .map(|i| (chain.history()[i].n, i))
            .collect();
        Self {
            chain,
            head,
            texts: HashMap::new(),
            docs: HashMap::new(),
            lines: HashMap::new(),
            located: HashMap::new(),
            spans: HashMap::new(),
            steps,
        }
    }

    /// The step log line `n` made, and its index in the chain's history.
    pub fn step_at(&self, n: u32) -> Option<(usize, &'a Step)> {
        let i = *self.steps.get(&n)?;
        Some((i, &self.chain.history()[i]))
    }

    /// Loads the texts of `revs` in one pass over the log.
    pub fn prefetch(&mut self, revs: impl IntoIterator<Item = Rev>) {
        let missing: HashSet<Rev> = revs
            .into_iter()
            .filter(|rev| *rev != self.head.rev && !self.texts.contains_key(rev))
            .collect();
        if missing.is_empty() {
            return;
        }
        let mut found = self.chain.texts(&missing);
        for rev in missing {
            let text = found.remove(&rev);
            self.texts.insert(rev, text);
        }
    }

    pub fn text(&mut self, rev: &Rev) -> Option<Arc<str>> {
        if *rev == self.head.rev {
            return Some(self.head.text.clone());
        }
        let chain = self.chain;
        self.texts
            .entry(rev.clone())
            .or_insert_with(|| chain.text(rev))
            .clone()
    }

    /// Runs `f` on the doc at `rev`, parsing it at most once.
    pub fn with_doc<R>(&mut self, rev: &Rev, f: impl FnOnce(&Doc) -> R) -> Option<R> {
        if *rev == self.head.rev {
            return Some(f(self.head));
        }
        if !self.docs.contains_key(rev) {
            let doc = self.text(rev).map(|text| Arc::new(Doc::new(&text)));
            self.docs.insert(rev.clone(), doc);
        }
        self.docs[rev].as_deref().map(f)
    }

    /// Lines of the text at `rev`.
    pub fn lines(&mut self, rev: &Rev) -> Option<Arc<LineIndex>> {
        if !self.lines.contains_key(rev) {
            let index = self.text(rev).map(|text| Arc::new(LineIndex::new(&text)));
            self.lines.insert(rev.clone(), index);
        }
        self.lines[rev].clone()
    }

    /// Note `n`'s anchor placed on the current text.
    pub fn located(&mut self, n: u32, anchor: &Anchor) -> &Located {
        let (head, chain) = (self.head, self.chain);
        self.located
            .entry(n)
            .or_insert_with(|| ladder::relocate(anchor, head, chain))
    }

    /// The lines note `n`'s verdict or change covers in the text of `rev`: its anchor's
    /// block, or its block's or section's lines.
    pub fn span_at(&mut self, n: u32, anchor: &Anchor, rev: &Rev) -> Option<(u32, u32)> {
        let key = (n, rev.clone());
        if let Some(span) = self.spans.get(&key) {
            return *span;
        }
        let carried = self.base(n, anchor).and_then(|(from, span)| {
            if from == *rev {
                return Some(span);
            }
            follow(self.chain.steps(&from, rev)?, span)
        });
        let chain = self.chain;
        let span = carried.or_else(|| {
            self.with_doc(rev, |doc| {
                span_of(anchor, &ladder::relocate(anchor, doc, chain))
            })
            .flatten()
        });
        self.spans.insert(key, span);
        span
    }

    /// The span on the text the anchor was made on, where no parse is needed for it.
    fn base(&mut self, n: u32, anchor: &Anchor) -> Option<(Rev, (u32, u32))> {
        if anchor.scope != Scope::Quote {
            return Some((anchor.rev.clone(), anchor.lines));
        }
        let head_rev = self.head.rev.clone();
        let located = self.located(n, anchor);
        // Only a quote the chain carried here can be carried back
        if !matches!(located.status, AnchorStatus::Ok | AnchorStatus::Moved) {
            return None;
        }
        let block = located.block?;
        if anchor.rev == head_rev {
            return Some((head_rev, block));
        }
        let steps = self.chain.steps(&anchor.rev, &head_rev)?;
        let back = (map_back(steps, block.0)?, map_back(steps, block.1)?);
        Some((anchor.rev.clone(), back))
    }
}

/// The lines a located anchor covers: a quote's block, or a block's or section's lines.
pub(super) fn span_of(anchor: &Anchor, located: &Located) -> Option<(u32, u32)> {
    match anchor.scope {
        Scope::Quote => located.block,
        Scope::Block | Scope::Section => located.lines,
    }
}

/// Whether hunk `h`, in its old text, touches lines `span`.
pub(super) fn touches(h: &Hunk, (first, last): (u32, u32)) -> bool {
    let a = h.a;
    if h.d.is_empty() {
        // Inserted after line `a`
        return first <= a && a <= last;
    }
    let end = a + h.d.len() as u32;
    a < last && first <= end
}

/// `span` after `steps`, if no step changed either end.
pub(super) fn carry(steps: &[Step], (first, last): (u32, u32)) -> Option<(u32, u32)> {
    Some((
        mapped(map_line(steps, first))?,
        mapped(map_line(steps, last))?,
    ))
}

/// `span` after `steps`, grown or shrunk to what replaced its ends; `None` once a step
/// removes all of it.
fn follow(steps: &[Step], (mut first, mut last): (u32, u32)) -> Option<(u32, u32)> {
    for step in steps {
        let hunks = step.hunks.as_deref()?;
        (first, last) = (through(hunks, first, true), through(hunks, last, false));
        if first > last {
            return None;
        }
    }
    Some((first, last))
}

/// Where `line` goes through one step's hunks. A line a hunk replaced goes to the first
/// (`start`) or last line of what replaced it.
fn through(hunks: &[Hunk], line: u32, start: bool) -> u32 {
    let mut shift = 0i64;
    for h in hunks {
        if line <= h.a {
            break;
        }
        if line <= h.a + h.d.len() as u32 {
            let at = i64::from(h.a) + shift;
            let end = if start { at + 1 } else { at + h.i.len() as i64 };
            return end.max(0) as u32;
        }
        shift += h.i.len() as i64 - h.d.len() as i64;
    }
    (i64::from(line) + shift) as u32
}

/// Whether no hunk of `steps` touches `span` as it moves through them.
pub(super) fn untouched(steps: &[Step], mut span: (u32, u32)) -> bool {
    for step in steps {
        let Some(hunks) = &step.hunks else {
            return false;
        };
        if hunks.iter().any(|h| touches(h, span)) {
            return false;
        }
        match carry(std::slice::from_ref(step), span) {
            Some(next) => span = next,
            None => return false,
        }
    }
    true
}

fn mapped(m: Mapped) -> Option<u32> {
    match m {
        Mapped::Same(line) | Mapped::Shifted(line) => Some(line),
        Mapped::Overlapped => None,
    }
}

/// Where `line` of the text after `steps` was before them, if no step made it.
fn map_back(steps: &[Step], mut line: u32) -> Option<u32> {
    for step in steps.iter().rev() {
        let back = Step {
            n: step.n,
            from: step.to.clone(),
            to: step.from.clone(),
            hunks: Some(hunk::reverse(step.hunks.as_deref()?)),
            cause: step.cause,
        };
        line = mapped(map_line(std::slice::from_ref(&back), line))?;
    }
    Some(line)
}
