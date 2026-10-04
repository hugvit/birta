var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  // Blocks that own quote text, as quote.js finds them
  var OWNERS = 'p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,pre';
  // Moving keys leave a picked link behind
  var MOVES = { focus_next: 1, focus_prev: 1, focus_first: 1, focus_last: 1, next_item: 1, prev_item: 1 };
  // Only these start review in a tab; the rest act without it
  var ACTIVATES = { focus_next: 1, focus_prev: 1, focus_first: 1, focus_last: 1, note: 1, ask: 1, yes: 1, no: 1, later: 1,
    your_call: 1, next_item: 1, prev_item: 1 };
  var VERDICTS = { yes: 1, no: 1, later: 1, your_call: 1 };
  var ACTIVE_KEY = 'birta-review-active';
  // The stop this tab had on a path, which a reload of that path opens on
  var FOCUS_KEY = 'birta-focus:';
  var CLAMPED = 'selection clamped to this paragraph';
  // Before review shows, a mode's way back is a plain message, as the strip names it once review does
  var SOURCE_SAYS = 'source view · esc back';
  var READING_SAYS = 'reading mode · esc back';
  var NOTHING = 'nothing to undo';
  var WITHDRAWN = 'hand-back withdrawn · your notes are back';
  var APPROVE_WITHDRAWN = 'approve withdrawn';
  var WITH_CLAUDE = 'already with Claude · c adds a note';
  var UNDO_KEY = 'birta-undo:';
  var TOP_OF_DOC = 'top of the doc';
  var END_OF_DOC = 'end of the doc';
  var REOPEN = 'approved · c to reopen';
  // After approve these only say how to start again
  var REOPENS = { yes: 1, no: 1, later: 1, your_call: 1, revert_change: 1, next_item: 1, prev_item: 1 };
  // What a change row takes before the block under it does
  var ROW_KEYS = { yes: 1, no: 1, note: 1, revert_change: 1 };
  var WORD = { yes: 'yes', no: 'no', later: 'later', your_call: 'your call' };
  var LABEL_WORDS = 6;
  // Keys the browser scrolls the page with when no field or control takes them
  var SCROLL_KEYS = { ' ': 1, PageUp: 1, PageDown: 1, ArrowUp: 1, ArrowDown: 1, ArrowLeft: 1, ArrowRight: 1 };
  // A scroll this soon after your wheel, touch or scroll key is your own
  var USER_MS = 200;
  // A smooth scroll's later frames stay yours while they keep coming this close, up to USER_MAX_MS after the input
  var USER_GAP_MS = 100;
  var USER_MAX_MS = 1000;

  /** The title's sp: a first stop that is an H1 whose section holds every later stop. Null otherwise. */
  BR.pure.titleStop = function (stops) {
    var first = stops && stops[0];
    if (!first || first.kind !== 'heading' || first.level !== 1 || !first.section) return null;
    for (var i = 1; i < stops.length; i++) if (stops[i].lines[0] > first.section[1]) return null;
    return first.sp;
  };

  /**
   * What a verdict key at the whole-doc stop says instead of deciding. `b` is the turn button;
   * `keys` names the bound move and note keys (`j` and `c` by default).
   */
  BR.pure.wholeDocPill = function (action, b, mac, keys) {
    var j = (keys && keys.focus_next) || 'j';
    var c = (keys && keys.note) || 'c';
    if (action === 'revert_change') return 'nothing here to revert';
    if (action === 'later') return 'later needs a block · ' + j;
    if (action === 'your_call') return 'your call needs a block · ' + j;
    if (action === 'no') return 'whole doc: ' + c + " to say what's wrong";
    if (!b) return null;
    var key = mac ? '⌘⏎' : 'Ctrl⏎';
    if (/^Hand back/.test(b.text)) return key + ' hands back';
    if (/^Approve/.test(b.text) && !b.disabled) return key + ' approves';
    if (/^Approved/.test(b.text)) return 'approved · ' + c + ' to reopen';
    return b.text.charAt(0).toLowerCase() + b.text.slice(1);
  };

  /**
   * Where j, k, Home and End take the focus among `count` stops from stop `at`: `step` is 1, -1,
   * "first" or "last". Nothing wraps; `says` names the end a step ran into, else null.
   */
  BR.pure.focusStep = function (at, count, step) {
    if (!count) return { to: -1, says: null };
    if (step === 'first') return { to: 0, says: null };
    if (step === 'last') return { to: count - 1, says: null };
    var to = at + step;
    if (to < 0) return { to: 0, says: TOP_OF_DOC };
    if (to > count - 1) return { to: count - 1, says: END_OF_DOC };
    return { to: to, says: null };
  };

  /** Up to six words of `text`, with … only when it was cut. */
  BR.pure.fewWords = function (text) {
    var words = String(text || '').trim().split(/\s+/).filter(Boolean);
    return words.length > LABEL_WORDS ? words.slice(0, LABEL_WORDS).join(' ') + '…' : words.join(' ');
  };

  var stops = [];
  var focus = null;
  // The whole doc stop, above the first block
  var docStop = null;
  // A click landed outside the focused block since it took focus
  var pointedAway = false;
  // The link Tab picked inside the focused block; ⏎ follows it natively while it has DOM focus
  var picked = null;
  // What the focused block looked like, to find it again when a patch replaces its node
  var focusWas = null;
  // The stops as they were when focus last settled, to find what comes after it once it goes
  var focusList = null;
  var active = false;
  // This tab's own acts on this path, newest last, as plain data kept across a reload; z takes back the top once its line is known
  var undos = [];
  // Entries a reload brought back, unchecked until the first state shows what was sent since
  var unchecked = false;
  // z pressed before that state came
  var waitingZ = 0;
  var scrollQueued = false;
  // The walk's place, `{lo, hi}` as `[line, rank, order]`: n goes on past hi and p back before lo.
  // Null is before the first stop, where a tab opens and Claude's reply puts it back
  var cursor = null;
  // The round and reply the place was set in; a new one puts it back before the first stop
  var cursorRound = null;
  // Focus moved since the walk last landed, by anything but the page: the place is at the focus
  var movedSince = false;

  // Each stop's element, or the summary of a closed details for those it hides, until the doc changes
  var placed = null;
  var placedAt = -1;
  var placedFor = null;
  var standIns = new Set();
  // The walk's stops for the state they came from
  var walkFor = null;
  var walkFolder = 0;
  var walkStops = [];
  // When your last wheel, touch or scroll key came, and the last scroll counted as yours
  var userAt = -Infinity;
  var userLast = -Infinity;
  var userScroll = false;
  // The stop your scroll moved focus to; while that scroll goes on, focus stays on the first stop in view
  var scrolledTo = null;
  // The stops there when that scroll first moved focus; a stop the page adds meanwhile never takes it
  var scrolledFrom = null;
  // The focus's place on screen before the page changed under it, put back once the change is done
  var pinned = null;
  // The focus held in place after a page-made change while late layout settles, until any other scroll
  var kept = null;
  var settleQueued = false;
  // A frame changed the stops, so the list focus was found in is taken again
  var relist = false;
  // The line a focused block Claude cut started on, until the state that names its removal comes
  var cutLine = null;
  // A review state came since the last settle
  var stateCame = false;

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

  /** The focus stops in document order, whole doc first; a closed `<details>` stands in with its summary. */
  function targets() {
    var out = placedStops().filter(function (el) { return standIns.has(el) || el.getClientRects().length; });
    // A blank is a stop of its own, right where it sits, and so is a removal, in its gap
    var hosts = BR.flow.blanks().concat(BR.flow.removals());
    if (hosts.length) {
      out = out.concat(hosts).sort(function (a, b) {
        return a === b ? 0 : a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1;
      });
    }
    return [docStop].concat(out);
  }

  /** Whether `el` is the title: an H1 whose section spans the doc, an ordinary block whose verdicts are on its line. */
  function isTitle(el) {
    var sp = BR.pure.titleStop(stops);
    return !!sp && !!el && el === blockOf(sp);
  }

  /** Where a stop sits on screen; whole doc's is its rows and line, null while none of them shows. */
  function rectOf(el) {
    if (el === docStop) return BR.flow.docRect();
    return el.getClientRects().length ? el.getBoundingClientRect() : null;
  }

  /** The walk for `state`, in the order n and p step through it. */
  function walkOf(state) {
    var folder = BR.flow.folderNotes();
    // A listing can bring folder notes with no new state
    if (state !== walkFor || folder !== walkFolder) {
      walkFor = state;
      walkFolder = folder;
      walkStops = BR.pure.walk(state, { folder: folder }).slice().sort(function (a, b) {
        return before(keyOf(a), keyOf(b)) ? -1 : before(keyOf(b), keyOf(a)) ? 1 : 0;
      });
    }
    return walkStops;
  }

  /** The latest round and reply as a key: a new hand-back, claim or reply changes it. */
  function roundOf(state) {
    var r = (state.rounds || [])[(state.rounds || []).length - 1];
    var replied = state.phrase && state.phrase.replied ? state.phrase.replied.turn : '';
    return (r ? r.submit + ':' + (r.claim || '') : '') + '|' + replied;
  }

  /** A new round or Claude's reply puts the walk's place back before its first stop. */
  function checkRound() {
    var state = BR.currentState();
    if (!state) return;
    var r = roundOf(state);
    if (cursorRound !== null && r !== cursorRound) {
      cursor = null;
      movedSince = false;
    }
    cursorRound = r;
  }

  /** The view's bottom: the strip's top while it shows. */
  function viewBottom() { return BR.chrome.stripTop(); }

  function headerBottom() {
    var h = document.querySelector('.file-header');
    return h ? h.getBoundingClientRect().bottom : 0;
  }

  function activate() {
    if (active) return;
    active = true;
    try { sessionStorage.setItem(ACTIVE_KEY, '1'); } catch (e) {}
    BR.chrome.render();
  }

  /** The block's links a reader can follow; a heading's anchor icon is not one of them. */
  function linksIn(block) {
    return Array.prototype.filter.call(block.querySelectorAll('a[href]'), function (a) {
      return !a.classList.contains('anchor') && a.getClientRects().length > 0;
    });
  }

  function unpick() {
    var a = picked;
    if (!a) return false;
    picked = null;
    a.removeAttribute('data-br-pick');
    if (document.activeElement === a) a.blur();
    return true;
  }

  function pick(a) {
    unpick();
    picked = a;
    a.setAttribute('data-br-pick', '');
    a.focus();
  }

  /** Tab and Shift+Tab: steps the pick through the focused block's links. False past either end, where the browser's Tab goes on. */
  function tab(step) {
    var links = focus && !isSummary(focus) ? linksIn(focus) : [];
    var at = links.indexOf(picked);
    var next = at < 0 ? (step > 0 ? 0 : links.length - 1) : at + step;
    if (next < 0 || next >= links.length) return false;
    pick(links[next]);
    return true;
  }

  // Focus leaving the picked link, by a click or the browser, ends the pick
  function onFocusOut(e) {
    if (picked && e.target === picked) unpick();
  }

  // The browser's own Tab out of a block with no links: the block focus follows it, and the link is the pick
  function onFocusIn(e) {
    var a = e.target;
    if (!focus || a === picked || !a || a.localName !== 'a' || !content().contains(a)) return;
    // A click that focuses a link moves no focus; only the keyboard's Tab does
    try {
      if (!a.matches(':focus-visible')) return;
    } catch (err) {}
    var stop = stopAt(a);
    if (!stop || isSummary(stop) || linksIn(stop).indexOf(a) < 0) return;
    if (stop !== focus) setFocus(stop);
    picked = a;
    a.setAttribute('data-br-pick', '');
  }

  /** Moves the focus to `el`; `byPage` is a move the reader didn't make, which leaves the walk's place. */
  function setFocus(el, all, byPage) {
    if (!byPage) {
      movedSince = true;
      // Your own move outranks a cut block's hand-off still waiting for its state
      cutLine = null;
    }
    unpick();
    // A doc link the browser focused would take ⏎ from the block the keys are on now
    var held = document.activeElement;
    if (held && held.localName === 'a' && content().contains(held) && !(el && el.contains(held))) held.blur();
    // A row or blank left behind would keep the keys
    if (document.activeElement !== el) BR.flow.release(document.activeElement);
    focus = el;
    pointedAway = false;
    focusList = el ? all || targets() : null;
    focusWas = el && describe(el, focusList);
    // Its line shows before anything measures it
    BR.flow.focusDoc(el === docStop);
    BR.lane.focus(el);
    BR.flow.focusBlock(el === docStop ? null : el);
    keepStop(el);
    if (!byPage) BR.flow.tipDot(el);
    BR.chrome.renderStrip();
  }

  /** Remembers the focused block for a reload of this tab on this path; whole doc and stops with no sp keep nothing. */
  function keepStop(el) {
    var sp = el && el !== docStop && el.getAttribute('data-sourcepos');
    try {
      if (sp) sessionStorage.setItem(FOCUS_KEY + CURRENT_PATH, sp);
      else sessionStorage.removeItem(FOCUS_KEY + CURRENT_PATH);
    } catch (e) {}
  }

  /** The block a reload of this path had focus on, or null when this load is no reload or it is gone. */
  function restored() {
    var nav = performance.getEntriesByType && performance.getEntriesByType('navigation')[0];
    if (!nav || nav.type !== 'reload') return null;
    var sp = null;
    try { sp = sessionStorage.getItem(FOCUS_KEY + CURRENT_PATH); } catch (e) {}
    var el = sp ? blockOf(sp) : null;
    return el && targets().indexOf(el) >= 0 ? el : null;
  }

  /** What the focused stop looked like, to find it, or what replaced it, once a patch has gone through. */
  function describe(el, list) {
    var w = { name: el.localName, text: el.textContent, sp: el.getAttribute('data-sourcepos'), at: list.indexOf(el) };
    if (BR.isHost(el) && el !== docStop) {
      w.item = BR.flow.blankItem(el);
      w.owner = blockUnder(el, list);
    }
    return w;
  }

  /** The stop a blank sits under, or null when it sits above the first block. */
  function blockUnder(host, list) {
    var under = host.parentElement && host.parentElement.localName === 'li' ? host.parentElement : host.previousElementSibling;
    while (under && BR.isHost(under)) under = under.previousElementSibling;
    if (!under || !content().contains(under)) return null;
    if (list.indexOf(under) >= 0) return under;
    // A table or a closed details holds the stop the blank is under
    var hit = null;
    list.forEach(function (el) { if (!BR.isHost(el) && under.contains(el)) hit = el; });
    return hit;
  }

  function reveal(el) {
    // Its own scrolls are the page's, never yours
    userAt = userLast = -Infinity;
    var r = rectOf(el);
    if (!r) return;
    var top = headerBottom();
    var bottom = viewBottom();
    if (r.top < top) window.scrollBy(0, r.top - top - 8);
    else if (r.bottom > bottom) window.scrollBy(0, Math.min(r.bottom - bottom + 8, r.top - top - 8));
    // The place to keep is the revealed one, not where the stop sat before
    var pr = pinned && pinned.el && pinned.el.isConnected ? rectOf(pinned.el) : null;
    if (pr) pinned.top = pr.top;
    var kr = kept && kept.el.isConnected ? rectOf(kept.el) : null;
    if (kr) kept = { el: kept.el, top: kr.top, y: window.scrollY };
  }

  // A stale selection would outrank the new focus as the next verdict's target
  function dropSelection() {
    var s = getSelection();
    if (s && s.rangeCount && !s.isCollapsed) {
      s.removeAllRanges();
      onSelection();
    }
  }

  function go(next, all) {
    setFocus(next, all);
    reveal(next);
    if (BR.isHost(next)) BR.flow.hold(next);
  }

  function move(step) {
    dropSelection();
    var all = targets();
    var at = all.indexOf(focus);
    if (at < 0) {
      // Focus sits inside a details that has since closed: count from where it was
      var after = all.findIndex(function (el) {
        return focus.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING;
      });
      if (after < 0) after = all.length;
      at = step > 0 ? after - 1 : after;
    }
    var r = BR.pure.focusStep(at, all.length, step);
    if (r.says) say(r.says);
    var next = all[r.to];
    if (next === focus) return;
    go(next, all);
  }

  /** Home and End: the first or the last stop, brought into view. */
  function jump(which) {
    dropSelection();
    var all = targets();
    var next = all[BR.pure.focusStep(-1, all.length, which).to];
    if (next === focus) reveal(next);
    else go(next, all);
  }

  // --- the walk, and checking what Claude did ---

  function keyOf(stop) { return [stop.line, stop.rank, stop.order]; }

  /** The focused stop as the walk names it: `doc`, `b:<sp>`, `r:<removal id>` or `q:<id>`. */
  function focusStopKey() {
    var el = isSummary(focus) ? summaryStop(focus) || focus : focus;
    if (!el || el === docStop) return 'doc';
    if (BR.isHost(el)) {
      var n = BR.flow.blankItem(el);
      if (n != null) return 'q:#' + n;
      var gap = BR.flow.removalOf(el);
      return gap ? 'r:' + gap.change : 'doc';
    }
    return 'b:' + el.getAttribute('data-sourcepos');
  }

  /** Claude's news on stop `key`, else the focused one; a question's left out: its field takes the keys. */
  function newsHere(state, key) {
    key = key || focusStopKey();
    return walkOf(state).filter(function (x) { return BR.pure.isNews(x.kind) && x.kind !== 'question' && x.stop === key; });
  }

  /**
   * y or u on the focused stop's news. y checks everything on it in one seen, and nothing goes to
   * Claude; u reverts each change of Claude's there that it can. `key` names the stop when a row,
   * not the stop, holds the keyboard. False leaves y to the page.
   */
  function checkStop(action, key) {
    var state = BR.currentState();
    if (!state || state.log_writable === false) return false;
    var here = newsHere(state, key);
    var changes = here.filter(function (x) { return x.kind === 'change' || x.kind === 'removal'; });
    var ask = BR.keyName('note') + ' to ask Claude';
    if (action === 'revert_change') {
      if (!changes.length) say('nothing here to revert');
      else if (changes.every(function (x) { return x.check === 'reverted'; })) say('already reverted · ' + ask);
      else BR.flow.revertAll(changes.filter(function (x) { return x.check !== 'reverted'; }).map(function (x) { return x.id; }));
      return true;
    }
    if (!here.length) return false;
    var open = here.filter(function (x) { return x.check == null; });
    if (!open.length) {
      if (changes.some(function (x) { return x.check === 'reverted'; })) say('already reverted · ' + ask);
      else if (changes.length) say('already kept · ' + BR.keyName('revert_change') + ' reverts it');
      else say(okOf(here));
      return true;
    }
    BR.op({ op: 'seen', ids: open.map(function (x) { return x.id; }) });
    if (open.some(function (x) { return x.kind === 'change' || x.kind === 'removal'; })) {
      // A keep can't be taken back yet, so z stops at it rather than reaching past it
      remember({ marker: 'nothing to undo · a keep stays kept' });
      say('kept');
    } else say(okOf(here));
    return true;
  }

  /** What y on Claude's replies says: whole doc's are its replies, an answer to your question is its answer. */
  function okOf(here) {
    if (here[0].stop === 'doc') return "ok · Claude's replies";
    return here.every(function (x) { return x.kind === 'answer'; }) ? "ok · Claude's answer" : "ok · Claude's reply";
  }

  /** The stop a walk entry sits on: a question and a removal are their own; the rest share their block, or whole doc. */
  function stopOf(x) { return x.stop; }

  function before(a, b) {
    for (var i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i];
    return false;
  }

  /** Focuses the stop walk entry `stop` sits on, the whole stop at once; landing checks nothing. */
  function land(stop, state) {
    checkRound();
    var here = walkOf(state).filter(function (x) { return stopOf(x) === stopOf(stop); });
    cursor = { lo: keyOf(here[0]), hi: keyOf(here[here.length - 1]) };
    var key = stopOf(stop);
    var el = key.indexOf('r:') === 0 ? BR.flow.removalStop(key.slice(2)) : key.indexOf('b:') === 0 ? blockOf(key.slice(2))
      : stop.sp ? blockOf(stop.sp) : docStop;
    if (el) {
      setFocus(el, null, true);
      reveal(el);
    }
    var host = BR.flow.focusStop(stop);
    // A question is the stop itself, so j and k go on from it
    if (stop.kind === 'question' && host) {
      setFocus(host, null, true);
      reveal(host);
    }
    movedSince = false;
  }

  /**
   * The walk's place at the focused stop, as if n had landed there: n goes on past the stop's
   * walk entries and p back before them; a stop with none sits between its neighbours by line.
   */
  function placeAtFocus(all) {
    var el = isSummary(focus) ? summaryStop(focus) || focus : focus;
    if (!el) return cursor;
    var mine;
    var line;
    var blank = el !== docStop && BR.isHost(el) ? BR.flow.blankItem(el) : null;
    var gap = el !== docStop ? BR.flow.removalOf(el) : null;
    if (el === docStop) {
      mine = all.filter(function (x) { return x.stop === 'doc'; });
      line = 0;
    } else if (gap) {
      mine = all.filter(function (x) { return x.stop === 'r:' + gap.change; });
      line = gap.after ? lineOf(gap.after) : 0;
      if (!mine.length) return { lo: [line, 5, -Infinity], hi: [line, 5, Infinity] };
    } else if (blank != null) {
      mine = all.filter(function (x) { return x.stop === 'q:#' + blank; });
      var owner = focusWas && focusWas.owner;
      var sp = owner && owner.getAttribute('data-sourcepos');
      line = sp ? lineOf(sp) : 0;
      if (!mine.length) return { lo: [line, 2, -Infinity], hi: [line, 2, Infinity] };
    } else {
      var at = el.getAttribute('data-sourcepos');
      mine = all.filter(function (x) { return x.stop === 'b:' + at; });
      line = at ? lineOf(at) : 0;
    }
    if (mine.length) return { lo: keyOf(mine[0]), hi: keyOf(mine[mine.length - 1]) };
    // A question under the block comes after it
    return { lo: [line, -Infinity, -Infinity], hi: [line, el === docStop ? Infinity : 1.5, Infinity] };
  }

  /** n and p: the next or previous stop holding Claude's news or your unsent note, from the walk's place. */
  function walk(step) {
    var state = BR.currentState();
    if (!state || state.log_writable === false) return;
    checkRound();
    var all = walkOf(state);
    if (movedSince) {
      cursor = placeAtFocus(all);
      movedSince = false;
    }
    var next = step > 0
      ? all.filter(function (x) { return !cursor || before(cursor.hi, keyOf(x)); })[0]
      : all.filter(function (x) { return cursor && before(keyOf(x), cursor.lo); }).pop();
    // A stop is landed on as a whole, at its first entry, whichever way the walk came
    if (next) land(all.filter(function (x) { return stopOf(x) === stopOf(next); })[0], state);
    else say('no more · ' + BR.keyName('prev_item') + ' back');
  }

  /** Whether review is under way in this tab: started here, or a review file the state shows. */
  function engaged() {
    var s = BR.currentState();
    return active || !!(s && s.log && s.log.lines > 0);
  }

  /** The innermost focus stop holding `node`, or null. */
  function stopAt(node) {
    var hit = null;
    targets().forEach(function (el) { if (el.contains(node)) hit = el; });
    return hit;
  }

  function onClick(e) {
    var t = e.target;
    if (!t || !t.closest || BR.isHost(t) || !content().contains(t)) return;
    // A click elsewhere in the doc shuts the focused block's row
    if (focus && !focus.contains(t)) pointedAway = true;
    // A drag that selects text is pointing, not looking
    var s = getSelection();
    if (s && !s.isCollapsed) return;
    // A click on a block focuses it, as j would, so a mouse opens its rows; links and controls keep their own click
    var stop = engaged() && !t.closest('a, button, input, select, textarea, summary, label') ? stopAt(t) : null;
    if (stop && stop !== focus) setFocus(stop);
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

  /** What a verdict points at: the selection, else the focus, else the whole doc. */
  function target() {
    var range = selectedRange();
    var q = range && BR.quote.fromRange(range);
    if (q) return quoteAnchor(q);
    var block = isSummary(focus) ? summaryStop(focus) : focus;
    if (block && BR.isHost(block)) block = null;
    if (block) {
      return {
        k: 'block',
        sp: block.getAttribute('data-sourcepos'),
        scope: /^h[1-6]$/.test(block.localName) && !isTitle(block) ? 'section' : 'block'
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

  /**
   * The block that replaced the focused one: same tag and text, nearest its old place; else, as
   * when a revert rewrote it, the same tag starting on the same line.
   */
  function refind(all) {
    // A blank is the same host or gone; its words are its shadow's, so none of them match
    if (!focusWas || focusWas.item !== undefined) return null;
    var best = null;
    var gap = Infinity;
    all.forEach(function (el, i) {
      if (el.localName !== focusWas.name || el.textContent !== focusWas.text) return;
      if (Math.abs(i - focusWas.at) < gap) {
        gap = Math.abs(i - focusWas.at);
        best = el;
      }
    });
    if (best || !focusWas.sp) return best && { el: best, exact: true };
    var line = lineOf(focusWas.sp);
    var at = all.filter(function (el) {
      var sp = el.getAttribute('data-sourcepos');
      return el.localName === focusWas.name && !!sp && lineOf(sp) === line;
    })[0];
    return at ? { el: at, exact: false } : null;
  }

  function lineOf(sp) { return parseInt(sp, 10); }

  // --- messages, which the strip shows ---

  function say(text) { BR.chrome.say(text); }

  function says(text) { return BR.chrome.says(text); }

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
      if (says(CLAMPED)) say('');
      BR.chrome.renderStrip();
      return;
    }
    var owner = q.sp ? blockOf(q.sp) : null;
    // The strip's keys say what a key does to the selection; only a clamp is news
    if (q.crossBlock && owner && drops(range, owner)) say(CLAMPED);
    else if (says(CLAMPED)) say('');
    BR.chrome.renderStrip();
    // A selection outranks the stop: the block its quote is in takes the focus
    var stop = owner ? stopAt(owner) : null;
    if (stop && stop !== focus) setFocus(stop);
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

  /**
   * Puts an act on the z history: `{cid, undo, label, item?, n?}`, where `undo` is the op that takes
   * it back (a `to` of null names the act's own line), or `{marker}` for an act z can't take back.
   */
  function remember(entry) {
    if (entry.n === undefined) entry.n = null;
    undos.push(entry);
    keepUndos();
  }

  function undoKey() { return UNDO_KEY + (birtaBus.tab ? birtaBus.tab() : '') + ':' + CURRENT_PATH; }

  function keepUndos() {
    try {
      // A list that can't be written must not come back stale
      sessionStorage.removeItem(undoKey());
      sessionStorage.setItem(undoKey(), JSON.stringify(undos.map(function (u) {
        var o = {};
        Object.keys(u).forEach(function (k) { if (k !== 'queued') o[k] = u[k]; });
        return o;
      })));
    } catch (e) {}
  }

  function restoreUndos() {
    var got = null;
    try { got = JSON.parse(sessionStorage.getItem(undoKey()) || 'null'); } catch (e) {}
    // An entry of another shape, e.g. from another build, is no act z can take back
    undos = Array.isArray(got) ? got.filter(function (u) {
      return !!u && typeof u === 'object' && (typeof u.marker === 'string' || u.kind === 'handback' || (!!u.undo && typeof u.undo.op === 'string'));
    }) : [];
    unchecked = undos.length > 0;
  }

  /** What a verdict on `at` is on, as its message names it. */
  function scopeLabel(at) {
    if (at.k === 'quote') return '"' + BR.pure.fewWords(at.q) + '"';
    var el = at.sp ? blockOf(at.sp) : null;
    return el ? BR.flow.blockLabel(el, true) : 'this block';
  }

  /** What a verdict's take-back names: `"no" removed from ## Stacked on`, with no kind after it. */
  function verdictGone(name, at) { return '"' + WORD[name] + '" removed from ' + bareScope(at); }

  function verdictBack(name, at) { return '"' + WORD[name] + '" back on ' + bareScope(at); }

  function bareScope(at) { return scopeLabel(at).replace(/ \([^)]*\)$/, ''); }

  function verdict(name) {
    var at = target();
    if (at.k === 'doc') {
      // A whole-doc verdict asks the agent nothing it can act on
      var keys = { focus_next: BR.keyName('focus_next'), note: BR.keyName('note') };
      say(BR.pure.wholeDocPill(name, BR.chrome.button(), /Mac/.test(navigator.platform), keys) || '');
      return;
    }
    verdictOn(name, at);
  }

  /** Writes verdict `name` on `at`, or changes or clears your draft one there, and says what it did. */
  function verdictOn(name, at) {
    var own = draftVerdictAt(at);
    if (!own) {
      remember({
        kind: 'verdict',
        cid: BR.op({ op: 'comment', at: at, verdict: name }),
        undo: { op: 'retract', to: null },
        label: verdictGone(name, at)
      });
      say(WORD[name] + ' · ' + scopeLabel(at) + ' · ' + BR.keyName('undo') + ' to undo');
      return;
    }
    var was = own.verdict;
    if (own.verdict !== name) {
      remember({
        kind: 'verdict',
        cid: BR.op({ op: 'amend', to: own.n, verdict: name }),
        item: own.n,
        itemCid: own.cid,
        undo: { op: 'amend', to: own.n, verdict: was },
        label: verdictGone(name, at)
      });
      say(WORD[name] + ' · ' + scopeLabel(at) + ' · ' + BR.keyName('undo') + ' to undo');
      return;
    }
    if (own.body) {
      // The same key clears the verdict; the words stay
      remember({
        kind: 'verdict',
        cid: BR.op({ op: 'amend', to: own.n, drop_verdict: true }),
        item: own.n,
        itemCid: own.cid,
        undo: { op: 'amend', to: own.n, verdict: was },
        label: verdictBack(was, at)
      });
    } else {
      remember({
        kind: 'verdict',
        cid: BR.op({ op: 'retract', to: own.n }),
        undo: { op: 'comment', at: at, verdict: was },
        label: verdictBack(was, at)
      });
    }
    say(WORD[name] + ' removed · ' + BR.keyName('undo') + ' to undo');
  }

  /** The newest of your drafts on `at`'s block, or on the doc for a doc target; null for none. */
  function newestDraft(at) {
    var state = BR.currentState();
    if (!state) return null;
    var mine = (state.items || []).filter(function (i) {
      if (!i.by || i.by.type !== 'human' || i.status !== 'draft' || !i.at) return false;
      if (at.k === 'doc') return i.at.kind === 'doc';
      return i.at.kind !== 'doc' && !!i.at.loc && i.at.loc.sp === at.sp;
    });
    return mine.reduce(function (a, i) { return !a || i.n > a.n ? i : a; }, null);
  }

  /** Your draft quote note whose words hold the selection `q`, newest first; null for none. */
  function draftHolding(q) {
    var state = BR.currentState();
    if (!state || !q.sp || q.off == null) return null;
    var end = q.off + Array.from(q.quote).length;
    var mine = (state.items || []).filter(function (i) {
      var loc = i.at && i.at.loc;
      return !!i.by && i.by.type === 'human' && i.status === 'draft' && !!i.at && i.at.scope === 'quote' &&
        !!loc && loc.sp === q.sp && !!loc.off && q.off >= loc.off[0] && end <= loc.off[1];
    });
    return mine.reduce(function (a, i) { return !a || i.n > a.n ? i : a; }, null);
  }

  /** c: your draft under the selection, else a new quote note; else your newest draft on the focus. */
  function note() {
    var range = selectedRange();
    var q = range && BR.quote.fromRange(range);
    if (q) {
      BR.flow.open('note', quoteAnchor(q), draftHolding(q));
      return;
    }
    var at = target();
    BR.flow.open('note', at, newestDraft(at));
  }

  // --- The viewer's modes: source view and reading mode ---

  // Whether focus last moved by Tab rather than by the pointer
  var byKeys = false;

  function rawShown() { return typeof viewMode !== 'undefined' && viewMode === 'raw'; }
  function readingShown() { return typeof isReadingMode !== 'undefined' && isReadingMode === true; }

  /** Leaves reading mode, then the source view, so a key acts on the rendered doc. */
  function leaveModes() {
    var view = window.birtaView;
    if (!view) return;
    // Reading mode puts back the source view it was entered from, so it goes first
    if (readingShown()) view.toggleReadingMode(false);
    if (rawShown()) view.setViewMode('preview');
  }

  // Review follows the view, never the key, so the Source button and ⌥u keep focus alike
  function onView(e) {
    var d = e.detail || {};
    if (!BR.chrome.shown() && (d.reading || d.mode === 'raw')) say(d.reading ? READING_SAYS : SOURCE_SAYS);
    else if (says(SOURCE_SAYS) || says(READING_SAYS)) say('');
    BR.chrome.renderStrip();
    if (d.reading || d.mode === 'raw') return;
    BR.lane.redraw();
    if (!focus || !focus.isConnected) return;
    BR.lane.focus(focus);
    reveal(focus);
  }

  /** A header control holding DOM focus: the page's buttons, or review's phrase and turn button. */
  function pageControl() {
    var t = document.activeElement;
    if (!t || t === document.body || content().contains(t)) return null;
    // Reading mode hides the header, and a hidden control is no place to be
    if (readingShown() && t.closest('.header, .file-header')) return null;
    if (BR.isHost(t)) return t.closest('.file-header, .header') ? t : null;
    return t.closest('button, [role="button"], [role="menu"], .theme-dropdown') ? t : null;
  }

  // Heading anchors show only on hover, so Tab never stops on one
  function untabAnchors() {
    content().querySelectorAll('a.anchor:not([tabindex])').forEach(function (a) { a.setAttribute('tabindex', '-1'); });
  }

  /** Whether the note on line `n` has left the drafts, so z can no longer touch it. */
  function sent(n) {
    var state = BR.currentState();
    var item = state && (state.items || []).filter(function (i) { return i.n === n; })[0];
    // A reply is no item, but a hand-back after it took it all the same
    var handed = !!state && (state.rounds || []).some(function (r) { return r.submit > n; });
    return handed || (!!item && item.status !== 'draft' && item.status !== 'retracted');
  }

  /** The op that takes `entry` back: its own line where the op names none. */
  function undoOp(entry) {
    var f = {};
    Object.keys(entry.undo).forEach(function (k) { f[k] = entry.undo[k]; });
    if (f.to === null) f.to = entry.n;
    return f;
  }

  function takeBack(entry) {
    undos.splice(undos.indexOf(entry), 1);
    var n = entry.item != null ? entry.item : entry.n;
    // What went out with a hand-back stays there; so does everything before it. A folder note's line is
    // in the folder's file, which this doc's state doesn't hold: the server refuses one a hand-back took
    if (!entry.folder && sent(n)) {
      undos = [];
      keepUndos();
      say(nothingLeft());
      return;
    }
    BR.op(undoOp(entry));
    keepUndos();
    say(entry.label || NOTHING);
  }

  /** What z says with nothing of yours left to take back: while Claude works, that the hand-back is with it. */
  function nothingLeft() {
    var state = BR.currentState();
    if (state && state.phrase && state.phrase.stage === 'picked_up') return WITH_CLAUDE.replace(/^(.*· )c /, '$1' + BR.keyName('note') + ' ');
    return NOTHING;
  }

  /** Whether `state` holds your reply on line `n`. */
  function replyAt(state, n) {
    return (state.items || []).some(function (i) {
      return (i.replies || []).some(function (r) { return r.n === n && !!r.by && r.by.type === 'human'; });
    });
  }

  /**
   * Whether a reload's entry still names what this tab wrote: its own line holds the op with its
   * cid, and the note it edits is the one it had, so a log reset in between never takes back another line.
   */
  function stillOurs(state, u) {
    if (u.marker || u.kind === 'handback' || u.folder || u.n == null) return true;
    if (u.undo && u.undo.to === null) return u.kind === 'answer' ? replyAt(state, u.n) : lineOfCid(state, u.cid) === u.n;
    if (u.item == null) return true;
    if (u.itemCid) return lineOfCid(state, u.itemCid) === u.item;
    return replyAt(state, u.item) || (state.items || []).some(function (i) { return i.n === u.item; });
  }

  /** The line of a note or reply the state holds with `cid`, or null. */
  function lineOfCid(state, cid) {
    var found = null;
    (state.items || []).forEach(function (i) {
      if (i.cid === cid) found = i.n;
      (i.replies || []).forEach(function (r) { if (r.cid === cid && r.n != null) found = r.n; });
    });
    return found;
  }

  /** The round a hand-back on line `n` started, or null while the state shows none. */
  function roundFrom(state, n) {
    return (state.rounds || []).filter(function (r) { return r.submit === n; })[0] || null;
  }

  /** A hand-back on the history: withdrawn by z before pickup; a gone one is passed over. True when z is done. */
  function takeBackHandBack(top, state) {
    var p = (state && state.phrase) || {};
    if (p.stage === 'sent' && p.sent && p.sent.n === top.n && p.sent.can_withdraw) {
      undos.pop();
      keepUndos();
      BR.op({ op: 'withdraw', to: top.n });
      say(WITHDRAWN);
      return true;
    }
    var round = state ? roundFrom(state, top.n) : null;
    // Its ack came before the state that shows it
    if (!round && state && state.log && state.log.lines < top.n) return true;
    // Picked up since: it and all before it are Claude's
    if (round && round.claim != null) {
      undos = [];
      keepUndos();
      say(nothingLeft());
      return true;
    }
    undos.pop();
    keepUndos();
    return false;
  }

  function undo() {
    var state = BR.currentState();
    // A reload's entries wait for the state that says what was sent meanwhile
    if (unchecked && !state) {
      // One press waits; more would all land at once, unseen
      waitingZ = 1;
      return;
    }
    if (BR.chrome.withdrawBatch()) {
      say(WITHDRAWN);
      return;
    }
    var hold = BR.flow.holdActive();
    if (hold) {
      BR.chrome.withdrew(BR.op({ op: 'withdraw', to: hold.n }), hold.n);
      undos = undos.filter(function (u) { return !(u.kind === 'handback' && u.n === hold.n); });
      keepUndos();
      say(hold.kind === 'approve' ? APPROVE_WITHDRAWN : WITHDRAWN);
      return;
    }
    for (;;) {
      var top = undos[undos.length - 1];
      if (!top) {
        say(nothingLeft());
        return;
      }
      if (top.marker) {
        undos.pop();
        keepUndos();
        say(top.marker);
        return;
      }
      if (top.kind === 'handback') {
        if (takeBackHandBack(top, state)) return;
        continue;
      }
      if (top.n == null && state && top.cid && !top.folder) top.n = lineOfCid(state, top.cid);
      // Before its ack the line isn't known: take it back once the ack names it
      if (top.n == null) top.queued = true;
      else takeBack(top);
      return;
    }
  }

  /** A state came: a reload's entries learn their lines, a pickup clears what it took, and a waiting z runs. */
  function checkUndos(state) {
    var changed = false;
    if (unchecked) {
      var kept = undos.filter(function (u) { return stillOurs(state, u); });
      changed = kept.length !== undos.length;
      undos = kept;
    }
    undos.forEach(function (u) {
      if (u.n == null && u.cid && !u.marker && !u.folder && u.kind !== 'handback') {
        var n = lineOfCid(state, u.cid);
        if (n != null) {
          u.n = n;
          changed = true;
        }
      }
    });
    for (var i = undos.length - 1; i >= 0; i--) {
      var u = undos[i];
      var round = u.kind === 'handback' ? roundFrom(state, u.n) : null;
      if (round && round.claim != null) {
        undos = undos.slice(i + 1);
        changed = true;
        break;
      }
    }
    if (changed) keepUndos();
    unchecked = false;
    if (waitingZ) {
      waitingZ = 0;
      undo();
    }
  }

  function onFrame(msg) {
    if (msg.type === 'content') {
      if (msg.path && msg.path !== CURRENT_PATH) return;
      if (msg.stops) stops = msg.stops;
      relist = true;
      queueSettle();
    } else if (msg.type === 'review_state') {
      relist = true;
      stateCame = true;
      queueSettle();
      var state = BR.currentState();
      if (state && msg.path === CURRENT_PATH) checkUndos(state);
    } else if (msg.type === 'ack' || msg.type === 'nack') {
      var entry = undos.filter(function (u) { return u.cid === msg.cid; })[0];
      if (!entry) return;
      if (msg.type === 'nack') {
        undos.splice(undos.indexOf(entry), 1);
        keepUndos();
        return;
      }
      if (msg.n != null) entry.n = msg.n;
      keepUndos();
      if (entry.queued) takeBack(entry);
    }
  }

  // --- what moves focus, and what never does ---

  function inView(el) {
    var r = rectOf(el);
    return !!r && r.bottom > headerBottom() && r.top < viewBottom();
  }

  /** The first of `list` whose top is in view, else the first that shows at all; null for none. */
  function firstInView(list) {
    var top = headerBottom();
    var shown = list.filter(inView);
    return shown.filter(function (el) { return rectOf(el).top >= top - 1; })[0] || shown[0] || null;
  }

  function onScrollInput() { userAt = performance.now(); }

  /** Marks a scroll key as your scroll input; `e` is seen where its real target shows, inside a review host too. */
  function keyInput(e) {
    if (e.defaultPrevented || !SCROLL_KEYS[e.key]) return;
    // Arrows and Page keys scroll with a modifier too (⌘↓, ⌥↓); Space with one is the system's
    if (e.key === ' ' && (e.altKey || e.ctrlKey || e.metaKey)) return;
    var t = e.composedPath()[0];
    // A field keeps its keys, and Space presses a button
    if (t && (t.isContentEditable || /^(input|textarea|select)$/.test(t.localName))) return;
    if (e.key === ' ' && t && t.closest && t.closest('button, summary, [role="button"]')) return;
    onScrollInput();
  }

  // A closed review host shows the page only itself; its own root reports the key (BR.keys.fromHost)
  function onKeyInput(e) {
    if (!BR.isHost(e.composedPath()[0])) keyInput(e);
  }

  // Only your own scroll moves the focus: once it leaves the view, to the first stop in view
  function onScroll() {
    if (kept && Math.abs(window.scrollY - kept.y) > 1) kept = null;
    var now = performance.now();
    var chained = now - userLast <= USER_GAP_MS;
    if (now - userAt <= USER_MS || (chained && now - userAt <= USER_MAX_MS)) {
      if (!chained) scrolledTo = null;
      userLast = now;
      userScroll = true;
    }
    if (scrollQueued) return;
    scrollQueued = true;
    requestAnimationFrame(function () {
      scrollQueued = false;
      var mine = userScroll;
      userScroll = false;
      // The source view hides the doc; its focus comes back with it
      if (!mine || !focus || rawShown()) return;
      // Once your scroll has moved focus, it keeps to the first stop in view until that scroll ends
      if (focus !== scrolledTo) {
        if (inView(focus)) return;
        scrolledFrom = targets();
      }
      var next = firstInView(scrolledFrom);
      if (!next || next === focus) return;
      setFocus(next);
      scrolledTo = next;
      if (BR.isHost(next)) BR.flow.hold(next);
    });
  }

  /** Notes where the focus sits on screen before the page changes, to put it back after. */
  function pin() {
    if (!pinned) pinned = focus && focus.isConnected && inView(focus) ? { el: focus, top: rectOf(focus).top } : {};
    queueSettle();
  }

  function queueSettle() {
    if (settleQueued) return;
    settleQueued = true;
    Promise.resolve().then(settle);
  }

  // Puts the focus back where the reader last saw it, so a page-made change above never moves it
  function keepAt(el, top) {
    var r = rectOf(el);
    if (!r) return;
    var dy = r.top - top;
    if (Math.abs(dy) >= 1) window.scrollBy(0, dy);
    if (pinned && pinned.el === el) pinned.top = top;
    kept = { el: el, top: top, y: window.scrollY };
  }

  // A row or note can take its full height a frame late, as WebKit lays out a new host
  function onResize() {
    if (kept && kept.el === focus && kept.el.isConnected) keepAt(kept.el, kept.top);
  }

  function gone(el) {
    if (el === docStop) return false;
    return !el.isConnected || (BR.isHost(el) && BR.flow.blankItem(el) == null && !BR.flow.removalOf(el));
  }

  /** After a frame, a patch or a row opening: focus that went is handed on, and focus that stayed keeps its place. */
  function settle() {
    settleQueued = false;
    checkRound();
    var was = pinned || {};
    pinned = null;
    var fresh = stateCame;
    stateCame = false;
    if (focus && gone(focus)) {
      var lost = focus;
      var line = focusWas && focusWas.item === undefined && focusWas.sp ? lineOf(focusWas.sp) : null;
      var next = replacement();
      // The page moved it, so the walk's place stays
      setFocus(next.el, null, true);
      relist = false;
      if (next.same && was.el === lost) keepAt(focus, was.top);
      // Only a block found by its own words survived; one on the same line may have been cut
      if (!next.exact && line != null) cutLine = line;
      if (fresh) handOff();
      return;
    }
    if (fresh) handOff();
    if (relist && focus) {
      focusList = targets();
      focusWas = describe(focus, focusList);
      // A patch can shift the focused block's lines, which a reload must find it by
      keepStop(focus);
    }
    relist = false;
    if (focus && was.el === focus) keepAt(focus, was.top);
  }

  /**
   * A block Claude cut hands focus to its removal's stop once the state naming it comes: the
   * removal of the latest write whose old lines held it.
   */
  function handOff() {
    var line = cutLine;
    cutLine = null;
    var state = BR.currentState();
    if (line == null || !state) return;
    // Old lines are counted in the text before their own write, so only the latest write's are this page's
    var latest = Math.max.apply(null, (state.changes || []).map(function (c) { return c.op; }).concat([-1]));
    var hit = null;
    (state.changes || []).forEach(function (c) {
      if (c.op !== latest || !c.old || c.old[0] > line || line > c.old[1] || !BR.flow.removalStop(c.id)) return;
      if (!hit || c.idx > hit.idx) hit = c;
    });
    if (!hit) return;
    var to = BR.flow.removalStop(hit.id);
    setFocus(to, null, true);
    if (!inView(to)) reveal(to);
  }

  /** A removal stop as the strip names it: the heading over its gap, and what went. */
  function removalFocus(gap) {
    var state = BR.currentState();
    var d = state && (state.deletions || []).filter(function (x) { return x.change === gap.change && x.kind === 'block'; })[0];
    var above = gap.after ? blockOf(gap.after) : null;
    var kind = d && d.label ? String(d.label).split(' · ')[0] : 'block';
    return { k: 'removal', change: gap.change, heading: above ? BR.flow.crumb(above).heading : null, kind: kind };
  }

  /**
   * What takes the place of a focus that went: a question folded into its note hands it to the
   * note's block, or whole doc for a whole-doc note; a rebuilt block is itself; else the next stop
   * below, or the last above.
   */
  function replacement() {
    var all = targets();
    var w = focusWas || { at: -1 };
    if (w.item != null) {
      var state = BR.currentState();
      var item = state && (state.items || []).filter(function (i) { return i.n === w.item; })[0];
      if (item && item.status !== 'retracted' && item.status !== 'withdrawn') {
        if (!w.owner) return { el: docStop };
        var under = survivor(w.owner, all, w.at - 1);
        if (under) return { el: under };
      }
    }
    var same = refind(all);
    if (same) return { el: same.el, same: true, exact: same.exact };
    var old = focusList || [];
    for (var i = w.at + 1; i < old.length; i++) {
      var below = survivor(old[i], all, i);
      if (below) return { el: below };
    }
    for (var j = w.at - 1; j >= 0; j--) {
      var above = survivor(old[j], all, j);
      if (above) return { el: above };
    }
    return { el: docStop };
  }

  /** `el` while it is still a stop, else the stop a patch put in its place: same tag and words, nearest `near`. */
  function survivor(el, all, near) {
    if (!el) return null;
    if (all.indexOf(el) >= 0) return el;
    if (BR.isHost(el)) return null;
    var best = null;
    var gap = Infinity;
    all.forEach(function (t, i) {
      if (t.localName !== el.localName || t.textContent !== el.textContent || Math.abs(i - near) >= gap) return;
      gap = Math.abs(i - near);
      best = t;
    });
    return best;
  }

  BR.pointing = {
    init: function () {
      stops = (BOOT.stops || []).slice();
      try { active = sessionStorage.getItem(ACTIVE_KEY) === '1'; } catch (e) {}
      restoreUndos();
      docStop = BR.flow.docStop();
      // A tab opens on whole doc, or on the stop it had before a reload
      var back = restored();
      setFocus(back || docStop, null, true);
      if (back) reveal(back);
      document.addEventListener('selectionchange', onSelection);
      document.addEventListener('click', onClick);
      document.addEventListener('focusout', onFocusOut);
      document.addEventListener('focusin', onFocusIn);
      document.addEventListener('birta-view', onView);
      window.addEventListener('keydown', function (e) { if (e.key === 'Tab') byKeys = true; }, true);
      window.addEventListener('pointerdown', function () { byKeys = false; }, true);
      untabAnchors();
      window.addEventListener('scroll', onScroll, { passive: true });
      window.addEventListener('wheel', onScrollInput, { passive: true, capture: true });
      window.addEventListener('touchmove', onScrollInput, { passive: true, capture: true });
      window.addEventListener('keydown', onKeyInput);
      if (typeof ResizeObserver !== 'undefined') new ResizeObserver(onResize).observe(content());
      // A doc patch pins the focus here; the flow pins it before it redraws (BR.pointing.pin)
      if (typeof birtaPatch !== 'undefined') {
        var apply = birtaPatch.apply;
        birtaPatch.apply = function (html, o) {
          pin();
          var out = apply.call(birtaPatch, html, o);
          untabAnchors();
          return out;
        };
      }
      birtaBus.on(onFrame);
    },
    /** Runs a review action; false leaves the key to the page. */
    act: function (action) {
      if (ACTIVATES[action]) activate();
      if (MOVES[action]) unpick();
      if (action !== 'no') BR.flow.disarm();
      // The clean view hides the rows, so its keys come before theirs
      if (BR.chrome.clean()) {
        if (REOPENS[action]) {
          say(REOPEN);
          return true;
        }
        if (action === 'note' || action === 'ask') {
          var on = target();
          var kind = action === 'ask' ? 'ask' : 'note';
          BR.flow.open(kind, on, null, 'new round · ' + BR.flow.label(on, kind));
          return true;
        }
      }
      // Claude's news on the stop takes y and u first, as one check of the whole stop
      if ((action === 'yes' || action === 'revert_change') && !selectedRange() && checkStop(action)) return true;
      // An open change row takes its keys first, the whole-doc stop's included; a selection beats it
      // c: a selection, then your unsent note on the stop, then Claude's change, then the stop
      var mine = action === 'note' && !selectedRange() && !!newestDraft(target());
      var row = ROW_KEYS[action] && !mine && (action === 'revert_change' || !selectedRange()) ? BR.flow.openRow() : null;
      if (row) {
        BR.flow.rowKey(action, row);
        return true;
      }
      // Its own key with no row open: a revert needs a change to revert
      if (action === 'revert_change') {
        if (!selectedRange()) say(BR.pure.wholeDocPill(action));
        return true;
      }
      if (action === 'focus_next' || action === 'focus_prev') {
        move(action === 'focus_next' ? 1 : -1);
        return true;
      }
      if (action === 'focus_first' || action === 'focus_last') {
        jump(action === 'focus_first' ? 'first' : 'last');
        return true;
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
      if (action === 'note') {
        note();
        return true;
      }
      if (action === 'ask') {
        BR.flow.open('ask', target(), null);
        return true;
      }
      if (action === 'open') {
        // A focused blank opens its own field first; then the picked link, a dot or half-typed words, the only link, else a message
        // Enter on a link, summary or button keeps its own meaning, so a picked link opens natively
        var held = document.activeElement;
        if (held && held !== document.body && held.closest && held.closest('a, summary, button, input, select, textarea')) return false;
        if (isSummary(focus)) return false;
        if (BR.flow.openAt(focus)) return true;
        var links = focus ? linksIn(focus) : [];
        if (links.length === 1) links[0].click();
        else if (links.length > 1) say(links.length + ' links · Tab to pick');
        return links.length > 0;
      }
      return false;
    },
    /** The focused stop as the strip names it: `{k:"doc"}`, `{k:"blank", id}`, `{k:"removal", change, heading, kind}` or `{k:"block", sp, heading, kind}`. */
    stripFocus: function () {
      var el = isSummary(focus) ? summaryStop(focus) || focus : focus;
      if (!el || el === docStop) return { k: 'doc' };
      if (BR.isHost(el)) {
        var n = BR.flow.blankItem(el);
        if (n != null) return { k: 'blank', id: '#' + n };
        var gap = BR.flow.removalOf(el);
        return gap ? removalFocus(gap) : { k: 'doc' };
      }
      var c = BR.flow.crumb(el);
      return { k: 'block', sp: el.getAttribute('data-sourcepos'), heading: c.heading, kind: c.kind };
    },
    /** Whether a selection outranks the stop, so c, q and x act on its words. */
    selecting: function () {
      var range = selectedRange();
      return !!range && !!BR.quote.fromRange(range);
    },
    /** A header control that Tab, not a click, gave the keyboard; null for none. */
    keyedControl: function () { return byKeys ? pageControl() : null; },
    /** The focused stop's element, whole doc's host included, or null once a click went elsewhere. */
    focused: function () { return pointedAway ? null : focus; },
    /** Whether `el` is the title, whose verdicts and wash are on its own line. */
    isTitle: isTitle,
    /** Writes verdict `name` on request target `at`, as its key would on that block. */
    verdictOn: verdictOn,
    /** Leaves reading mode, then the source view, so a key acts on the rendered doc. */
    leaveModes: leaveModes,
    /** The approve guard's jump: to the first walk stop with something to check. False when there is none. */
    guard: function () {
      var state = BR.currentState();
      if (!state) return false;
      var all = walkOf(state);
      var first = all.filter(function (x) { return BR.pure.isNews(x.kind) && x.check == null; })[0];
      if (first) land(all.filter(function (x) { return stopOf(x) === stopOf(first); })[0], state);
      return !!first;
    },
    /** y or u on the focused stop's news: true when it acted; y on a stop with none is left to the page. */
    checkStop: checkStop,
    /** Lands on this page's first or last walk stop, else stays on whole doc, and says `says` when given. */
    landAt: function (which, says) {
      var state = BR.currentState();
      if (!state) return;
      var all = walkOf(state);
      var stop = which === 'last' ? all[all.length - 1] : all[0];
      if (stop) land(stop, state);
      if (says) say(says);
    },
    /** Notes the focus's place before the page changes under it; it is put back once the change is done. */
    pin: pin,
    /** Scrolls `el` into the view between the header and the strip, as the page's own move. */
    reveal: reveal,
    keyInput: keyInput,
    /** Puts an act of this tab's on the z history: `{kind, cid, undo, label, item?, n?}`, or `{marker}`. */
    remember: remember,
    tab: tab,
    /**
     * Drops one Esc layer: the phrase menu or key sheet, the composer, the selection, a picked
     * link or page control, then reading mode or the source view. A layer the source view hides is
     * skipped. False when none is there; focus stays.
     */
    escape: function () {
      if (BR.chrome.escape()) return true;
      var raw = rawShown();
      if (!raw && BR.flow.escape()) return true;
      var s = getSelection();
      if (!raw && s && s.rangeCount && !s.isCollapsed) {
        s.removeAllRanges();
        onSelection();
        return true;
      }
      if (!raw && unpick()) return true;
      // A control a click focused shows no ring, so only one Tab reached is a layer
      var ctl = byKeys && pageControl();
      if (ctl) {
        ctl.blur();
        return true;
      }
      if (readingShown() && window.birtaView) {
        window.birtaView.toggleReadingMode(false);
        return true;
      }
      if (raw && window.birtaView) {
        window.birtaView.setViewMode('preview');
        return true;
      }
      return false;
    },
    pageControl: pageControl
  };
})();
