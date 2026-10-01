var BR = typeof BR === 'undefined' ? {} : BR;
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
   * What `n` and `p` walk, in order: unseen changes, then unseen agent replies, open agent
   * questions and not-handled rows, each in doc order with the doc slot first. Each stop is
   * `{kind, id, sp}`, with `id` the one its seen op would carry.
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

  var VERDICT_OF = { yes: 'yes', no: 'no', later: 'later', your_call: 'your_call' };
  // A seen note in one of these folds to its dot
  var FOLDS = { handled: 1, answered: 1, later: 1 };
  // Open blanks past this many show only their question until focused
  var OPEN_BLANKS = 5;
  // How long a first x on a many-hunk change waits for the second
  var CONFIRM_MS = 3000;
  var CANT_UNDO = "can't undo this one · c instead";
  var RESOLVE_MS = 120;
  var COMPOSE_MS = 300;
  var QUOTE_CHARS = 40;
  var UNSENT = 'unsent · ⏎ to keep';

  // Group order under one anchor: notes with their replies, then what the page added, then facts
  var G_NOTE = 1, G_DRAFT = 2, G_UNSENT = 3, G_COMPOSER = 4, G_RECEIPT = 5, G_FACT = 6, G_BLANK = 7,
    G_TURN = 8, G_LOST = 9, G_REWRITE = 10, G_FOLDER = 11;

  var hosts = new Map();
  var state = null;
  var skew = 0;
  // Kept notes the snapshot doesn't hold yet, by cid
  var drafts = [];
  // Half-typed notes: {key, text}
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
  // Words an op carries that a nack must hand back: {key, text} for notes and edits, {blank, text} for answers
  var pending = {};
  // A many-hunk undo waiting for its second x: {id, until}
  var armed = null;
  // The cid of an undo the server hasn't answered, so a second x can't send another
  var undoing = null;

  function content() { return document.getElementById('content'); }
  function narrow() { return window.innerWidth < 768; }
  function writable() { return !state || state.log_writable !== false; }

  function blockOf(sp) {
    // A list shares its first item's sp and comes first, so the last match is the item
    var all = content().querySelectorAll('[data-sourcepos="' + sp + '"]');
    return all[all.length - 1] || null;
  }

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

  /** A half-typed note's key: its target, its kind, and the note its words edit, if any. */
  function composeKey(at, kind, to) {
    var k = { at: at, kind: kind };
    if (to != null) k.to = to;
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

  /** Where an item of the state goes: under the block it is on now, or in the doc slot. */
  function slotOfItem(i) {
    return i.at && i.at.kind !== 'doc' && i.at.loc ? { sp: i.at.loc.sp } : { doc: true };
  }

  // --- hosts ---

  function makeHost(id) {
    var host = document.createElement('birta-flow');
    host.toggleAttribute('data-br-hidden', hiddenAll);
    var root = host.attachShadow({ mode: 'closed' });
    var style = document.createElement('style');
    style.textContent = BR.shadowCss || '';
    var box = document.createElement('div');
    box.className = 'flow';
    root.append(style, box);
    BR.FLOW_HOSTS.add(host);
    var h = { id: id, host: host, box: box, sig: null, rule: null, entry: null };
    box.addEventListener('keydown', function (ev) {
      if (h.blank && h.entry && h.entry.kind === 'blank') blankKey(ev, h.blank.n, h.blank.slot);
      else if (h.entry && h.entry.change) receiptKey(ev, h.entry.change);
      else if (h.entry && h.entry.nh != null) notHandledKey(ev, h.entry.nh);
      else if (h.entry && h.entry.focusable) noteKey(ev, h);
    });
    // A quiet blank shows its slot while it has focus
    box.addEventListener('focusin', function () { if (h.blank) h.blank.hint.hidden = false; });
    box.addEventListener('focusout', function () {
      if (h.blank && h.blank.quiet && !h.blank.slot.querySelector('textarea')) h.blank.hint.hidden = true;
    });
    box.addEventListener('click', function () {
      if (h.entry && h.entry.kind === 'rewrite' && state && state.rewrite) markRewriteSeen();
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

  // Each key is its own text, so a bidi override in a quoted body can't reorder them
  function keysRow(parent, keys) {
    parent.append(' · ');
    keys.forEach(function (k, i) {
      if (i) parent.append('  ');
      parent.appendChild(el('span', 'key', k));
    });
  }

  function isolated(text) { return el('bdi', null, text); }

  /** Draws `e` into its host unless nothing it shows has changed. */
  function render(h, e) {
    h.host.setAttribute('data-kind', e.kind);
    h.rule = e.rule || null;
    h.entry = e;
    var sig = JSON.stringify([e.kind, e.lead, e.text, e.tail, e.prefix, e.meta, e.keys, e.fold, e.quiet, e.lost, lostOpen]);
    if (h.sig === sig) return;
    h.sig = sig;
    h.box.textContent = '';
    h.blank = null;
    if (e.kind === 'blank') return renderBlank(h, e);
    if (e.kind === 'lost') return renderLost(h, e);
    if (e.kind === 'folder') return renderFolder(h, e);
    if (e.prefix) h.box.appendChild(el('div', 'muted', e.prefix));
    var line = el('div', e.fold ? 'text fold' : 'text');
    if (e.lead) line.append(e.lead);
    if (e.text) line.appendChild(isolated(e.text));
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
    if (e.focusable) h.box.tabIndex = 0;
    else h.box.removeAttribute('tabindex');
  }

  // The question reads like a note; under it the dashed slot takes the answer
  function renderBlank(h, e) {
    h.box.tabIndex = 0;
    h.box.appendChild(el('div', 'text')).appendChild(isolated(e.text));
    var slot = el('div', 'blank');
    var hint = el('div', 'muted', 'y x . l or type');
    hint.hidden = !!e.quiet;
    slot.appendChild(hint);
    h.box.appendChild(slot);
    h.blank = { n: e.blank, slot: slot, hint: hint, quiet: !!e.quiet };
  }

  function renderLost(h, e) {
    var row = el('div', 'muted', plural(e.lost.length, 'note lost its place ', 'notes lost their place '));
    var open = el('span', 'more', '▸');
    open.addEventListener('click', function () {
      lostOpen = !lostOpen;
      reconcile();
    });
    row.appendChild(open);
    h.box.appendChild(row);
    if (!lostOpen) return;
    e.lost.forEach(function (i) {
      var quote = i.at && (i.at.shown || i.at.quote);
      var item = el('div', 'text');
      if (quote) item.appendChild(el('div', 'muted', '→ "' + clamp(quote) + '"'));
      item.appendChild(isolated(i.body || i.verdict || ''));
      h.box.appendChild(item);
    });
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

  function canUndo(c) {
    return !!c.reversible && c.reversible.ok && !(state && state.writable === false);
  }

  /** `e` as the line a change answers to: its keys follow whether it can be undone. */
  function receiptOf(c, e) {
    if (!c) return e;
    e.change = c;
    e.keys = canUndo(c) ? ['y keep', 'x undo', 'c instead'] : ['y keep', 'c instead'];
    e.focusable = true;
    e.seen = c.seen ? null : c.id;
    return e;
  }

  function whatOf(c) {
    var w = (c && c.what) || {};
    if (w.words) return plural(w.words, 'word changed', 'words changed');
    return plural(w.old_lines || 0, 'line', 'lines') + ' → ' + (w.new_lines || 0);
  }

  function folded(i, slot) {
    return !!i.seen && settled(i) && !!slot.sp && !expanded[i.n] && !!blockOf(slot.sp);
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
    dotList = [];
    function add(e, slot, order) {
      e.slot = slot;
      e.order = order;
      out.push(e);
    }
    if (s) {
      (s.lost || []).forEach(function (n) { lost[n] = 1; });
      (s.items || []).forEach(function (i) {
        if (i.cid) cids[i.cid] = 1;
        if (lost[i.n] || i.status === 'retracted') return;
        var slot = slotOfItem(i);
        var agent = i.by && i.by.type === 'agent';
        // Its done reply carries the change's keys, folded or not
        var named = i.outcome && i.outcome.done && !i.outcome.no_change && i.outcome.changes.length ? i.outcome.changes[0] : null;
        if (named) keyed[named] = 1;
        if (folded(i, slot)) {
          dotList.push({ sp: slot.sp, role: agent ? 'agent' : 'you' });
          return;
        }
        if (asking(i)) {
          add({ id: 'b' + i.n, kind: 'blank', text: i.body || '', blank: i.n, open: i.status === 'open',
            rule: ['dotted', 'agent'] }, slot, [G_BLANK, i.n, 0]);
          // The latest answer wins, so only it shows
          var answer = lastHumanReply(i);
          if (answer) {
            add({ id: 'r' + answer.n, kind: 'reply', lead: '↳ ' + who(answer.by) + ' · ', text: answer.body || answer.verdict || '',
              fold: true, rule: ['solid', 'you'] }, slot, [G_BLANK, i.n, 1]);
          }
          return;
        }
        // A bare verdict is its glyph; only a row left not handled shows under it
        var bare = !i.body && !agent && !(i.replies || []).length;
        if (bare && i.status !== 'not_handled') return;
        var group = agent ? G_BLANK : G_NOTE;
        var quote = i.at && i.at.scope === 'quote' ? i.at.shown || i.at.quote : null;
        if (!bare) {
          add({ id: 'n' + i.n, kind: 'note', text: i.body || '', prefix: quote ? '→ "' + clamp(quote) + '"' : null,
            item: i, focusable: true,
            rule: agent ? ['dotted', 'agent'] : [i.status === 'draft' ? 'dashed' : 'solid', 'you'] }, slot, [group, i.n, 0]);
        }
        (i.replies || []).forEach(function (r, k) {
          var human = r.by && r.by.type === 'human';
          var lead = '↳ ' + who(r.by) + ' · ' + (r.kind === 'didnt' ? "didn't · " : '');
          add({ id: 'r' + r.n, kind: 'reply', lead: lead, text: r.body || r.verdict || '', fold: true,
            seen: !human && !r.seen ? r.id : null, rule: ['solid', human ? 'you' : 'agent'] }, slot, [group, i.n, 1 + k]);
        });
        var name = turnAgent(i.turn);
        if (i.outcome && i.outcome.done) {
          var change = !i.outcome.no_change && i.outcome.changes.length ? changeOf(i.outcome.changes[0]) : null;
          add(receiptOf(change, { id: 'o' + i.n, kind: 'reply', lead: '↳ ' + name + ' · done · ',
            text: change ? whatOf(change) : 'no change in text', rule: ['solid', 'agent'] }), slot, [group, i.n, 1e6]);
        }
        if (i.status === 'not_handled') {
          var replied = s.phrase && s.phrase.replied;
          var when = replied && replied.turn === i.turn && replied.ts ? hhmm(replied.ts) : 'earlier';
          add({ id: 'h' + i.n, kind: 'reply', text: '↳ not handled in ' + name + "'s " + when + ' turn',
            nh: i.n, focusable: true, rule: ['solid', 'agent'] }, slot, [group, i.n, 1e6 + 1]);
        }
      });
      var removed = {};
      (s.deletions || []).forEach(function (d) { removed[d.change] = d; });
      (s.changes || []).forEach(function (c) {
        // A removed block's change goes under the block above its tick
        var d = removed[c.id];
        var slot = c.range && c.range.sp ? { sp: c.range.sp } : d ? (d.after_sp ? { sp: d.after_sp } : { doc: true }) : null;
        if (!slot || keyed[c.id] || c.attr === 'human') return;
        var count = linesOf(c);
        var order = [G_RECEIPT, c.op, c.idx];
        if (c.attr === 'outside') {
          add(receiptOf(c, { id: 'f' + c.id, kind: 'fact', text: 'changed outside birta' + (c.cause === 'git' ? ' (git)' : ''),
            rule: ['dotted', 'agent'] }), slot, [G_FACT, c.op, c.idx]);
        } else if (c.asked && (c.asked.kind === 'cited' || c.asked.kind === 'location')) {
          var note = (s.items || []).filter(function (i) { return c.asked.notes.indexOf(i.n) >= 0; })[0];
          add(receiptOf(c, { id: 'c' + c.id, kind: 'receipt', lead: who(c.by) + ' · because you said "',
            text: (note && note.body) || '', tail: '" · ' + count, rule: ['solid', 'agent'] }), slot, order);
        } else {
          add(receiptOf(c, { id: 'c' + c.id, kind: 'receipt', lead: who(c.by) + ' · nobody asked · ', text: count,
            rule: ['dotted', 'agent'] }), slot, order);
        }
      });
      var replied = s.phrase && s.phrase.replied;
      if (replied && replied.note) {
        add({ id: 'turn', kind: 'turn_note', lead: (replied.by ? capital(replied.by) : 'an agent') + ' · ', text: replied.note,
          rule: ['dotted', 'agent'] }, { doc: true }, [G_TURN, 0, 0]);
      }
      var gone = (s.items || []).filter(function (i) { return lost[i.n]; });
      if (gone.length) add({ id: 'lost', kind: 'lost', lost: gone }, { doc: true }, [G_LOST, 0, 0]);
      var row = rewriteRow(s);
      if (row) add(row, { doc: true }, [G_REWRITE, 0, 0]);
    }
    if (folderNotes > 0 && BOOT.review.dir) {
      add({ id: 'folder', kind: 'folder', text: plural(folderNotes, 'note on this folder', 'notes on this folder') },
        { doc: true }, [G_FOLDER, 0, 0]);
    }
    drafts.forEach(function (d) {
      if (cids[d.cid]) return;
      add({ id: 'd' + d.cid, kind: 'note', text: d.body, rule: ['dashed', 'you'] }, slotOfAt(d.at), [G_DRAFT, d.seq, 0]);
    });
    unsent.forEach(function (u, k) {
      if (comp && comp.key === u.key) return;
      var t = keyTarget(u.key);
      if (!t) return;
      add({ id: 'u' + u.key, kind: 'note', text: u.text, meta: UNSENT, unsent: u, focusable: true, rule: ['dashed', 'you'] },
        slotOfAt(t.at), [G_UNSENT, k, 0]);
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
    return { id: 'rewrite', kind: 'rewrite', text: name + ' rewrote most of it' + (s.rewrite ? ' · y mark seen' : '') };
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
    if (BR.lane && BR.lane.redraw) BR.lane.redraw();
  }

  // --- the composer ---

  /** The composer's block: its own element while it lives, else the new one at its line. */
  function composerSlot() {
    if (!comp.anchor) return { doc: true };
    if (comp.anchor.isConnected) return { el: comp.anchor };
    var line = comp.sp.split(':')[0] + ':';
    var found = Array.prototype.filter.call(content().querySelectorAll('[data-sourcepos]'), function (e) {
      return e.localName === comp.anchor.localName && e.getAttribute('data-sourcepos').indexOf(line) === 0;
    });
    if (found.length) comp.anchor = found[found.length - 1];
    return comp.anchor.isConnected ? { el: comp.anchor } : { doc: true };
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
   * target, whose words the note then edits.
   */
  function open(kind, at, own) {
    if (comp) keep(false);
    var key = composeKey(at, kind, own ? own.n : null);
    var h = makeHost('composer');
    h.host.setAttribute('data-kind', 'composer');
    h.box.classList.add('composer');
    var sp = spOfAt(at);
    var anchor = sp ? blockOf(sp) : null;
    comp = { h: h, kind: kind, at: at, key: key, own: own, anchor: anchor, sp: anchor ? sp : null,
      candidates: null, pick: 0, asked: null, rcid: null, timer: null, qtimer: null, sent: false, active: true };
    var target = el('div', 'muted target');
    var area = document.createElement('textarea');
    area.setAttribute('dir', 'auto');
    if (kind === 'ask') {
      area.placeholder = 'explain (optional words)';
      area.rows = 1;
    }
    var mac = /Mac/.test(navigator.platform);
    var hint = kind === 'ask' ? '⏎ keep · esc close' : '⏎ keep · esc close · ' + (mac ? '⌘⏎' : 'Ctrl⏎') + ' hand back';
    h.box.append(target, area, el('div', 'muted hint', hint));
    comp.area = area;
    comp.target = target;
    var restored = unsent.filter(function (u) { return u.key === key; })[0];
    if (restored) area.value = restored.text;
    else if (own && own.body) area.value = own.body;
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
      birtaBus.send({ type: 'resolve', cid: comp.rcid, path: CURRENT_PATH, rev: birtaBus.rev(), text: quote });
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
      keep(false);
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
    var host = comp.h.host;
    comp = null;
    hosts.delete('composer');
    host.remove();
    reconcile();
  }

  /** Keeps what the composer holds as an op, or closes it when empty. */
  function keep(handBack) {
    var text = comp.area.value;
    var hadDraft = comp.sent || unsent.some(function (u) { return u.key === comp.key; });
    clearTimeout(comp.timer);
    if (!text.trim()) {
      if (hadDraft) sendCompose('');
      unsent = unsent.filter(function (u) { return u.key !== comp.key; });
      close();
      return;
    }
    var at = comp.at;
    var body = text;
    var p = BR.pure.parseTypedQuote(text);
    if (p && comp.candidates && p.quote === comp.asked) {
      if (comp.candidates.length) {
        at = comp.candidates[comp.pick].at;
        body = p.rest || text;
      } else {
        at = { k: 'doc' };
      }
    } else if (p) {
      // Not resolved yet: the server searches for it
      at = { k: 'quote', q: p.quote, pre: '', suf: '' };
      body = p.rest || text;
    }
    var cid;
    if (comp.own && at === comp.at) {
      cid = BR.op({ op: 'amend', to: comp.own.n, body: body });
    } else {
      cid = submit(at, body, comp.kind);
    }
    pending[cid] = { key: comp.key, text: text };
    if (hadDraft) sendCompose('');
    unsent = unsent.filter(function (u) { return u.key !== comp.key; });
    if (handBack) afterAck[cid] = BR.chrome.handBack;
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
      cid = submit(t.at, u.text, t.kind);
    }
    pending[cid] = { key: u.key, text: u.text };
    birtaBus.send({ type: 'compose', path: CURRENT_PATH, key: u.key, text: '' });
    unsent = unsent.filter(function (x) { return x !== u; });
    reconcile();
  }

  // --- receipts, not-handled rows, notes and blanks with focus ---

  /** x on a change: undo it, after a second x for a many-hunk write. */
  function undo(c) {
    if (!canUndo(c)) {
      BR.chrome.pill(CANT_UNDO);
      return;
    }
    if (undoing) return;
    var many = (c.op_hunks || 1) > 1;
    if (many && !(armed && armed.id === c.id && Date.now() < armed.until)) {
      armed = { id: c.id, until: Date.now() + CONFIRM_MS };
      BR.chrome.pill('x again to undo ' + c.op_hunks + ' changes');
      return;
    }
    armed = null;
    var f = { op: 'undo_change', change: c.id };
    if (many) f.confirm = true;
    undoing = BR.op(f);
  }

  // Keys on a receipt act on its change; every other review key acts as on the page
  function receiptKey(e, c) {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    var a = action(e);
    var mine = a === 'no' || a === 'yes' || a === 'note';
    if (mine && writable()) {
      if (a !== 'no') armed = null;
      if (a === 'no') undo(c);
      else if (a === 'yes') BR.op({ op: 'seen', ids: [c.id] });
      else open('note', { k: 'change', id: c.id }, null);
    } else if (!mine && !BR.keys.run(e)) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  // A not-handled row needs y to be acknowledged; stepping or clicking onto it never does
  function notHandledKey(e, n) {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    var a = action(e);
    if (a === 'yes') {
      if (writable()) BR.op({ op: 'seen', ids: ['nh:' + n] });
    } else if (!BR.keys.run(e)) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function action(e) {
    return BR.pure.keyAction(e, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS);
  }

  function noteKey(e, h) {
    var entry = h.entry;
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || !writable()) return;
    if (entry.unsent && e.key === 'Enter' && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      e.stopPropagation();
      keepUnsent(entry.unsent);
    } else if (entry.item && action(e) === 'yes' && settled(entry.item)) {
      e.preventDefault();
      e.stopPropagation();
      BR.op({ op: 'seen', ids: ['#' + entry.item.n] });
    }
  }

  function blankKey(e, n, slot) {
    e.stopPropagation();
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || !writable()) return;
    if (slot.querySelector('textarea')) return;
    var verdict = VERDICT_OF[action(e)];
    if (verdict) {
      e.preventDefault();
      BR.op({ op: 'reply', to: n, verdict: verdict });
      return;
    }
    if (e.key.length !== 1 || e.ctrlKey || e.metaKey || e.altKey) return;
    e.preventDefault();
    answer(slot, n, e.key);
  }

  /** A blank whose written answer is open holds typed words, which reading mode must keep. */
  function typing(h) {
    return !!(h.blank && h.blank.slot.querySelector('textarea'));
  }

  /** Opens the blank's written answer holding `text`. */
  function answer(slot, n, text) {
    hosts.forEach(function (h) { if (h.blank && h.blank.slot === slot) h.host.removeAttribute('data-br-hidden'); });
    var area = document.createElement('textarea');
    area.setAttribute('dir', 'auto');
    area.value = text;
    slot.appendChild(area);
    area.focus();
    area.setSelectionRange(text.length, text.length);
    area.addEventListener('keydown', function (ev) {
      ev.stopPropagation();
      if (ev.defaultPrevented || ev.isComposing || ev.keyCode === 229) return;
      var done = ev.key === 'Escape' || (ev.key === 'Enter' && !ev.shiftKey);
      if (!done) return;
      ev.preventDefault();
      // Esc keeps written words, as it does in the composer
      if (area.value.trim() && writable()) {
        var cid = BR.op({ op: 'reply', to: n, body: area.value });
        pending[cid] = { blank: n, text: area.value };
      }
      area.remove();
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
        comp.candidates = msg.candidates || [];
        comp.pick = 0;
        showPick();
        break;
      case 'compose_restore':
        if (msg.path !== CURRENT_PATH) break;
        unsent = (msg.items || []).filter(function (u) { return typeof u.key === 'string' && typeof u.text === 'string'; });
        reconcile();
        break;
      case 'ack':
        if (msg.cid === undoing) undoing = null;
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
        if (words) giveBack(words);
        reconcile();
        break;
    }
  }

  /** Hands a refused op's words back: a blank's answer to its blank, a note as half-typed. */
  function giveBack(words) {
    if (words.blank != null) {
      var h = hosts.get('b' + words.blank);
      if (h && h.blank && !h.blank.slot.querySelector('textarea')) answer(h.blank.slot, words.blank, words.text);
      return;
    }
    // Kept on the server too, so a reload still offers it
    unsent = unsent.filter(function (u) { return u.key !== words.key; }).concat([{ key: words.key, text: words.text }]);
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
      keep(false);
      return true;
    },
    /** ⏎ on a block, or on the doc for null: keeps its half-typed note, else opens its dot. */
    openAt: function (block) {
      var sp = block ? block.getAttribute('data-sourcepos') : null;
      var u = unsent.filter(function (x) {
        var t = keyTarget(x.key);
        return t && (sp ? t.at.sp === sp : t.at.k === 'doc');
      })[0];
      if (u && writable()) {
        keepUnsent(u);
        return true;
      }
      if (!sp || !state) return false;
      var hit = false;
      (state.items || []).forEach(function (i) {
        var at = i.at && i.at.loc;
        if (at && at.sp === sp && i.seen && settled(i) && !expanded[i.n]) {
          expanded[i.n] = 1;
          hit = true;
        }
      });
      if (hit) reconcile();
      return hit;
    },
    rewriteActive: function () { return !!(state && state.rewrite); },
    /**
     * Puts the keys where a walk stop takes them: a change's receipt, a not-handled row, or a
     * blank. False when the stop has no line of its own.
     */
    focusStop: function (stop) {
      var hit = null;
      hosts.forEach(function (h) {
        var e = h.entry;
        if (hit || !e || !h.host.isConnected) return;
        if ((stop.kind === 'change' && e.change && e.change.id === stop.id) ||
          (stop.kind === 'not_handled' && 'nh:' + e.nh === stop.id) ||
          (stop.kind === 'blank' && e.kind === 'blank' && '#' + e.blank === stop.id)) hit = h;
      });
      if (hit) hit.box.focus({ preventScroll: true });
      return !!hit;
    },
    markRewriteSeen: markRewriteSeen,
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
