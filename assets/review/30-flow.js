var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  /**
   * A leading `"quote"` (or `“quote”`) in a note and the words after it, or null. The quote
   * stays on one line, holds more than blanks, and ends a word.
   */
  BR.pure.parseTypedQuote = function (text) {
    var m = /^(?:"([^"\n]*)"|“([^”\n]*)”)(?=\s|$)\s*([\s\S]*)$/.exec(text || '');
    if (!m) return null;
    var quote = m[1] != null ? m[1] : m[2];
    if (!quote.trim()) return null;
    return { quote: quote, rest: m[3] };
  };

  /**
   * What is news to the reader, the count behind the tab's dot: unseen changes, then unseen agent
   * replies, open agent questions and not-handled rows, each in doc order with the doc slot first.
   * Each stop is `{kind, id, sp}`, with `id` the one its seen op would carry.
   */
  // Doc order is block order: the start line of a stop's sp, with the doc slot first
  function lineOf(sp) { return sp ? parseInt(sp, 10) : 0; }

  BR.pure.attention = function (s) {
    var out = [];
    var lost = {};
    (s.lost || []).forEach(function (n) { lost[n] = 1; });
    var removed = {};
    (s.deletions || []).forEach(function (d) { removed[d.change] = d; });
    (s.changes || []).forEach(function (c) {
      if (c.seen || c.settled || c.attr === 'human') return;
      var d = removed[c.id];
      // A removal walks at the block above its tick, or in the doc slot at the very top
      var sp = (c.range && c.range.sp) || (d && d.after_sp) || null;
      out.push({ kind: 'change', id: c.id, sp: sp, rank: 0, line: lineOf(sp), order: c.op * 1e4 + c.idx });
    });
    (s.items || []).forEach(function (i) {
      var sp = !lost[i.n] && i.at && i.at.kind !== 'doc' && i.at.loc ? i.at.loc.sp : null;
      var line = lineOf(sp);
      (i.replies || []).forEach(function (r) {
        if (r.by && r.by.type === 'agent' && !r.seen) out.push({ kind: 'reply', id: r.id, sp: sp, rank: 1, line: line, order: r.n });
      });
      if (i.by && i.by.type === 'agent' && i.verdict === 'ask' && i.status === 'open') {
        out.push({ kind: 'blank', id: '#' + i.n, sp: sp, rank: 2, line: line, order: i.n });
      }
      if (i.status === 'not_handled' && !i.seen) {
        out.push({ kind: 'not_handled', id: 'nh:' + i.n, sp: sp, rank: 3, line: line, order: i.n });
      }
    });
    out.sort(function (a, b) { return a.rank - b.rank || a.line - b.line || a.order - b.order; });
    return out;
  };

  /** Whether a hand-back after line `n` has covered it. */
  function covered(s, n) {
    return (s.rounds || []).some(function (r) { return r.submit > n; });
  }

  /**
   * What `n` and `p` walk: every stop until it is settled, that is seen and then covered by a
   * hand-back, so a round stays walkable after one pass. One top-to-bottom order, the doc slot
   * first; on one block a change, then a reply, a blank, a not-handled row, and last in the doc
   * slot the lost-notes row. Each stop is `{kind, id, sp, unseen}`, `id` as its seen op names it.
   */
  BR.pure.walk = function (s) {
    var out = [];
    var lost = {};
    (s.lost || []).forEach(function (n) { lost[n] = 1; });
    var removed = {};
    (s.deletions || []).forEach(function (d) { removed[d.change] = d; });
    function add(kind, id, sp, rank, order, unseen) {
      out.push({ kind: kind, id: id, sp: sp, unseen: unseen, rank: rank, line: lineOf(sp), order: order });
    }
    var back = {};
    (s.changes || []).forEach(function (c) { if (c.range && c.range.sp && back[c.op] == null) back[c.op] = c.range.sp; });
    (s.changes || []).forEach(function (c) {
      if (c.attr === 'human') return;
      // Your revert settles a change, yet its row stays a stop, at the block it put back, until the round ends
      if (c.undone_by != null && !covered(s, c.undone_by)) {
        add('change', c.id, (c.range && c.range.sp) || back[c.undone_by] || null, 0, c.op * 1e4 + c.idx, false);
        return;
      }
      if (c.settled) return;
      var d = removed[c.id];
      add('change', c.id, (c.range && c.range.sp) || (d && d.after_sp) || null, 0, c.op * 1e4 + c.idx, !c.seen);
    });
    (s.items || []).forEach(function (i) {
      var sp = !lost[i.n] && i.at && i.at.kind !== 'doc' && i.at.loc ? i.at.loc.sp : null;
      (i.replies || []).forEach(function (r) {
        if (r.by && r.by.type === 'agent' && (!r.seen || !covered(s, r.n))) add('reply', r.id, sp, 1, r.n, !r.seen);
      });
      if (i.by && i.by.type === 'agent' && i.verdict === 'ask') {
        var mine = (i.replies || []).filter(function (r) { return r.by && r.by.type === 'human'; });
        var last = mine[mine.length - 1];
        if (i.status === 'open' || (i.status === 'answered' && last && !covered(s, last.n))) {
          add('blank', '#' + i.n, sp, 2, i.n, i.status === 'open');
        }
      }
      if (i.status === 'not_handled' && (!i.seen || !covered(s, i.turn))) add('not_handled', 'nh:' + i.n, sp, 3, i.n, !i.seen);
    });
    if ((s.lost || []).length) add('lost', 'lost', null, 4, 0, false);
    out.sort(function (a, b) { return a.line - b.line || a.rank - b.rank || a.order - b.order; });
    return out;
  };

  var VERDICT_OF = { yes: 'yes', no: 'no', later: 'later', your_call: 'your_call' };
  // What a focused note keeps from the page: each would write a verdict or a note
  var NOTE_KEYS = { yes: 1, no: 1, later: 1, your_call: 1, note: 1, ask: 1 };
  // A seen note in one of these folds to its dot
  var FOLDS = { handled: 1, answered: 1, later: 1 };
  // Open blanks past this many show only their question until focused
  var OPEN_BLANKS = 5;
  // How long a first x on an open change row waits for the second
  var CONFIRM_MS = 3000;
  var CANT_UNDO = "can't undo this one · c instead";
  var BLANK_HINT = 'type an answer · y yes · x no · . your call · l later';
  var TYPE_IT = '⏎ to type an answer';
  var ANSWERED = { yes: 'yes', no: 'no', later: 'later', your_call: 'your call' };
  // Keys a closed blank leaves to the page: moving, taking back, and switches that write nothing
  var BLANK_PASSES = { focus_next: 1, focus_prev: 1, next_item: 1, prev_item: 1, next_file: 1, prev_file: 1, undo: 1,
    hide_marks: 1, turn: 1, revert_change: 1 };
  // Shown once ever, the first time a note folds to its dot
  var DOT_HINT = '• a settled note · ⏎ opens it';
  var DOT_HINT_KEY = 'birta-dot-hint';
  var RESOLVE_MS = 120;
  var COMPOSE_MS = 300;
  var QUOTE_CHARS = 40;
  var UNSENT = 'unsent · ⏎ to keep';
  var MOVED = 'its block changed · c to re-point';

  // Group order under one anchor: notes with their replies, then what the page added, then facts
  var G_NOTE = 1, G_DRAFT = 2, G_UNSENT = 3, G_COMPOSER = 4, G_RECEIPT = 5, G_FACT = 6, G_BLANK = 7,
    G_TURN = 8, G_LOST = 9, G_REWRITE = 10, G_FOLDER = 11;

  var hosts = new Map();
  var state = null;
  var skew = 0;
  // Kept notes the snapshot doesn't hold yet, by cid
  var drafts = [];
  // Half-typed notes: {key, text, tie?, lost?}, `tie` the block they were typed for while this page has it
  var unsent = [];
  var comp = null;
  var seq = 0;
  // Live notes on the served folder, from the listing; drawn only when a folder is served
  var folderNotes = 0;
  // Reading mode: every note but the open composer steps aside
  var hiddenAll = false;
  // Folded notes opened in this page, by n
  var expanded = {};
  var dotList = [];
  var lostOpen = false;
  // The rewrite row stays, without its key, from its turn until the next hand-back
  var rewriteTurn = null;
  // Runs once the op with that cid is acked; dropped on a nack
  var afterAck = {};
  // Words an op carries that a nack must hand back: {key, text, tie} for notes and edits, {blank, text} for answers
  var pending = {};
  // A no on an open change row waiting for its second x: {id, until}
  var armed = null;
  // The cid of an undo the server hasn't answered, so a second x can't send another, and its change
  var undoing = null;
  var reverting = null;
  // The block the keys act on, and the change n landed on there: they decide which rows are open
  var focusSp = null;
  var landed = null;
  // Rows open now, in doc order, and the one whose keys act
  var openIds = [];
  var keyedRow = null;
  var dotHinted = false;

  function content() { return document.getElementById('content'); }
  function narrow() { return window.innerWidth < 768; }
  function writable() { return !state || state.log_writable !== false; }

  function blockOf(sp) { return BR.blocks.of(sp); }

  function capital(name) { return name.charAt(0).toUpperCase() + name.slice(1); }

  function who(by) {
    if (!by) return 'someone';
    if (by.type === 'human') return by.name || 'you';
    if (by.type === 'agent') return by.name ? capital(by.name) : 'an agent';
    return 'birta';
  }

  /** Who held the turn with claim line `turn`, from the state's rounds. */
  function turnAgent(turn) {
    var s = state;
    var round = ((s && s.rounds) || []).filter(function (r) { return r.claim === turn; })[0];
    if (round && round.claimed_by) return capital(round.claimed_by);
    var replied = s && s.phrase && s.phrase.replied;
    if (replied && replied.turn === turn && replied.by) return capital(replied.by);
    return 'an agent';
  }

  function hhmm(ts) {
    var d = new Date(ts);
    return String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0');
  }

  function plural(n, one, many) { return n + ' ' + (n === 1 ? one : many); }

  function clamp(text) {
    var chars = Array.from(text);
    return chars.length > QUOTE_CHARS ? chars.slice(0, QUOTE_CHARS).join('') + '…' : text;
  }

  /** A half-typed note's key: its target, its kind, the note its words edit, if any, and the rev its sp is from. */
  function composeKey(at, kind, to) {
    var k = { at: at, kind: kind };
    if (to != null) k.to = to;
    k.rev = birtaBus.rev();
    return JSON.stringify(k);
  }

  function keyTarget(key) {
    try {
      var k = JSON.parse(key);
      return k && k.at ? k : null;
    } catch (e) {
      return null;
    }
  }

  /** The block a request points at: its own sp, or for a change, the block it changed. */
  function spOfAt(at) {
    if (!at || at.k === 'doc') return null;
    if (at.k !== 'change') return at.sp || null;
    var c = changeOf(at.id);
    var d = ((state && state.deletions) || []).filter(function (x) { return x.change === at.id; })[0];
    return c && c.range && c.range.sp ? c.range.sp : d ? d.after_sp : null;
  }

  /** Where a note on `at` (a request) goes: under its block, or in the doc slot. */
  function slotOfAt(at) {
    var sp = spOfAt(at);
    return sp ? { sp: sp } : { doc: true };
  }

  /** Ties words to the block `el`, so they can find it again once a patch replaces it. */
  function tie(el) {
    return el ? { el: el, name: el.localName, text: el.textContent } : null;
  }

  /**
   * The tied block in the doc now: its element, else the one block a patch left with its tag and
   * its text, or null when none or several are.
   */
  function tied(t) {
    if (!t) return null;
    if (!t.el.isConnected) {
      var alike = BR.blocks.alike(t.name, t.text);
      if (alike.length !== 1) return null;
      t.el = alike[0];
    }
    return t.el;
  }

  /**
   * `at` as the doc on screen places it, for words tied by `t` (or, untied, from rev `rev`). A tied
   * block gives the sp as it is now. Otherwise a quote drops its sp so the server searches, and a
   * block anchor is null.
   */
  function aim(at, t, rev) {
    if (!at || !at.sp || (at.k !== 'block' && at.k !== 'quote')) return at;
    var out = {};
    for (var k in at) out[k] = at[k];
    var el = tied(t);
    if (t ? el : rev === birtaBus.rev()) {
      if (el) out.sp = el.getAttribute('data-sourcepos');
      return out;
    }
    if (at.k === 'block') return null;
    delete out.sp;
    delete out.off;
    delete out.tl;
    return out;
  }

  /** Whether a half-typed note's block is known: it is tied and found, or untied and from this rev. */
  function fresh(u, t) {
    if (u.lost) return false;
    return u.tie ? !!tied(u.tie) : t.rev === birtaBus.rev();
  }

  /** A half-typed note on a block the page can no longer find, until c re-points it. */
  function stranded(u) {
    var t = keyTarget(u.key);
    return !!t && t.at.k === 'block' && !!t.at.sp && t.to == null && !fresh(u, t);
  }

  /** Where an item of the state goes: under the block it is on now, at the change that took its words, or in the doc slot. */
  function slotOfItem(i) {
    if (i.at && i.at.kind !== 'doc' && i.at.loc) return { sp: i.at.loc.sp };
    var c = i.at && i.at.change ? changeOf(i.at.change) : null;
    return c ? slotOfChange(c) : { doc: true };
  }

  function deletionOf(c) {
    return ((state && state.deletions) || []).filter(function (d) { return d.change === c.id && d.kind === 'block'; })[0] || null;
  }

  /** A removal of whole blocks: its row sits at its tick, and only n opens it. */
  function tombstone(c) {
    return !(c.range && c.range.sp) && c.undone_by == null && !!deletionOf(c);
  }

  /** The block a change's row goes under: its own, the one your revert put back, or the one above its tick. */
  function rowSp(c) {
    if (c.range && c.range.sp) return c.range.sp;
    if (c.undone_by != null) {
      var back = ((state && state.changes) || []).filter(function (x) { return x.op === c.undone_by && x.range && x.range.sp; })[0];
      if (back) return back.range.sp;
    }
    var d = deletionOf(c) || ((state && state.deletions) || []).filter(function (x) { return x.change === c.id; })[0];
    return d ? d.after_sp || null : null;
  }

  function slotOfChange(c) {
    var sp = rowSp(c);
    return sp ? { sp: sp } : { doc: true };
  }

  /** A row that shows no keys: a settled one, an earlier round's, or one your revert took back. */
  function muted(c) {
    var latest = latestTurn();
    return !!c.settled || c.undone_by != null || (c.turn != null && latest != null && c.turn < latest);
  }

  // --- hosts ---

  function makeHost(id) {
    var made = BR.shadowHost('birta-flow');
    var host = made.host;
    host.toggleAttribute('data-br-hidden', hiddenAll);
    var box = document.createElement('div');
    box.className = 'flow';
    made.root.appendChild(box);
    var h = { id: id, host: host, box: box, sig: null, rule: null, entry: null };
    box.addEventListener('keydown', function (ev) {
      // A change row's keys reach the page, which asks openRow() whether the row is open
      if (h.entry && h.entry.kind === 'lost') lostKey(ev);
      else if (h.blank && h.entry && h.entry.kind === 'blank') blankKey(ev, h);
      else if (h.entry && h.entry.change) return;
      else if (h.entry && h.entry.nh != null) notHandledKey(ev, h.entry.nh);
      else if (h.entry && h.entry.focusable) noteKey(ev, h);
    });
    // A quiet blank shows its slot while it has focus
    box.addEventListener('focusin', function () { if (h.blank) h.blank.hint.hidden = false; });
    box.addEventListener('focusout', function () {
      if (h.blank && h.blank.quiet && !h.blank.slot.querySelector('textarea')) h.blank.hint.hidden = true;
    });
    box.addEventListener('click', function () {
      if (h.blank && h.entry && h.entry.kind === 'blank') {
        if (writable() && !h.blank.slot.querySelector('textarea')) openAnswer(h);
      } else if (h.entry && h.entry.kind === 'rewrite' && state && state.rewrite) markRewriteSeen();
      else if (h.entry && h.entry.seen && writable()) BR.op({ op: 'seen', ids: [h.entry.seen] });
    });
    hosts.set(id, h);
    return h;
  }

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  // Each key is its own element, so a bidi override in a quoted body can't reorder them
  function keysRow(parent, keys) {
    parent.append(' · ');
    var group = el('span', 'keys');
    keys.forEach(function (k, i) {
      if (i) group.append(' · ');
      var key = el('span', 'key');
      var at = k.indexOf(' ');
      key.append(el('kbd', null, k.slice(0, at)), k.slice(at));
      group.appendChild(key);
    });
    parent.appendChild(group);
  }

  function isolated(text) { return el('bdi', null, text); }

  /** Draws `e` into its host unless nothing it shows has changed. */
  function render(h, e) {
    h.host.setAttribute('data-kind', e.kind);
    h.rule = e.rule || null;
    h.entry = e;
    var sig = JSON.stringify([e.kind, e.lead, e.pre, e.text, e.struck, e.tail, e.prefix, e.meta, e.keys, e.fold, e.quiet,
      e.muted, e.lost, e.kind === 'lost' && lostOpen, e.answer && e.answer.n]);
    if (h.sig === sig) return;
    h.sig = sig;
    h.box.textContent = '';
    h.blank = null;
    if (e.kind === 'blank') return renderBlank(h, e);
    if (e.kind === 'lost') return renderLost(h, e);
    if (e.kind === 'folder') return renderFolder(h, e);
    if (e.prefix) h.box.appendChild(el('div', 'muted', e.prefix));
    var line = el('div', (e.fold ? 'text fold' : 'text') + (e.muted ? ' muted' : ''));
    if (e.lead) line.appendChild(el('span', 'lead', e.lead));
    if (e.pre) line.append(e.pre);
    if (e.text) line.appendChild(isolated(e.text));
    if (e.struck) line.appendChild(el('bdi', 'struck', e.struck));
    if (e.tail) line.append(e.tail);
    if (e.keys) keysRow(line, e.keys);
    h.box.appendChild(line);
    if (e.fold) {
      var more = el('div', 'muted more', '▸ more');
      more.hidden = true;
      more.addEventListener('click', function () {
        line.classList.remove('fold');
        more.remove();
      });
      h.box.appendChild(more);
      h.fold = { line: line, more: more };
    } else {
      h.fold = null;
    }
    if (e.meta) h.box.appendChild(el('div', 'muted', e.meta));
    // Reached by n, p and a click, never by Tab, which walks the focused block's links
    if (e.focusable) h.box.tabIndex = -1;
    else h.box.removeAttribute('tabindex');
  }

  // The question reads like a note; under it the dashed slot takes the answer
  function renderBlank(h, e) {
    h.box.tabIndex = 0;
    var line = h.box.appendChild(el('div', 'text'));
    if (e.lead) line.appendChild(el('span', 'lead', e.lead));
    line.appendChild(isolated(e.text));
    var slot = el('div', 'blank');
    var hint = el('div', 'muted', BLANK_HINT);
    hint.hidden = !!e.quiet;
    slot.appendChild(hint);
    // Once answered, the answer shows under it and the empty slot goes
    slot.hidden = !!e.answer;
    h.box.appendChild(slot);
    h.blank = { n: e.blank, slot: slot, hint: hint, quiet: !!e.quiet, to: e.to };
  }

  function renderLost(h, e) {
    // A walk stop, never a Tab stop: ⏎ on it opens or folds the list
    h.box.tabIndex = -1;
    var row = el('div', 'muted', plural(e.lost.length, 'note lost its place ', 'notes lost their place '));
    var open = el('span', 'more', '▸');
    open.addEventListener('click', toggleLost);
    row.appendChild(open);
    h.box.appendChild(row);
    if (!lostOpen) return;
    e.lost.forEach(function (i) {
      var item = el('div', 'text');
      item.appendChild(el('div', 'muted')).appendChild(isolated(wasOn(i)));
      item.appendChild(el('span', 'lead', who(i.by) + ' · '));
      item.appendChild(isolated(i.body || i.verdict || ''));
      h.box.appendChild(item);
    });
  }

  function toggleLost() {
    lostOpen = !lostOpen;
    reconcile();
  }

  /** Where a lost note was: its section and words, or a section note's own heading. */
  function wasOn(i) {
    var at = i.at || {};
    var quote = at.shown || at.quote || '';
    if (at.scope === 'section') return 'was on: ' + quote;
    return 'was on: ' + (at.section ? at.section + ' · ' : '') + '"' + clamp(quote) + '"';
  }

  function renderFolder(h, e) {
    var row = el('div', 'muted', e.text + ' ');
    var open = el('span', 'more', '▸');
    open.addEventListener('click', function (ev) {
      ev.stopPropagation();
      BR.chrome.openMenu('folder');
    });
    row.appendChild(open);
    h.box.appendChild(row);
  }

  // --- the model ---

  function changeOf(id) {
    return ((state && state.changes) || []).filter(function (c) { return c.id === id; })[0] || null;
  }

  function linesOf(c) {
    var w = (c && c.what) || {};
    return plural(w.old_lines || 0, 'line', 'lines') + ' → ' + (w.new_lines || 0);
  }

  /** What kind of change it was and how big, as the server words it. */
  function labelOf(c) {
    return (c && c.label) || linesOf(c);
  }

  function canUndo(c) {
    return !!c.reversible && c.reversible.ok && !(state && state.writable === false);
  }

  /** A row's key chips, named as the page binds them. */
  function rowKeys(revertible) {
    var keys = [BR.keyName('yes') + ' keep'];
    if (revertible) keys.push(BR.keyName('revert_change') + ' revert');
    return keys.concat([BR.keyName('note') + ' note']);
  }

  /** `e` as a line that answers for change `c`: it takes the keys while it is the open row. */
  function receiptOf(c, e) {
    if (!c) return e;
    e.change = c;
    if (c.id === keyedRow) e.keys = rowKeys(canUndo(c));
    e.focusable = true;
    e.seen = c.seen ? null : c.id;
    return e;
  }

  /** What a bare verdict was on, from its anchor, as the block may be gone: a section's heading, else its first words. */
  function verdictOn(i) {
    var at = i.at || {};
    if (at.kind === 'doc') return 'the whole doc';
    var quote = at.shown || at.quote || '';
    if (at.scope === 'section') return quote.split('\n')[0].trim();
    return '"' + BR.pure.fewWords(quote) + '"';
  }

  /** Your answer among the reply lines a `covers` cited the change for, or null. */
  function answerCited(s, ns) {
    var hit = null;
    (s.items || []).forEach(function (i) {
      (i.replies || []).forEach(function (r) {
        if (!hit && r.by && r.by.type === 'human' && ns.indexOf(r.n) >= 0) hit = r;
      });
    });
    return hit;
  }

  /** The row for change `c`, as it opens under its block or at its tick. */
  function rowOf(c, s) {
    var e = { id: 'c' + c.id, kind: 'receipt', muted: muted(c), rule: ['dotted', 'agent'] };
    if (c.undone_by != null) {
      e.pre = 'reverted · ' + labelOf(c);
      return receiptOf(c, e);
    }
    if (tombstone(c)) {
      var d = deletionOf(c);
      e.lead = who(c.by) + ' · ';
      e.pre = 'removed: ';
      e.struck = d.first_line || '';
      e.tail = d.label ? ' (' + d.label + ')' : '';
      return receiptOf(c, e);
    }
    if (c.attr === 'outside') {
      e.kind = 'fact';
      e.id = 'f' + c.id;
      e.pre = 'changed outside birta' + (c.cause === 'git' ? ' (git)' : '') + ' · ' + labelOf(c);
      return receiptOf(c, e);
    }
    e.lead = who(c.by) + ' · ';
    if (c.asked && (c.asked.kind === 'cited' || c.asked.kind === 'location')) {
      var note = (s.items || []).filter(function (i) { return c.asked.notes.indexOf(i.n) >= 0; })[0];
      e.rule = ['solid', 'agent'];
      var answer = note ? null : answerCited(s, c.asked.answers || []);
      if (answer) {
        e.pre = 'because you answered "';
        e.text = answer.body || ANSWERED[answer.verdict] || '';
        e.tail = '" · ' + labelOf(c);
        return receiptOf(c, e);
      }
      if (note && !note.body && note.verdict) {
        e.pre = 'because you said ' + (ANSWERED[note.verdict] || note.verdict) + ' to ';
        e.text = verdictOn(note);
        e.tail = ' · ' + labelOf(c);
        return receiptOf(c, e);
      }
      e.pre = 'because you said "';
      e.text = (note && note.body) || '';
      e.tail = '" · ' + labelOf(c);
    } else {
      e.pre = 'without a note · ' + labelOf(c);
    }
    return receiptOf(c, e);
  }

  /** Works out which rows are open: the change n landed on, else every row under the focused block. */
  function openRows(s) {
    var ids = [];
    if (landed && changeOf(landed)) ids = [landed];
    else if (focusSp) {
      (s.changes || []).forEach(function (c) {
        if (c.attr !== 'human' && !tombstone(c) && rowSp(c) === focusSp) ids.push(c.id);
      });
    }
    openIds = ids;
    keyedRow = ids.filter(function (id) { return !muted(changeOf(id)); })[0] || null;
  }

  /** A settled note folds once seen; from an earlier round it folds unseen too, unless a reply is news. */
  function folded(i, slot) {
    if (!settled(i) || !slot.sp || expanded[i.n] || !blockOf(slot.sp)) return false;
    if (i.seen) return true;
    var latest = latestTurn();
    var earlier = i.turn != null && latest != null && i.turn < latest;
    return earlier && !(i.replies || []).some(function (r) { return r.by && r.by.type === 'agent' && !r.seen; });
  }

  /** A dot ⏎ opened in the clean view: the note and its replies to read, with no keys of their own. */
  function readOnly(i, slot, add) {
    var agent = i.by && i.by.type === 'agent';
    var quote = i.at && i.at.scope === 'quote' ? i.at.shown || i.at.quote : null;
    add({ id: 'n' + i.n, kind: 'note', lead: who(i.by) + ' · ', text: i.body || ANSWERED[i.verdict] || '',
      prefix: quote ? '→ "' + clamp(quote) + '"' : null, rule: [agent ? 'dotted' : 'solid', agent ? 'agent' : 'you'] }, slot, [G_NOTE, i.n, 0]);
    (i.replies || []).forEach(function (r, k) {
      add({ id: 'r' + r.n, kind: 'reply', lead: '↳ ' + who(r.by) + ' · ', text: r.body || ANSWERED[r.verdict] || '', fold: true,
        rule: ['solid', r.by && r.by.type === 'human' ? 'you' : 'agent'] }, slot, [G_NOTE, i.n, 1 + k]);
    });
  }

  function dotTitle(i) {
    var owner = i.by && i.by.type === 'agent' ? who(i.by) + "'s note" : 'your note';
    return owner + ' · settled · ⏎ opens it';
  }

  function lastHumanReply(i) {
    var mine = (i.replies || []).filter(function (r) { return r.by && r.by.type === 'human'; });
    return mine[mine.length - 1] || null;
  }

  // An answer counts once a hand-back covers it; until then the blank takes a new one
  function answerSent(i) {
    var r = lastHumanReply(i);
    return !!r && ((state && state.rounds) || []).some(function (x) { return x.submit > r.n; });
  }

  /** An agent's question still taking answers: open, or answered by a draft. */
  function asking(i) {
    return !!i.by && i.by.type === 'agent' && i.verdict === 'ask' &&
      (i.status === 'open' || (i.status === 'answered' && !answerSent(i)));
  }

  function settled(i) {
    return !!FOLDS[i.status] && !asking(i);
  }

  /** Every flow element the state and the page's own work call for, with where each goes. */
  function entries() {
    var out = [];
    var s = state;
    var cids = {};
    var lost = {};
    // Changes a done reply already carries the keys for
    var keyed = {};
    // The first whole-doc note of yours, which the turn note answers under
    var docNote = null;
    dotList = [];
    function add(e, slot, order) {
      e.slot = slot;
      e.order = order;
      out.push(e);
    }
    // Approved: every note on a block is its dot, and nothing else of the state is drawn
    var clean = !!s && BR.pure.clean(s, BR.chrome.now());
    if (clean) {
      openIds = [];
      keyedRow = null;
      (s.lost || []).forEach(function (n) { lost[n] = 1; });
      (s.items || []).forEach(function (i) {
        if (i.cid) cids[i.cid] = 1;
        var slot = slotOfItem(i);
        if (lost[i.n] || i.status === 'retracted' || !slot.sp || !blockOf(slot.sp)) return;
        if (expanded[i.n]) readOnly(i, slot, add);
        else dotList.push({ sp: slot.sp, role: i.by && i.by.type === 'agent' ? 'agent' : 'you', title: dotTitle(i) });
      });
    } else if (s) {
      openRows(s);
      (s.lost || []).forEach(function (n) { lost[n] = 1; });
      (s.items || []).forEach(function (i) {
        if (i.cid) cids[i.cid] = 1;
        if (lost[i.n] || i.status === 'retracted') return;
        var slot = slotOfItem(i);
        var agent = i.by && i.by.type === 'agent';
        var whole = !!i.at && i.at.kind === 'doc';
        if (whole && !agent && docNote == null) docNote = i.n;
        // Its done reply carries the change's keys, folded or not; a doc note's spans many
        var named = !whole && i.outcome && i.outcome.done && !i.outcome.no_change && i.outcome.changes.length ? i.outcome.changes[0] : null;
        // A folded note draws no done reply, so its change keeps its own row
        if (folded(i, slot)) {
          dotList.push({ sp: slot.sp, role: agent ? 'agent' : 'you', title: dotTitle(i) });
          return;
        }
        var bare = !i.body && !agent && !(i.replies || []).length;
        if (named) keyed[named] = 1;
        if (asking(i)) {
          // The latest answer wins, so only it shows
          var answer = lastHumanReply(i);
          add({ id: 'b' + i.n, kind: 'blank', lead: who(i.by) + ' · ', text: i.body || '', blank: i.n, open: i.status === 'open',
            answer: answer, to: who(i.by), rule: ['dotted', 'agent'] }, slot, [G_BLANK, i.n, 0]);
          if (answer) {
            add({ id: 'r' + answer.n, kind: 'reply', lead: '↳ ' + who(answer.by) + ' · ', text: answer.body || answer.verdict || '',
              fold: true, rule: ['solid', 'you'] }, slot, [G_BLANK, i.n, 1]);
          }
          return;
        }
        // A bare verdict is its glyph; only a row left not handled, or the agent's done, shows under it
        var done = !!(i.outcome && i.outcome.done);
        if (bare && i.status !== 'not_handled' && !done) return;
        var group = agent ? G_BLANK : G_NOTE;
        var quote = i.at && i.at.scope === 'quote' ? i.at.shown || i.at.quote : null;
        if (!bare) {
          add({ id: 'n' + i.n, kind: 'note', lead: who(i.by) + ' · ', text: i.body || '', prefix: quote ? '→ "' + clamp(quote) + '"' : null,
            item: i, focusable: true,
            rule: agent ? ['dotted', 'agent'] : [i.status === 'draft' ? 'dashed' : 'solid', 'you'] }, slot, [group, i.n, 0]);
        }
        (i.replies || []).forEach(function (r, k) {
          var human = r.by && r.by.type === 'human';
          add({ id: 'r' + r.n, kind: 'reply', lead: '↳ ' + who(r.by) + ' · ', pre: r.kind === 'didnt' ? "didn't · " : null,
            text: r.body || r.verdict || '', fold: true,
            seen: !human && !r.seen ? r.id : null, rule: ['solid', human ? 'you' : 'agent'] }, slot, [group, i.n, 1 + k]);
        });
        var name = turnAgent(i.turn);
        if (done) {
          var changes = i.outcome.no_change ? [] : i.outcome.changes;
          var e = { id: 'o' + i.n, kind: 'reply', lead: '↳ ' + name + ' · ', rule: ['solid', 'agent'] };
          var first = changes.length ? changeOf(changes[0]) : null;
          if (bare && first && tombstone(first)) {
            // Nothing above it says what went, so its one row is the removal itself
            e = rowOf(first, s);
            e.id = 'o' + i.n;
          } else if (whole && changes.length) {
            e.pre = 'done · ' + plural(changes.length, 'change', 'changes') + ' · n to walk';
          } else {
            var change = changes.length ? changeOf(changes[0]) : null;
            e.pre = 'done · ' + (change ? labelOf(change) : 'no change in text');
            receiptOf(change, e);
          }
          add(e, slot, [group, i.n, 1e6]);
        }
        if (i.status === 'not_handled') {
          var replied = s.phrase && s.phrase.replied;
          var when = replied && replied.turn === i.turn && replied.ts ? hhmm(replied.ts) : 'earlier';
          add({ id: 'h' + i.n, kind: 'reply', text: '↳ not handled in ' + name + "'s " + when + ' turn',
            nh: i.n, focusable: true, rule: ['solid', 'agent'] }, slot, [group, i.n, 1e6 + 1]);
        }
      });
      (s.changes || []).forEach(function (c) {
        if (openIds.indexOf(c.id) < 0 || keyed[c.id] || c.attr === 'human') return;
        add(rowOf(c, s), slotOfChange(c), [c.attr === 'outside' ? G_FACT : G_RECEIPT, c.op, c.idx]);
      });
      var replied = s.phrase && s.phrase.replied;
      if (replied && replied.note) {
        add({ id: 'turn', kind: 'turn_note', lead: '↳ ' + (replied.by ? capital(replied.by) : 'an agent') + ' · ', text: replied.note,
          rule: ['dotted', 'agent'] }, { doc: true }, docNote != null ? [G_NOTE, docNote, 1e6 + 2] : [0, 0, 0]);
      }
      var gone = (s.items || []).filter(function (i) { return lost[i.n]; });
      if (gone.length) add({ id: 'lost', kind: 'lost', lost: gone }, { doc: true }, [G_LOST, 0, 0]);
      var row = rewriteRow(s);
      if (row) add(row, { doc: true }, [G_REWRITE, 0, 0]);
    }
    if (!clean && folderNotes > 0 && BOOT.review.dir) {
      add({ id: 'folder', kind: 'folder', text: plural(folderNotes, 'note on this folder', 'notes on this folder') },
        { doc: true }, [G_FOLDER, 0, 0]);
    }
    drafts.forEach(function (d) {
      if (cids[d.cid]) return;
      add({ id: 'd' + d.cid, kind: 'note', lead: 'you · ', text: d.body, rule: ['dashed', 'you'] }, slotOfAt(d.at), [G_DRAFT, d.seq, 0]);
    });
    unsent.forEach(function (u, k) {
      if (comp && comp.key === u.key) return;
      var t = keyTarget(u.key);
      if (!t) return;
      // An sp from another rev may name another block now
      var slot = !t.at.sp ? slotOfAt(t.at) : !fresh(u, t) ? { doc: true } : u.tie ? { el: u.tie.el } : slotOfAt(t.at);
      add({ id: 'u' + u.key, kind: 'note', lead: 'you · ', text: u.text, meta: stranded(u) ? MOVED : UNSENT, unsent: u, focusable: true,
        rule: ['dashed', 'you'] }, slot, [G_UNSENT, k, 0]);
    });
    // Open blanks past the fifth, in doc order, show only their question until focused
    var open = out.filter(function (e) { return e.kind === 'blank' && e.open; });
    open.sort(function (a, b) { return docOrder(a.slot, b.slot); });
    open.forEach(function (e, k) { e.quiet = k >= OPEN_BLANKS; });
    return out;
  }

  function docOrder(a, b) {
    var x = a.sp ? blockOf(a.sp) : null;
    var y = b.sp ? blockOf(b.sp) : null;
    if (x === y) return 0;
    if (!x) return -1;
    if (!y) return 1;
    return x.compareDocumentPosition(y) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1;
  }

  function rewriteRow(s) {
    if (s.rewrite) rewriteTurn = s.rewrite.turn;
    if (rewriteTurn == null) return null;
    var turn = rewriteTurn;
    // A hand-back after the rewrite ends it
    if ((s.rounds || []).some(function (r) { return r.submit > turn; })) {
      rewriteTurn = null;
      return null;
    }
    var mine = (s.changes || []).filter(function (c) { return c.turn === turn; });
    if (!s.rewrite && mine.some(function (c) { return !c.seen; })) return null;
    var name = mine.length && mine[0].by ? who(mine[0].by) : turnAgent(turn);
    return { id: 'rewrite', kind: 'rewrite', text: name + ' rewrote most of it' + (s.rewrite ? ' · ' + BR.keyName('yes') + ' mark seen' : '') };
  }

  function markRewriteSeen() {
    var turn = state.rewrite.turn;
    BR.op({
      op: 'seen',
      ids: state.changes.filter(function (c) { return c.turn === turn; }).map(function (c) { return c.id; })
    });
  }

  // --- placement ---

  /** Where a host for `slot` goes: after its block, last in its item, or above the doc. */
  function place(slot) {
    var C = content();
    var el = slot.el && slot.el.isConnected ? slot.el : slot.sp ? blockOf(slot.sp) : null;
    if (!el) return { parent: C.parentNode, before: C };
    // A closed details would hide it
    var closed = el.parentElement && el.parentElement.closest('details:not([open])');
    if (closed && C.contains(closed)) return { parent: closed.parentNode, after: closed };
    if (el.localName === 'p' && el.parentElement && el.parentElement.localName === 'li') el = el.parentElement;
    if (el.localName === 'li') return { parent: el, last: true };
    // Anything between rows is squashed into a cell
    if (/^(tr|td|th)$/.test(el.localName)) el = el.closest('table');
    return { parent: el.parentNode, after: el };
  }

  function slotKey(p) {
    return p.last ? p.parent : p.after || p.before;
  }

  function byOrder(a, b) {
    for (var i = 0; i < 3; i++) if (a.order[i] !== b.order[i]) return a.order[i] - b.order[i];
    return 0;
  }

  // Moves only what is out of place, so a host keeps focus, selection and IME state
  function arrange(p, nodes) {
    if (p.last) {
      var kids = Array.prototype.slice.call(p.parent.children);
      var tail = kids.slice(kids.length - nodes.length);
      if (tail.length === nodes.length && tail.every(function (n, i) { return n === nodes[i]; })) return;
      nodes.forEach(function (n) { p.parent.appendChild(n); });
    } else if (p.before) {
      var next = p.before;
      for (var i = nodes.length - 1; i >= 0; i--) {
        if (nodes[i].nextSibling !== next) p.parent.insertBefore(nodes[i], next);
        next = nodes[i];
      }
    } else {
      var ref = p.after;
      nodes.forEach(function (n) {
        if (ref.nextSibling !== n) p.parent.insertBefore(n, ref.nextSibling);
        ref = n;
      });
    }
  }

  function reconcile() {
    if (!content()) return;
    var list = entries();
    var want = {};
    var groups = new Map();
    function join(p, h, order) {
      var k = slotKey(p);
      if (!groups.has(k)) groups.set(k, { p: p, items: [] });
      groups.get(k).items.push({ h: h, order: order });
      want[h.id] = 1;
    }
    list.forEach(function (e) {
      var h = hosts.get(e.id) || makeHost(e.id);
      render(h, e);
      join(place(e.slot), h, e.order);
    });
    if (comp) join(place(composerSlot()), comp.h, [G_COMPOSER, 0, 0]);
    var apply = function () {
      hosts.forEach(function (h, id) {
        if (want[id]) return;
        h.host.remove();
        hosts.delete(id);
      });
      groups.forEach(function (g) {
        g.items.sort(byOrder);
        arrange(g.p, g.items.map(function (x) { return x.h.host; }));
      });
      var pad = narrow() ? 4 : 12;
      hosts.forEach(function (h) {
        // An item's note keeps the item's indent, 12px in at every width
        var inItem = h.host.parentElement && h.host.parentElement.localName === 'li';
        h.box.style.paddingLeft = (inItem ? 12 : pad) + 'px';
        h.host.toggleAttribute('data-br-hidden', hiddenAll && !typing(h));
        h.host.toggleAttribute('data-doc-end', h.host.nextElementSibling === content());
      });
    };
    if (typeof birtaPatch !== 'undefined') birtaPatch.withScrollAnchor(apply);
    else apply();
    hosts.forEach(function (h) {
      if (h.fold) h.fold.more.hidden = h.fold.line.scrollHeight <= h.fold.line.clientHeight + 1;
    });
    if (comp) refocus();
    if (dotList.length) hintDots();
    if (BR.lane && BR.lane.redraw) BR.lane.redraw();
  }

  /** The first dot ever says what it is, once; storage that refuses keeps it to this page. */
  function hintDots() {
    if (dotHinted) return;
    dotHinted = true;
    try {
      if (localStorage.getItem(DOT_HINT_KEY)) return;
      localStorage.setItem(DOT_HINT_KEY, '1');
    } catch (e) {}
    // A state can be drawn before the chrome that shows pills is up
    setTimeout(function () { BR.chrome.pill(DOT_HINT); }, 0);
  }

  // --- the composer ---

  /** Where the composer is drawn: its block while it lives, else the new one at its line. */
  function composerSlot() {
    if (!comp.anchor) return { doc: true };
    if (comp.anchor.isConnected) return { el: comp.anchor };
    var found = tied(comp.tie);
    if (found) {
      comp.anchor = found;
      return { el: found };
    }
    var line = comp.sp.split(':')[0] + ':';
    var found = Array.prototype.filter.call(content().querySelectorAll('[data-sourcepos]'), function (e) {
      return e.localName === comp.anchor.localName && e.getAttribute('data-sourcepos').indexOf(line) === 0;
    });
    if (found.length) comp.anchor = found[found.length - 1];
    return comp.anchor.isConnected ? { el: comp.anchor } : { doc: true };
  }

  function selectionRange() {
    var sel = getSelection();
    if (!sel || !sel.rangeCount || sel.isCollapsed) return null;
    var r = sel.getRangeAt(0);
    return content().contains(r.commonAncestorContainer) ? r.cloneRange() : null;
  }

  /** Keeps the words a note is on tinted while its composer is open; null clears it. */
  function tint(range) {
    if (typeof CSS === 'undefined' || !CSS.highlights || typeof Highlight === 'undefined') return;
    if (range) CSS.highlights.set('birta-pending', new Highlight(range));
    else CSS.highlights.delete('birta-pending');
  }

  var KIND = { p: 'paragraph', li: 'list item', tr: 'table row', td: 'table row', th: 'table row', pre: 'code block' };

  /** Words of `el` that are its own: a list item's nested list and review's hosts left out. */
  function ownText(el) {
    var out = '';
    el.childNodes.forEach(function (n) {
      if (n.nodeType === 3) out += n.data;
      else if (n.nodeType === 1 && !BR.isHost(n) && !/^(ul|ol)$/.test(n.localName)) out += n.textContent;
    });
    return out;
  }

  /** A block in words: a heading as its section, else its first words, with what it is when `kind`. */
  function blockLabel(block, kind) {
    var level = /^h([1-6])$/.exec(block.localName);
    if (level) return new Array(Number(level[1]) + 1).join('#') + ' ' + block.textContent.replace(/\s+/g, ' ').trim() + ' (section)';
    return '"' + BR.pure.fewWords(ownText(block)) + '"' + (kind ? ' (' + (KIND[block.localName] || 'block') + ')' : '');
  }

  /** The nearest heading at or above `el`, in doc order, or null. */
  function headingOver(el) {
    var found = null;
    content().querySelectorAll('h1,h2,h3,h4,h5,h6').forEach(function (h) {
      if (h === el || h.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) found = h;
    });
    return found;
  }

  /** The composer's label line: what its note will be on. */
  function targetLabel(at, own) {
    if (own && own.body) return 'editing your note';
    return 'on: ' + label(at);
  }

  /** What a request points at, in words. */
  function label(at) {
    if (at.k === 'doc') return 'whole doc';
    if (at.k === 'dir') return 'this folder';
    if (at.k === 'quote') return '"' + clamp(at.q) + '"';
    var sp = spOfAt(at);
    var block = sp ? blockOf(sp) : null;
    if (at.k === 'change') {
      var h = block && headingOver(block);
      var name = h ? h.textContent.trim() : block ? BR.pure.fewWords(block.textContent) : '';
      return name ? "Claude's change to \"" + name + '"' : "Claude's change";
    }
    return block ? blockLabel(block) : 'this block';
  }

  // A patch that replaced the composer's block moved it, which drops focus
  function refocus() {
    var area = comp.area;
    if (!comp.active || document.activeElement === comp.h.host) return;
    var a = area.selectionStart;
    var b = area.selectionEnd;
    area.focus({ preventScroll: true });
    area.setSelectionRange(a, b);
  }

  function sendCompose(text) {
    birtaBus.send({ type: 'compose', path: CURRENT_PATH, key: comp.key, text: text });
  }

  /**
   * Opens a composer on `at`. `kind` is `note` or `ask`; `own` is the draft verdict on that
   * target, whose words the note then edits; `label`, when given, heads it.
   */
  function open(kind, at, own, label) {
    if (comp) keep(false, true);
    // Read before the composer takes focus, which drops the selection
    var picked = at.k === 'quote' ? selectionRange() : null;
    var key = composeKey(at, kind, own ? own.n : null);
    var restored = unsent.filter(function (u) { return u.key === key; })[0];
    // The words a block change left behind go where the reader points next
    var moved = !restored && !(own && own.body) && at.k !== 'dir' ? unsent.filter(stranded).pop() : null;
    if (moved) {
      kind = keyTarget(moved.key).kind;
      key = composeKey(at, kind, own ? own.n : null);
    }
    var h = makeHost('composer');
    h.host.setAttribute('data-kind', 'composer');
    h.box.classList.add('composer');
    var sp = spOfAt(at);
    var anchor = sp ? blockOf(sp) : null;
    // `tie` is the block the note is for; `anchor` only places the composer
    comp = { h: h, kind: kind, at: at, key: key, own: own, anchor: anchor, sp: anchor ? sp : null,
      tie: at.sp && (at.k === 'block' || at.k === 'quote') ? tie(anchor) : null, rev: birtaBus.rev(),
      candidates: null, pick: 0, asked: null, rcid: null, rrev: null, timer: null, qtimer: null, sent: false, active: true };
    var target = el('div', 'muted target');
    var area = document.createElement('textarea');
    area.setAttribute('dir', 'auto');
    area.placeholder = kind === 'ask' ? 'question for Claude…' : 'note…';
    if (kind === 'ask') area.rows = 1;
    var mac = /Mac/.test(navigator.platform);
    var hint = '⏎ save · ⇧⏎ new line · ' + (mac ? '⌘⏎' : 'Ctrl⏎') + ' save & hand back · esc save & close';
    var named = el('div', 'muted label');
    named.appendChild(isolated(label || targetLabel(at, own)));
    h.box.append(named, target, area, el('div', 'muted hint', hint));
    tint(picked);
    comp.area = area;
    comp.target = target;
    if (restored) area.value = restored.text;
    else if (moved) area.value = moved.text;
    else if (own && own.body) area.value = own.body;
    if (moved) {
      unsent = unsent.filter(function (u) { return u !== moved; });
      birtaBus.send({ type: 'compose', path: CURRENT_PATH, key: moved.key, text: '' });
      sendCompose(moved.text);
      comp.sent = true;
    }
    area.addEventListener('keydown', composerKey);
    area.addEventListener('input', composerInput);
    area.addEventListener('focus', function () { if (comp && comp.area === area) comp.active = true; });
    area.addEventListener('blur', function () {
      // Removal by a patch blurs too; only a blur the composer survives in place counts
      setTimeout(function () {
        if (comp && comp.area === area && h.host.isConnected && document.activeElement !== h.host) comp.active = false;
      }, 0);
    });
    reconcile();
    area.focus({ preventScroll: true });
    area.setSelectionRange(area.value.length, area.value.length);
    BR.chrome.place();
  }

  function composerInput() {
    clearTimeout(comp.timer);
    comp.timer = setTimeout(function () {
      if (!comp) return;
      sendCompose(comp.area.value);
      comp.sent = true;
    }, COMPOSE_MS);
    var p = BR.pure.parseTypedQuote(comp.area.value);
    var quote = p ? p.quote : null;
    if (quote === comp.asked) return;
    clearTimeout(comp.qtimer);
    comp.asked = null;
    comp.rcid = null;
    comp.candidates = null;
    comp.target.textContent = '';
    if (!quote) return;
    comp.qtimer = setTimeout(function () {
      if (!comp) return;
      comp.asked = quote;
      comp.rcid = 'q' + (++seq);
      comp.rrev = birtaBus.rev();
      birtaBus.send({ type: 'resolve', cid: comp.rcid, path: CURRENT_PATH, rev: comp.rrev, text: quote });
    }, RESOLVE_MS);
  }

  function showPick() {
    var c = comp.candidates;
    comp.target.textContent = c.length ? c[comp.pick].preview : '→ no match · whole doc';
  }

  function composerKey(e) {
    e.stopPropagation();
    // An IME owns ⏎ and Esc mid-word
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      keep(e.metaKey || e.ctrlKey);
    } else if (e.key === 'Escape' && !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      keep(false, true);
    } else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && comp.candidates && comp.candidates.length > 1) {
      e.preventDefault();
      var last = comp.candidates.length - 1;
      comp.pick = Math.max(0, Math.min(last, comp.pick + (e.key === 'ArrowDown' ? 1 : -1)));
      showPick();
    }
  }

  function close() {
    clearTimeout(comp.timer);
    clearTimeout(comp.qtimer);
    tint(null);
    var host = comp.h.host;
    comp = null;
    hosts.delete('composer');
    host.remove();
    reconcile();
  }

  /**
   * Keeps what the composer holds as an op, or closes it when empty. An edit emptied and kept
   * with ⏎ takes your words back; with Esc (`esc`) it leaves the note as it was.
   */
  function keep(handBack, esc) {
    var text = comp.area.value;
    var hadDraft = comp.sent || unsent.some(function (u) { return u.key === comp.key; });
    var own = comp.own;
    clearTimeout(comp.timer);
    if (!text.trim() || (own && text === own.body)) {
      if (hadDraft) sendCompose('');
      unsent = unsent.filter(function (u) { return u.key !== comp.key; });
      if (!esc && !text.trim() && own && own.body) {
        // A verdict outlives its words; a plain note is only its words
        BR.op(own.verdict ? { op: 'amend', to: own.n, body: '' } : { op: 'retract', to: own.n });
      }
      // An unchanged edit sends nothing, yet ⌘⏎ still hands back
      if (handBack && text.trim()) BR.chrome.handBack();
      close();
      return;
    }
    // A typed quote picks its own target; else the note goes where the composer was opened
    var at = null;
    var body = text;
    var p = BR.pure.parseTypedQuote(text);
    if (p && comp.candidates && p.quote === comp.asked) {
      if (comp.candidates.length) {
        var c = comp.candidates[comp.pick];
        at = aim(c.at, c.tie, comp.rrev);
      } else {
        at = { k: 'doc' };
      }
      body = comp.candidates.length ? p.rest || text : text;
    } else if (p) {
      // Not resolved yet: the server searches for it
      at = { k: 'quote', q: p.quote, pre: '', suf: '' };
      body = p.rest || text;
    }
    var cid;
    if (!at && comp.own) {
      cid = BR.op({ op: 'amend', to: comp.own.n, body: body });
    } else {
      at = at || aim(comp.at, comp.tie, comp.rev);
      if (!at) {
        strand(text);
        return;
      }
      cid = submit(at, body, comp.kind);
    }
    pending[cid] = { key: comp.key, text: text, tie: comp.tie };
    if (hadDraft) sendCompose('');
    unsent = unsent.filter(function (u) { return u.key !== comp.key; });
    if (handBack) afterAck[cid] = BR.chrome.handBack;
    close();
  }

  /** Keeps the composer's words unsent, as its block is gone, and says so. */
  function strand(text) {
    unsent = unsent.filter(function (u) { return u.key !== comp.key; })
      .concat([{ key: comp.key, text: text, tie: comp.tie }]);
    // Kept on the server too, so a reload still offers it
    sendCompose(text);
    BR.chrome.pill(MOVED);
    close();
  }

  /** Sends a note, shows it at once as a dashed draft, and puts it on the z stack. */
  function submit(at, body, kind) {
    var s = state;
    var f = { op: 'comment', at: at, body: body };
    if (kind === 'ask') f.verdict = 'ask';
    var cid = BR.op(f);
    // A folder note lives in the folder's review file, never in this doc's state, and z here can't reach it
    if (at.k === 'dir') return cid;
    drafts.push({ cid: cid, at: at, body: body, seq: ++seq });
    BR.pointing.remember({
      cid: cid,
      created: !s || !s.log || !s.log.lines,
      undo: function (n) { return { op: 'retract', to: n }; }
    });
    return cid;
  }

  /** Keeps a half-typed note as it stands: an edit of its note, or a new note. */
  function keepUnsent(u) {
    var t = keyTarget(u.key);
    if (!t) return;
    var cid;
    var p = BR.pure.parseTypedQuote(u.text);
    if (t.to != null) {
      cid = BR.op({ op: 'amend', to: t.to, body: u.text });
    } else if (p) {
      // As a composer's ⏎ before its quote resolves: the server searches
      cid = submit({ k: 'quote', q: p.quote, pre: '', suf: '' }, p.rest || u.text, t.kind);
    } else {
      var at = u.lost ? aim(t.at, null, null) : aim(t.at, u.tie, t.rev);
      if (!at) {
        BR.chrome.pill(MOVED);
        return;
      }
      cid = submit(at, u.text, t.kind);
    }
    pending[cid] = { key: u.key, text: u.text, tie: u.tie };
    birtaBus.send({ type: 'compose', path: CURRENT_PATH, key: u.key, text: '' });
    unsent = unsent.filter(function (x) { return x !== u; });
    reconcile();
  }

  // --- receipts, not-handled rows, notes and blanks with focus ---

  /** The latest round's claim line, or null before any. */
  function latestTurn() {
    var claims = ((state && state.rounds) || []).map(function (r) { return r.claim; }).filter(function (n) { return n != null; });
    return claims.length ? Math.max.apply(null, claims) : null;
  }

  /** The change of the row that holds the keyboard, whatever its state; null for none. */
  function focusedChange() {
    var a = document.activeElement;
    var hit = null;
    hosts.forEach(function (h) { if (h.host === a && h.entry && h.entry.change) hit = changeOf(h.entry.change.id) || h.entry.change; });
    return hit;
  }

  /** u on a change: revert it, once, when birta can. */
  function revert(c) {
    if (!canUndo(c)) {
      BR.chrome.pill(CANT_UNDO);
      return;
    }
    if (undoing) return;
    undoing = BR.op({ op: 'undo_change', change: c.id });
    reverting = c;
  }

  /** x on an open row is "no" to its text, so it takes a second x within the arm window. */
  function rowNo(c) {
    var sp = c.range && c.range.sp;
    if (!sp) {
      BR.chrome.pill('nothing to say no to · ' + BR.keyName('revert_change') + ' reverts');
      return;
    }
    if (armed && armed.id === c.id && Date.now() < armed.until) {
      armed = null;
      var el = blockOf(sp);
      BR.pointing.verdictOn('no', { k: 'block', sp: sp, scope: el && /^h[1-6]$/.test(el.localName) ? 'section' : 'block' });
      return;
    }
    var mine = armed = { id: c.id, until: Date.now() + CONFIRM_MS };
    var x = BR.keyName('no');
    var revertible = canUndo(c) ? ' · ' + BR.keyName('revert_change') + " reverts Claude's change" : '';
    // The pill lasts as long as the arm, and only x keeps it
    BR.chrome.pill(x + ' = no to this text' + revertible + ' · ' + x + ' again for no', {
      until: mine.until,
      keep: function (e) { return action(e) === 'no'; },
      off: function () { if (armed === mine) armed = null; }
    });
  }

  /** A row key on change `c`: y keeps it, x says no, u reverts it, c notes it. */
  function rowKey(a, c) {
    if (a !== 'no') armed = null;
    if (!writable()) return;
    if (a === 'revert_change') revert(c);
    else if (a === 'no') rowNo(c);
    else if (a === 'yes') BR.op({ op: 'seen', ids: [c.id] });
    else if (a === 'note') open('note', { k: 'change', id: c.id }, null);
  }

  function lostKey(e) {
    if (e.defaultPrevented || e.isComposing || e.key !== 'Enter' || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    e.stopPropagation();
    toggleLost();
  }

  // A not-handled row needs y to be acknowledged; stepping or clicking onto it never does
  function notHandledKey(e, n) {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || action(e) !== 'yes') return;
    if (writable()) BR.op({ op: 'seen', ids: ['nh:' + n] });
    e.preventDefault();
    e.stopPropagation();
  }

  function action(e) {
    return BR.pure.keyAction(e, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS);
  }

  /** Whether a keydown is any key the page binds, review's or the viewer's. */
  function bound(e) {
    var kb = typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS;
    return !!kb && Object.keys(kb).some(function (k) { return BR.pure.keyMatches(e, kb[k]); });
  }

  // Keys that write act on the note or not at all, never on the doc behind it; the rest act as on the page
  function noteKey(e, h) {
    var entry = h.entry;
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    var a = action(e);
    var unsentEnter = !!entry.unsent && e.key === 'Enter' && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey;
    // On words not yet sent, c and q are how they are re-pointed
    if (!unsentEnter && !(entry.unsent ? VERDICT_OF[a] : NOTE_KEYS[a])) return;
    e.preventDefault();
    e.stopPropagation();
    if (!writable()) return;
    if (unsentEnter) keepUnsent(entry.unsent);
    else if (a === 'yes' && entry.item && settled(entry.item)) BR.op({ op: 'seen', ids: ['#' + entry.item.n] });
  }

  /**
   * A closed blank: verdict keys answer, ⏎ and c open the field, moving and taking back act as on
   * the page, and any other letter says how to type. An open field has every key.
   */
  function blankKey(e, h) {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || !writable()) return;
    if (h.blank.slot.querySelector('textarea')) return;
    var a = action(e);
    var plainEnter = e.key === 'Enter' && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey;
    if (BLANK_PASSES[a]) return;
    if (VERDICT_OF[a]) answerWith(h, VERDICT_OF[a]);
    else if (plainEnter || a === 'note') openAnswer(h);
    else if (a === 'ask' || (!bound(e) && /^[a-z]$/i.test(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey)) {
      BR.chrome.pill(typeIt());
    } else return;
    e.preventDefault();
    e.stopPropagation();
  }

  function typeIt() { return TYPE_IT.replace('⏎', BR.keyName('open')); }

  /** The answer the blank `n` holds that no hand-back took yet, or null. */
  function draftAnswer(n) {
    var item = ((state && state.items) || []).filter(function (i) { return i.n === n; })[0];
    return item && asking(item) && !answerSent(item) ? lastHumanReply(item) : null;
  }

  /** An op that puts answer `r` back as it was. */
  function restore(r) {
    var f = { op: 'amend', to: r.n, body: r.body || '' };
    if (r.verdict) f.verdict = r.verdict;
    else f.drop_verdict = true;
    return f;
  }

  /** A verdict key on a blank: the answer, or a change to the one not yet handed back. */
  function answerWith(h, verdict) {
    var n = h.blank.n;
    var r = draftAnswer(n);
    if (!r) {
      BR.pointing.remember({
        cid: BR.op({ op: 'reply', to: n, verdict: verdict }),
        undo: function (line) { return { op: 'retract', to: line }; }
      });
    } else if (r.verdict !== verdict || r.body) {
      var f = { op: 'amend', to: r.n, verdict: verdict };
      // The key's answer stands alone, so typed words go
      if (r.body) f.body = '';
      BR.pointing.remember({ cid: BR.op(f), item: r.n, undo: function () { return restore(r); } });
    }
    BR.chrome.pill('answered ' + ANSWERED[verdict] + ' · ' + BR.keyName('undo') + ' to undo');
  }

  /** A blank whose written answer is open holds typed words, which reading mode must keep. */
  function typing(h) {
    return !!(h.blank && h.blank.slot.querySelector('textarea'));
  }

  /** Opens blank `h`'s field, holding its answer not yet handed back, else `text`. */
  function openAnswer(h, text) {
    var n = h.blank.n;
    var slot = h.blank.slot;
    var r = draftAnswer(n);
    // Its job is done once the field is open
    if (BR.chrome.pillSays(typeIt())) BR.chrome.pill('');
    h.host.removeAttribute('data-br-hidden');
    slot.hidden = false;
    h.blank.hint.hidden = true;
    var label = el('div', 'muted', 'answer to ' + (h.blank.to || 'the agent'));
    var area = document.createElement('textarea');
    area.setAttribute('dir', 'auto');
    area.placeholder = BLANK_HINT;
    area.value = text != null ? text : (r && r.body) || '';
    slot.append(label, area);
    area.focus();
    area.setSelectionRange(area.value.length, area.value.length);
    var closed = false;
    function done(how) {
      if (closed) return;
      closed = true;
      var words = area.value;
      label.remove();
      area.remove();
      h.blank.hint.hidden = !!h.blank.quiet;
      slot.hidden = !!draftAnswer(n);
      if (how !== 'blur') h.box.focus({ preventScroll: true });
      if (!writable()) return;
      var cid = null;
      if (!words.trim()) {
        // Emptied, ⏎ takes the answer back; Esc and a blur leave it as it was
        if (how === 'enter' && r && r.body) cid = BR.op({ op: 'retract', to: r.n });
        return;
      }
      if (r) {
        if (words === r.body) return;
        var f = { op: 'amend', to: r.n, body: words };
        if (r.verdict) f.drop_verdict = true;
        cid = BR.op(f);
        BR.pointing.remember({ cid: cid, item: r.n, undo: function () { return restore(r); } });
      } else {
        cid = BR.op({ op: 'reply', to: n, body: words });
        BR.pointing.remember({ cid: cid, undo: function (line) { return { op: 'retract', to: line }; } });
      }
      pending[cid] = { blank: n, text: words };
    }
    area.addEventListener('keydown', function (ev) {
      ev.stopPropagation();
      if (ev.defaultPrevented || ev.isComposing || ev.keyCode === 229) return;
      if (ev.key === 'Escape' && !ev.shiftKey && !ev.altKey && !ev.ctrlKey && !ev.metaKey) {
        ev.preventDefault();
        done('esc');
      } else if (ev.key === 'Enter' && !ev.shiftKey) {
        ev.preventDefault();
        done('enter');
      }
    });
    // Leaving the field keeps its words, as Esc does; a redraw that removed it already lost them
    area.addEventListener('blur', function () {
      setTimeout(function () { if (area.isConnected && document.activeElement !== h.host) done('blur'); }, 0);
    });
  }

  // --- frames ---

  function onFrame(msg) {
    switch (msg.type) {
      case 'dir_state':
        var n = (msg.folder && msg.folder.summary && msg.folder.summary.notes) || 0;
        if (n !== folderNotes) {
          folderNotes = n;
          reconcile();
        }
        break;
      case 'review_state':
        if (msg.path === CURRENT_PATH && msg.state) skew = (msg.state.server_now || Date.now()) - Date.now();
        break;
      case 'resolved':
        if (!comp || msg.cid !== comp.rcid) break;
        // An answer for a quote since edited away is stale
        var p = BR.pure.parseTypedQuote(comp.area.value);
        if (!p || p.quote !== comp.asked) break;
        comp.candidates = (msg.candidates || []).map(function (c) {
          // Its sp names a block here only while the page still shows the rev it asked about
          var el = c.at && c.at.sp && comp.rrev === birtaBus.rev() ? blockOf(c.at.sp) : null;
          return { at: c.at, preview: c.preview, tie: tie(el) };
        });
        comp.pick = 0;
        showPick();
        break;
      case 'compose_restore':
        if (msg.path !== CURRENT_PATH) break;
        var had = {};
        unsent.forEach(function (u) { had[u.key] = u; });
        unsent = (msg.items || []).filter(function (u) { return typeof u.key === 'string' && typeof u.text === 'string'; })
          .map(function (u) {
            var was = had[u.key];
            if (was) return { key: u.key, text: u.text, tie: was.tie, lost: was.lost };
            // Typed at the rev on screen: its block is found now, so a write that only moves it keeps it
            var t = keyTarget(u.key);
            var el = t && t.at.sp && t.rev === birtaBus.rev() ? blockOf(t.at.sp) : null;
            return el ? { key: u.key, text: u.text, tie: tie(el) } : { key: u.key, text: u.text };
          });
        reconcile();
        break;
      case 'ack':
        if (msg.cid === undoing) {
          undoing = null;
          BR.chrome.pill('reverted · ' + labelOf(reverting));
        }
        delete pending[msg.cid];
        if (afterAck[msg.cid]) {
          var run = afterAck[msg.cid];
          delete afterAck[msg.cid];
          run();
        }
        break;
      case 'nack':
        if (msg.cid === undoing) undoing = null;
        delete afterAck[msg.cid];
        if (msg.reason) BR.chrome.nack(msg.reason);
        drafts = drafts.filter(function (d) { return d.cid !== msg.cid; });
        var words = pending[msg.cid];
        delete pending[msg.cid];
        if (words) giveBack(words, msg.reason);
        reconcile();
        break;
    }
  }

  /** Hands a refused op's words back: a blank's answer to its blank, a note as half-typed. */
  function giveBack(words, reason) {
    if (words.blank != null) {
      var h = hosts.get('b' + words.blank);
      if (h && h.blank && !h.blank.slot.querySelector('textarea')) openAnswer(h, words.text);
      return;
    }
    // An anchor the server could not place is never sent as it was again
    var u = { key: words.key, text: words.text, tie: words.tie, lost: reason === 'bad_anchor' };
    // Kept on the server too, so a reload still offers it
    unsent = unsent.filter(function (x) { return x.key !== words.key; }).concat([u]);
    birtaBus.send({ type: 'compose', path: CURRENT_PATH, key: words.key, text: words.text });
  }

  BR.flow = {
    // Core forwards frames, so the flow has each one before core draws
    frame: onFrame,
    /** Redraws the flow for `s`, the state of the page's rev, or null. */
    reconcile: function (s) {
      state = s;
      reconcile();
    },
    open: open,
    /** The Esc layer: keeps or closes the composer. False when none is open. */
    escape: function () {
      if (!comp) return false;
      keep(false, true);
      return true;
    },
    /** ⏎ on a block, or on the doc for null: keeps its half-typed note, else opens its dot. */
    openAt: function (block) {
      var sp = block ? block.getAttribute('data-sourcepos') : null;
      var u = unsent.filter(function (x) {
        var t = keyTarget(x.key);
        if (!t) return false;
        if (!sp) return t.at.k === 'doc';
        // Only words still known to be this block's: an sp from another rev may be another block's
        return fresh(x, t) && (x.tie ? x.tie.el === block : t.at.sp === sp);
      })[0];
      if (u && writable()) {
        keepUnsent(u);
        return true;
      }
      if (!state) return false;
      if (!sp) {
        if (!(state.lost || []).length) return false;
        toggleLost();
        return true;
      }
      // ⏎ again folds what ⏎ opened
      var here = (state.items || []).filter(function (i) { return slotOfItem(i).sp === sp; });
      var open = here.filter(function (i) { return expanded[i.n]; });
      var hit = open.length > 0;
      // The clean view shows every note on a block as its dot
      var clean = BR.chrome.clean();
      var lostNow = {};
      (state.lost || []).forEach(function (n) { lostNow[n] = 1; });
      if (hit) open.forEach(function (i) { delete expanded[i.n]; });
      else {
        here.forEach(function (i) {
          if (clean ? lostNow[i.n] || i.status === 'retracted' : !folded(i, slotOfItem(i))) return;
          expanded[i.n] = 1;
          hit = true;
        });
      }
      if (hit) reconcile();
      return hit;
    },
    /** Draws the flow again for the state it has, as when a hold ends with no new state. */
    refresh: function () { reconcile(); },
    /** The open composer's box, or null. */
    composerRect: function () {
      return comp && comp.h.host.isConnected ? comp.h.host.getBoundingClientRect() : null;
    },
    rewriteActive: function () { return !!(state && state.rewrite); },
    /**
     * Puts the keys where a walk stop takes them: a change's receipt, a not-handled row, or a
     * blank. False when the stop has no line of its own.
     */
    focusStop: function (stop) {
      if (stop.kind === 'change') {
        landed = stop.id;
        reconcile();
      }
      var hit = null;
      hosts.forEach(function (h) {
        var e = h.entry;
        if (hit || !e || !h.host.isConnected) return;
        if ((stop.kind === 'change' && e.change && e.change.id === stop.id) ||
          (stop.kind === 'not_handled' && 'nh:' + e.nh === stop.id) ||
          (stop.kind === 'blank' && e.kind === 'blank' && '#' + e.blank === stop.id) ||
          (stop.kind === 'lost' && e.kind === 'lost')) hit = h;
      });
      if (hit) hit.box.focus({ preventScroll: true });
      return hit ? hit.host : null;
    },
    /**
     * The open row whose keys act, which takes y, x, u and c: this round's, never reverted. Null
     * when none is, and always in the clean view.
     */
    openRow: function () {
      if (BR.chrome.clean()) return null;
      // The row that holds the keyboard is the one meant, over the rows its block opened
      var c = focusedChange();
      if (c) return muted(c) ? null : c;
      return keyedRow ? changeOf(keyedRow) : null;
    },
    /** The row that holds the keyboard, else the row on show for the focus, muted or not; null for none. */
    focusedRow: function () {
      return focusedChange() || (openIds.length ? changeOf(openIds[0]) : null);
    },
    /** Takes back a first x on a row: any other key does. */
    disarm: function () { armed = null; },
    rowKey: rowKey,
    /** The blank hosts on the page, which j and k stop on like blocks. */
    blanks: function () {
      var out = [];
      hosts.forEach(function (h) {
        if (h.blank && h.host.isConnected && h.host.getClientRects().length) out.push(h.host);
      });
      return out;
    },
    /** Gives blank `host` the keyboard. */
    hold: function (host) {
      hosts.forEach(function (h) { if (h.host === host) h.box.focus({ preventScroll: true }); });
    },
    /** Lets a host the focus has moved past give up the keyboard; an open field keeps it. */
    release: function (node) {
      hosts.forEach(function (h) { if (h.host === node && !typing(h) && h !== (comp && comp.h)) h.box.blur(); });
    },
    /** Focus moved to `el`, or to the doc for null: the rows under it open, and any other closes. */
    focusBlock: function (el) {
      var sp = el && el.getAttribute ? el.getAttribute('data-sourcepos') : null;
      if (sp === focusSp && !landed) return;
      focusSp = sp;
      landed = null;
      // Focus moved off an open composer, as a click elsewhere does: the redraw mustn't take it back
      if (comp && document.activeElement !== comp.h.host) comp.active = false;
      reconcile();
    },
    markRewriteSeen: markRewriteSeen,
    /** What request `at` points at, in words, as a composer's label names it. */
    label: label,
    /** A block in words, as every label names it: `## X (section)`, else `"its first words"`, `(kind)` added when asked. */
    blockLabel: blockLabel,
    /** Rules for the overlay: each host with its style and role. */
    rules: function () {
      var out = [];
      hosts.forEach(function (h) {
        if (h.rule && h.host.isConnected) out.push({ el: h.host, style: h.rule[0], role: h.rule[1] });
      });
      return out;
    },
    dots: function () { return dotList; },
    /** Reading mode: hides every note but an open composer or a blank being answered. */
    hide: function (on) {
      hiddenAll = on;
      hosts.forEach(function (h) { h.host.toggleAttribute('data-br-hidden', on && !typing(h)); });
    },
    /** The hand-back or approve still in its hold, by the server's clock, or null. */
    holdActive: function () {
      var s = BR.currentState();
      return s && s.hold && Date.now() + skew < s.hold.not_before ? s.hold : null;
    }
  };
})();
