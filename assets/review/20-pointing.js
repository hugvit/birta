var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  // Blocks that own quote text, as quote.js finds them
  var OWNERS = 'p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,pre';
  // Only these start review in a tab; the rest act without it
  var ACTIVATES = { focus_next: 1, focus_prev: 1, note: 1, ask: 1, yes: 1, no: 1, later: 1, your_call: 1, next_item: 1, prev_item: 1 };
  var VERDICTS = { yes: 1, no: 1, later: 1, your_call: 1 };
  var ACTIVE_KEY = 'birta-review-active';
  var HINT = 'c note · q ask · x cut';
  var CLAMPED = 'selection clamped to this paragraph';
  var CREATED = 'review file created · z to undo';
  var END = 'no more · j/k to move';
  var REOPEN = 'approved · c to reopen';
  // After approve these only say how to start again
  var REOPENS = { yes: 1, no: 1, later: 1, your_call: 1, ask: 1, next_item: 1, prev_item: 1 };

  var stops = [];
  var focus = null;
  // What the focused block looked like, to find it again when a patch replaces its node
  var focusWas = null;
  var active = false;
  // This page's own acts, newest last; z takes back the top once its ack names its line
  var undos = [];
  var scrollQueued = false;
  // Where n and p last landed, as `[line, rank, order]`, so a server push can't skip a stop
  var cursor = null;
  // The round the cursor walked; a new one starts the walk from the top
  var cursorRound = null;

  // Each stop's element, or the summary of a closed details for those it hides, until the doc changes
  var placed = null;
  var placedAt = -1;
  var placedFor = null;
  var standIns = new Set();
  // The walk's stops for the state they came from
  var walkFor = null;
  var walkStops = [];

  function content() { return document.getElementById('content'); }

  function blockOf(sp) { return BR.blocks.of(sp); }

  function isSummary(el) { return !!el && el.localName === 'summary'; }

  function placedStops() {
    var v = BR.blocks.version();
    if (placed && placedAt === v && placedFor === stops) return placed;
    var out = [];
    standIns = new Set();
    stops.forEach(function (s) {
      var el = blockOf(s.sp);
      if (!el) return;
      var closed = el.parentElement && el.parentElement.closest('details:not([open])');
      if (closed && content().contains(closed)) {
        var summary = closed.querySelector(':scope > summary');
        if (summary && !summary.contains(el)) {
          if (out[out.length - 1] !== summary) out.push(summary);
          standIns.add(summary);
          return;
        }
      }
      out.push(el);
    });
    placed = out;
    placedAt = v;
    placedFor = stops;
    return out;
  }

  /** The focus stops in document order; a closed `<details>` stands in with its summary. */
  function targets() {
    return placedStops().filter(function (el) { return standIns.has(el) || el.getClientRects().length; });
  }

  function walkOf(state) {
    if (state !== walkFor) {
      walkFor = state;
      walkStops = BR.pure.walk(state);
    }
    return walkStops;
  }

  /** The latest round as a key: a new hand-back or claim changes it. */
  function roundOf(state) {
    var r = (state.rounds || [])[(state.rounds || []).length - 1];
    return r ? r.submit + ':' + (r.claim || '') : '';
  }

  function headerBottom() {
    var h = document.querySelector('.file-header');
    return h ? h.getBoundingClientRect().bottom : 0;
  }

  function paintDoc() {
    var name = document.querySelector('.file-header-name');
    if (!name) return;
    if (active && !focus) name.setAttribute('data-review-focus', '');
    else name.removeAttribute('data-review-focus');
  }

  function activate() {
    if (active) return;
    active = true;
    try { sessionStorage.setItem(ACTIVE_KEY, '1'); } catch (e) {}
    paintDoc();
    BR.chrome.render();
  }

  function setFocus(el, all) {
    focus = el;
    focusWas = el && { name: el.localName, text: el.textContent, at: (all || targets()).indexOf(el) };
    BR.lane.focus(el);
    paintDoc();
  }

  function reveal(el) {
    var r = el.getBoundingClientRect();
    var top = headerBottom();
    if (r.top < top) window.scrollBy(0, r.top - top - 8);
    else if (r.bottom > innerHeight) window.scrollBy(0, Math.min(r.bottom - innerHeight + 8, r.top - top - 8));
  }

  function move(step) {
    // A stale selection would outrank the new focus as the next verdict's target
    var s = getSelection();
    if (s && s.rangeCount && !s.isCollapsed) {
      s.removeAllRanges();
      onSelection();
    }
    var all = targets();
    if (!all.length) return;
    var next;
    if (!focus) {
      if (step < 0) return;
      var top = headerBottom() - 1;
      next = all.find(function (el) { return el.getBoundingClientRect().top >= top; }) || all[all.length - 1];
    } else {
      var at = all.indexOf(focus);
      var i = at + step;
      if (at < 0) {
        // Focus sits inside a details that has since closed: count from where it was
        var after = all.findIndex(function (el) {
          return focus.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING;
        });
        if (after < 0) after = all.length;
        i = step > 0 ? after : after - 1;
      }
      if (i < 0) {
        setFocus(null);
        return;
      }
      next = all[Math.min(i, all.length - 1)];
    }
    setFocus(next, all);
    reveal(next);
    see(next);
  }

  // --- seen and the attention walk ---

  /** Sends seen for the unseen changes on `el`; true when there were any. */
  function see(el) {
    var state = BR.currentState();
    var sp = el && el.getAttribute && el.getAttribute('data-sourcepos');
    // The clean view shows no change to have seen
    if (!state || !sp || state.log_writable === false || BR.chrome.clean()) return false;
    var ids = (state.changes || []).filter(function (c) {
      return !c.seen && !c.settled && c.attr !== 'human' && c.range && c.range.sp === sp;
    }).map(function (c) { return c.id; });
    if (ids.length) BR.op({ op: 'seen', ids: ids });
    return ids.length > 0;
  }

  function keyOf(stop) { return [stop.line, stop.rank, stop.order]; }

  function before(a, b) {
    for (var i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i];
    return false;
  }

  /**
   * Focuses a walk stop, marks it seen when it isn't, and pills where it is in the walk;
   * blanks and not-handled rows wait for an answer or y.
   */
  function land(stop, state) {
    var all = walkOf(state);
    cursor = keyOf(stop);
    cursorRound = roundOf(state);
    var el = stop.sp ? blockOf(stop.sp) : null;
    setFocus(el);
    if (el) reveal(el);
    if (stop.unseen && (stop.kind === 'change' || stop.kind === 'reply')) BR.op({ op: 'seen', ids: [stop.id] });
    BR.flow.focusStop(stop);
    showPill((all.indexOf(stop) + 1) + ' of ' + all.length + ' · n next · p back');
  }

  function walk(step) {
    var state = BR.currentState();
    if (!state || state.log_writable === false) return;
    if (cursorRound !== roundOf(state)) cursor = null;
    var all = walkOf(state);
    var next = step > 0
      ? all.filter(function (x) { return !cursor || before(cursor, keyOf(x)); })[0]
      : all.filter(function (x) { return cursor && before(keyOf(x), cursor); }).pop();
    if (next) land(next, state);
    else if (!BR.dir.cross(step)) showPill(END);
  }

  function onClick(e) {
    var t = e.target;
    if (!t || !t.closest || BR.isHost(t) || !content().contains(t)) return;
    // A drag that selects text is pointing, not looking
    var s = getSelection();
    if (s && !s.isCollapsed) return;
    for (var el = t; el && el !== content(); el = el.parentElement) {
      if (el.hasAttribute('data-sourcepos') && see(el)) return;
    }
  }

  function selectedRange() {
    var s = getSelection();
    return s && s.rangeCount && !s.isCollapsed ? s.getRangeAt(0) : null;
  }

  function quoteAnchor(q) {
    var at = { k: 'quote', q: q.quote, pre: q.pre, suf: q.suf };
    if (q.sp) {
      at.sp = q.sp;
      at.off = q.off;
      at.tl = q.tl;
    }
    return at;
  }

  /** A block anchor in words, for a composer's label: a heading as its section, else its text. */
  function named(at) {
    var el = at.sp ? blockOf(at.sp) : null;
    var text = el ? el.textContent.replace(/\s+/g, ' ').trim() : '';
    if (at.k === 'block' && el && /^h[1-6]$/.test(el.localName)) {
      return new Array(Number(el.localName[1]) + 1).join('#') + ' ' + text + ' (section)';
    }
    return '"' + (at.k === 'quote' ? at.q : text).slice(0, 40) + '"';
  }

  /** What a verdict points at: the selection, else the focus, else the whole doc. */
  function target() {
    var range = selectedRange();
    var q = range && BR.quote.fromRange(range);
    if (q) return quoteAnchor(q);
    var block = isSummary(focus) ? summaryStop(focus) : focus;
    if (block) {
      return {
        k: 'block',
        sp: block.getAttribute('data-sourcepos'),
        scope: /^h[1-6]$/.test(block.localName) ? 'section' : 'block'
      };
    }
    return { k: 'doc' };
  }

  /**
   * A summary is raw HTML with no stop of its own, and searching for its words could land on
   * another copy of them: a stop that holds it, else the first stop its details hides.
   */
  function summaryStop(summary) {
    var els = stops.map(function (s) { return blockOf(s.sp); });
    for (var at = summary.parentElement; at && at !== content(); at = at.parentElement) {
      if (els.indexOf(at) >= 0) return at;
    }
    var details = summary.parentElement;
    return els.filter(function (el) { return el && details.contains(el); })[0] || null;
  }

  /** The block that replaced the focused one: same tag and text, nearest its old place. */
  function refind() {
    if (!focusWas) return null;
    var best = null;
    var gap = Infinity;
    targets().forEach(function (el, i) {
      if (el.localName !== focusWas.name || el.textContent !== focusWas.text) return;
      if (Math.abs(i - focusWas.at) < gap) {
        gap = Math.abs(i - focusWas.at);
        best = el;
      }
    });
    return best;
  }

  // --- the pill, which the chrome shows ---

  function showPill(text) { BR.chrome.pill(text); }

  function pillSays(text) { return BR.chrome.pillSays(text); }

  function ownerOf(node) {
    var el = node && (node.nodeType === 1 ? node : node.parentElement);
    var owner = el && el.closest(OWNERS);
    return owner && content().contains(owner) && owner.hasAttribute('data-sourcepos') ? owner : null;
  }

  /** Whether the range selects text outside the block its quote was clamped to. */
  function drops(range, owner) {
    var root = range.commonAncestorContainer;
    var walker = document.createTreeWalker(root.nodeType === 1 ? root : root.parentNode, NodeFilter.SHOW_TEXT);
    for (var n = walker.nextNode(); n; n = walker.nextNode()) {
      if (!range.intersectsNode(n) || ownerOf(n) === owner) continue;
      var text = n.data;
      if (n === range.endContainer) text = text.slice(0, range.endOffset);
      if (n === range.startContainer) text = text.slice(range.startOffset);
      if (text.trim()) return true;
    }
    return false;
  }

  function onSelection() {
    var range = selectedRange();
    var q = range && BR.quote.fromRange(range);
    if (!q) {
      if (pillSays(HINT) || pillSays(CLAMPED)) showPill('');
      return;
    }
    var owner = q.sp ? blockOf(q.sp) : null;
    showPill(q.crossBlock && owner && drops(range, owner) ? CLAMPED : HINT);
  }

  // --- verdicts and z ---

  /** Your draft verdict on `at`: a sent one is never edited, so a key makes a new note. */
  function draftVerdictAt(at) {
    var state = BR.currentState();
    if (!state || at.k === 'quote') return null;
    return (state.items || []).filter(function (i) {
      if (!i.by || i.by.type !== 'human' || !i.verdict || i.status !== 'draft') return false;
      if (at.k === 'doc') return !!i.at && i.at.kind === 'doc';
      return !!i.at && !!i.at.loc && i.at.loc.sp === at.sp && i.at.scope === at.scope;
    })[0] || null;
  }

  function remember(entry) {
    entry.n = null;
    undos.push(entry);
  }

  function verdict(name) {
    var state = BR.currentState();
    var at = target();
    if (at.k === 'doc' && name === 'yes' && BR.flow.rewriteActive()) {
      BR.flow.markRewriteSeen();
      return;
    }
    var own = draftVerdictAt(at);
    if (!own) {
      remember({
        cid: BR.op({ op: 'comment', at: at, verdict: name }),
        created: !state || !state.log || !state.log.lines,
        undo: function (n) { return { op: 'retract', to: n }; }
      });
      return;
    }
    var was = own.verdict;
    if (own.verdict !== name) {
      remember({
        cid: BR.op({ op: 'amend', to: own.n, verdict: name }),
        item: own.n,
        undo: function () { return { op: 'amend', to: own.n, verdict: was }; }
      });
    } else if (own.body) {
      // The same key clears the verdict; the words stay
      remember({
        cid: BR.op({ op: 'amend', to: own.n, drop_verdict: true }),
        item: own.n,
        undo: function () { return { op: 'amend', to: own.n, verdict: was }; }
      });
    } else {
      remember({
        cid: BR.op({ op: 'retract', to: own.n }),
        undo: function () { return { op: 'comment', at: at, verdict: was }; }
      });
    }
  }

  /** Whether the note on line `n` has left the drafts, so z can no longer touch it. */
  function sent(n) {
    var state = BR.currentState();
    var item = state && (state.items || []).filter(function (i) { return i.n === n; })[0];
    return !!item && item.status !== 'draft' && item.status !== 'retracted';
  }

  function takeBack(entry) {
    undos.splice(undos.indexOf(entry), 1);
    var n = entry.item != null ? entry.item : entry.n;
    // What went out with a hand-back stays there; so does everything before it
    if (sent(n)) {
      undos = [];
      return;
    }
    BR.op(entry.undo(entry.n));
    if (pillSays(CREATED)) showPill('');
  }

  function undo() {
    if (BR.chrome.withdrawBatch()) return;
    var hold = BR.flow.holdActive();
    if (hold) {
      BR.op({ op: 'withdraw', to: hold.n });
      return;
    }
    var top = undos[undos.length - 1];
    if (!top) return;
    // Before its ack the line isn't known: take it back once the ack names it
    if (top.n == null) top.queued = true;
    else takeBack(top);
  }

  function onFrame(msg) {
    if (msg.type === 'content') {
      if (msg.path && msg.path !== CURRENT_PATH) return;
      if (msg.stops) stops = msg.stops;
      if (focus && !focus.isConnected) setFocus(refind());
    } else if (msg.type === 'ack' || msg.type === 'nack') {
      var entry = undos.filter(function (u) { return u.cid === msg.cid; })[0];
      if (!entry) return;
      if (msg.type === 'nack') {
        undos.splice(undos.indexOf(entry), 1);
        return;
      }
      entry.n = msg.n;
      if (entry.created) showPill(CREATED);
      if (entry.queued) takeBack(entry);
    }
  }

  // Focus the reader scrolled away from would aim a key at a block they can't see
  function onScroll() {
    if (scrollQueued) return;
    scrollQueued = true;
    requestAnimationFrame(function () {
      scrollQueued = false;
      if (!focus) return;
      var r = focus.getBoundingClientRect();
      if (r.bottom <= headerBottom() || r.top >= innerHeight || !focus.getClientRects().length) setFocus(null);
    });
  }

  BR.pointing = {
    init: function () {
      stops = (BOOT.stops || []).slice();
      try { active = sessionStorage.getItem(ACTIVE_KEY) === '1'; } catch (e) {}
      paintDoc();
      document.addEventListener('selectionchange', onSelection);
      document.addEventListener('click', onClick);
      window.addEventListener('scroll', onScroll, { passive: true });
      birtaBus.on(onFrame);
    },
    /** Runs a review action; false leaves the key to the page. */
    act: function (action) {
      if (ACTIVATES[action]) activate();
      if (action === 'focus_next' || action === 'focus_prev') {
        move(action === 'focus_next' ? 1 : -1);
        return true;
      }
      if (BR.chrome.clean()) {
        if (REOPENS[action]) {
          showPill(REOPEN);
          return true;
        }
        if (action === 'note') {
          var on = target();
          BR.flow.open('note', on, null, 'new round · ' + (on.k === 'doc' ? 'whole doc' : named(on)));
          return true;
        }
      }
      if (action === 'next_item' || action === 'prev_item') {
        walk(action === 'next_item' ? 1 : -1);
        return true;
      }
      if (action === 'next_file' || action === 'prev_file') {
        BR.dir.step(action === 'next_file' ? 1 : -1);
        return true;
      }
      if (action === 'turn') {
        BR.chrome.turn();
        return true;
      }
      if (action === 'hide_marks') {
        BR.lane.toggle();
        return true;
      }
      if (VERDICTS[action]) {
        verdict(action);
        return true;
      }
      if (action === 'undo') {
        undo();
        return true;
      }
      if (action === 'note' || action === 'ask') {
        var at = target();
        BR.flow.open(action, at, action === 'note' ? draftVerdictAt(at) : null);
        return true;
      }
      if (action === 'open') {
        // Enter on a link, summary or button keeps its own meaning
        var active = document.activeElement;
        if (active && active !== document.body && active.closest && active.closest('a, summary, button, input, select, textarea')) return false;
        return !isSummary(focus) && BR.flow.openAt(focus);
      }
      return false;
    },
    /** Shows `text` in the pill, or hides it for ''. */
    pill: showPill,
    /**
     * The approve guard's jump: to the first unseen change or reply in doc order, else the first
     * open question. False when there is neither.
     */
    guard: function () {
      var state = BR.currentState();
      if (!state) return false;
      var all = walkOf(state);
      var first = all.filter(function (x) { return x.unseen && (x.kind === 'change' || x.kind === 'reply'); })[0] ||
        all.filter(function (x) { return x.kind === 'blank' && x.unseen; })[0];
      if (first) land(first, state);
      return !!first;
    },
    /** Lands on this page's first or last walk stop, after n or p crossed files. */
    landAt: function (which) {
      var state = BR.currentState();
      if (!state) return;
      var all = walkOf(state);
      var stop = which === 'last' ? all[all.length - 1] : all[0];
      if (stop) land(stop, state);
    },
    /** Puts an act of this page's on the z stack: `{cid, undo(n) -> op fields, item?, created?}`. */
    remember: remember,
    /** Drops one Esc layer: the phrase menu, the composer, the selection, then the focus. False when none is there. */
    escape: function () {
      if (BR.chrome.escape()) return true;
      if (BR.flow.escape()) return true;
      var s = getSelection();
      if (s && s.rangeCount && !s.isCollapsed) {
        s.removeAllRanges();
        onSelection();
        return true;
      }
      if (focus) {
        setFocus(null);
        return true;
      }
      return false;
    }
  };
})();
