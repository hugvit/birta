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
  var HINT = 'c note · q ask · x cut';
  var CLAMPED = 'selection clamped to this paragraph';
  var CREATED = 'note saved · z to undo';
  var END = 'no more · j/k to move';
  var TOP_OF_DOC = 'top of the doc';
  var END_OF_DOC = 'end of the doc';
  var REOPEN = 'approved · c to reopen';
  // After approve these only say how to start again
  var REOPENS = { yes: 1, no: 1, later: 1, your_call: 1, ask: 1, next_item: 1, prev_item: 1 };
  var RENDERED = 'rendered view';
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

  /**
   * The doc stop's block, when the first stop is an H1 whose section holds every later stop:
   * that heading and the doc are then one stop. Null otherwise.
   */
  BR.pure.wholeDocStop = function (stops) {
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
    if (action === 'later') return 'later needs a block · ' + j;
    if (action === 'your_call') return 'your call needs a block · ' + j;
    if (action === 'no') return 'whole doc: ' + c + " to say what's wrong";
    if (!b) return null;
    var key = mac ? '⌘⏎' : 'Ctrl⏎';
    if (/^Hand back/.test(b.text)) return key + ' hands back';
    if (/^Approve/.test(b.text) && !b.disabled) return key + ' approves';
    if (b.text === 'Approved') return 'approved · ' + c + ' to reopen';
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
  // A click landed outside the focused block since it took focus
  var pointedAway = false;
  // The link Tab picked inside the focused block; ⏎ follows it natively while it has DOM focus
  var picked = null;
  // What the focused block looked like, to find it again when a patch replaces its node
  var focusWas = null;
  // The stops as they were when focus last settled, to find what comes after it once it goes
  var focusList = null;
  var active = false;
  // This page's own acts, newest last; z takes back the top once its ack names its line
  var undos = [];
  var scrollQueued = false;
  // Where n and p last landed, as `[line, rank, order]`, so a server push can't skip a stop
  var cursor = null;
  // The round the cursor walked; a new one starts the walk from the top
  var cursorRound = null;
  // The focus a landing left: once focus moves off it, the walk goes on from the focus
  var cursorAt = null;

  // Each stop's element, or the summary of a closed details for those it hides, until the doc changes
  var placed = null;
  var placedAt = -1;
  var placedFor = null;
  var standIns = new Set();
  // The walk's stops for the state they came from
  var walkFor = null;
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
    var out = placedStops().filter(function (el) { return standIns.has(el) || el.getClientRects().length; });
    // A blank is a stop of its own, right where it sits
    var blanks = BR.flow.blanks();
    if (!blanks.length) return out;
    return out.concat(blanks).sort(function (a, b) {
      return a === b ? 0 : a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1;
    });
  }

  /** The whole-doc H1's element, or null when the doc has none. */
  function wholeDoc() {
    var sp = BR.pure.wholeDocStop(stops);
    return sp ? blockOf(sp) : null;
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

  /** Tab and Shift+Tab: steps the pick through the focused block's links, round the end. False when it has none. */
  function tab(step) {
    var links = focus && !isSummary(focus) ? linksIn(focus) : [];
    if (!links.length) return false;
    var at = links.indexOf(picked);
    pick(links[at < 0 ? (step > 0 ? 0 : links.length - 1) : (at + step + links.length) % links.length]);
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
    var stop = stopAt(a);
    if (!stop || isSummary(stop) || linksIn(stop).indexOf(a) < 0) return;
    if (stop !== focus) setFocus(stop);
    picked = a;
    a.setAttribute('data-br-pick', '');
  }

  function setFocus(el, all) {
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
    BR.lane.focus(el);
    BR.flow.focusBlock(el);
    paintDoc();
  }

  /** What the focused stop looked like, to find it, or what replaced it, once a patch has gone through. */
  function describe(el, list) {
    var w = { name: el.localName, text: el.textContent, sp: el.getAttribute('data-sourcepos'), at: list.indexOf(el) };
    if (BR.isHost(el)) {
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
    var r = el.getBoundingClientRect();
    var top = headerBottom();
    if (r.top < top) window.scrollBy(0, r.top - top - 8);
    else if (r.bottom > innerHeight) window.scrollBy(0, Math.min(r.bottom - innerHeight + 8, r.top - top - 8));
    // The place to keep is the revealed one, not where the stop sat before
    if (pinned && pinned.el === el) pinned.top = el.getBoundingClientRect().top;
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
    else see(next);
  }

  function move(step) {
    dropSelection();
    var all = targets();
    if (!all.length) return;
    if (!focus) {
      // The doc sits above every block
      if (step < 0) {
        showPill(TOP_OF_DOC);
        return;
      }
      var top = headerBottom() - 1;
      go(all.find(function (el) { return el.getBoundingClientRect().top >= top; }) || all[all.length - 1], all);
      return;
    }
    var at = all.indexOf(focus);
    if (at < 0) {
      // Focus sits inside a details that has since closed: count from where it was
      var after = all.findIndex(function (el) {
        return focus.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING;
      });
      if (after < 0) after = all.length;
      at = step > 0 ? after - 1 : after;
    }
    // The doc is a stop above the first block, unless a whole-doc H1 stands for it
    var lead = wholeDoc() ? 0 : 1;
    var r = BR.pure.focusStep(at + lead, all.length + lead, step);
    if (r.says) showPill(r.says);
    var next = r.to < lead ? null : all[r.to - lead];
    if (next === focus) return;
    if (next) go(next, all);
    else setFocus(null, all);
  }

  /** Home and End: the first or the last stop, brought into view. */
  function jump(which) {
    dropSelection();
    var all = targets();
    if (!all.length) return;
    var next = all[BR.pure.focusStep(-1, all.length, which).to];
    if (next === focus) reveal(next);
    else go(next, all);
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
    var host = BR.flow.focusStop(stop);
    // A blank is the stop itself, so j and k go on from it
    if (stop.kind === 'blank' && host) {
      setFocus(host);
      reveal(host);
    }
    cursorAt = focus;
    showPill((all.indexOf(stop) + 1) + ' of ' + all.length + ' · n next · p back');
  }

  /** Where the walk goes on from once focus left its last stop: past the focused block, or the viewport's top with none. */
  function cursorFromFocus() {
    if (!focus) return cursorFromView();
    var sp = focus.getAttribute && focus.getAttribute('data-sourcepos');
    // A blank or a summary has no line of its own, so the walk keeps its place
    return sp ? [lineOf(sp), Infinity, Infinity] : cursor;
  }

  /**
   * The walk's place with nothing focused: n goes on from the first stop at or below the viewport's
   * top, p from the last above it, so a scroll never sends the reader back to the first stop.
   */
  function cursorFromView() {
    var top = headerBottom() - 1;
    var blocks = targets().filter(function (el) { return !!el.getAttribute('data-sourcepos'); });
    // The doc's top in view: the walk starts at its very top, the doc slot included
    if (!blocks.length || blocks[0].getBoundingClientRect().top >= top) return null;
    var below = blocks.filter(function (el) { return el.getBoundingClientRect().top >= top; })[0];
    return below ? [lineOf(below.getAttribute('data-sourcepos')), -Infinity, -Infinity] : [Infinity, 0, 0];
  }

  function walk(step) {
    var state = BR.currentState();
    if (!state || state.log_writable === false) return;
    if (cursorRound !== roundOf(state)) cursor = null;
    if (cursor && focus !== cursorAt) cursor = cursorFromFocus();
    var all = walkOf(state);
    var next = step > 0
      ? all.filter(function (x) { return !cursor || before(cursor, keyOf(x)); })[0]
      : all.filter(function (x) { return cursor && before(keyOf(x), cursor); }).pop();
    if (next) land(next, state);
    else if (!BR.dir.cross(step)) showPill(END);
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

  /** What a verdict points at: the selection, else the focus, else the whole doc. */
  function target() {
    var range = selectedRange();
    var q = range && BR.quote.fromRange(range);
    if (q) return quoteAnchor(q);
    var block = isSummary(focus) ? summaryStop(focus) : focus;
    if (block && (block === wholeDoc() || BR.isHost(block))) block = null;
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
    if (best || !focusWas.sp) return best;
    var line = lineOf(focusWas.sp);
    return all.filter(function (el) {
      var sp = el.getAttribute('data-sourcepos');
      return el.localName === focusWas.name && !!sp && lineOf(sp) === line;
    })[0] || null;
  }

  function lineOf(sp) { return parseInt(sp, 10); }

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

  function remember(entry) {
    entry.n = null;
    undos.push(entry);
  }

  /** What a verdict on `at` is on, as its pill names it. */
  function scopeLabel(at) {
    if (at.k === 'quote') return '"' + BR.pure.fewWords(at.q) + '"';
    var el = at.sp ? blockOf(at.sp) : null;
    return el ? BR.flow.blockLabel(el, true) : 'this block';
  }

  function verdict(name) {
    var at = target();
    if (at.k === 'doc' && name === 'yes' && BR.flow.rewriteActive()) {
      BR.flow.markRewriteSeen();
      return;
    }
    if (at.k === 'doc') {
      // A whole-doc verdict asks the agent nothing it can act on
      var keys = { focus_next: BR.keyName('focus_next'), note: BR.keyName('note') };
      showPill(BR.pure.wholeDocPill(name, BR.chrome.button(), /Mac/.test(navigator.platform), keys) || '');
      return;
    }
    verdictOn(name, at);
  }

  /** Writes verdict `name` on `at`, or changes or clears your draft one there, and pills what it did. */
  function verdictOn(name, at) {
    var own = draftVerdictAt(at);
    if (!own) {
      remember({
        cid: BR.op({ op: 'comment', at: at, verdict: name }),
        undo: function (n) { return { op: 'retract', to: n }; }
      });
      showPill(WORD[name] + ' · ' + scopeLabel(at) + ' · ' + BR.keyName('undo') + ' to undo');
      return;
    }
    var was = own.verdict;
    if (own.verdict !== name) {
      remember({
        cid: BR.op({ op: 'amend', to: own.n, verdict: name }),
        item: own.n,
        undo: function () { return { op: 'amend', to: own.n, verdict: was }; }
      });
      showPill(WORD[name] + ' · ' + scopeLabel(at) + ' · ' + BR.keyName('undo') + ' to undo');
      return;
    }
    if (own.body) {
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
    showPill(WORD[name] + ' removed · ' + BR.keyName('undo') + ' to undo');
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

  /** Flips the source view, keeping the focused block and saying how back. */
  function toggleRaw() {
    if (typeof setViewMode !== 'function') return false;
    var raw = viewMode !== 'raw';
    var kept = focus;
    setViewMode(raw ? 'raw' : 'preview');
    showPill(raw ? 'source view · ' + BR.keyName('toggle_raw') + ' back' : RENDERED);
    if (raw) return true;
    BR.lane.redraw();
    if (kept && kept.isConnected) {
      setFocus(kept);
      reveal(kept);
    }
    return true;
  }

  /** Whether the note on line `n` has left the drafts, so z can no longer touch it. */
  function sent(n) {
    var state = BR.currentState();
    var item = state && (state.items || []).filter(function (i) { return i.n === n; })[0];
    // A reply is no item, but a hand-back after it took it all the same
    var handed = !!state && (state.rounds || []).some(function (r) { return r.submit > n; });
    return handed || (!!item && item.status !== 'draft' && item.status !== 'retracted');
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
      relist = true;
      queueSettle();
    } else if (msg.type === 'review_state') {
      relist = true;
      queueSettle();
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

  // --- what moves focus, and what never does ---

  function inView(el) {
    if (!el.getClientRects().length) return false;
    var r = el.getBoundingClientRect();
    return r.bottom > headerBottom() && r.top < innerHeight;
  }

  /** The first of `list` whose top is in view, else the first that shows at all; null for none. */
  function firstInView(list) {
    var top = headerBottom();
    var shown = list.filter(inView);
    return shown.filter(function (el) { return el.getBoundingClientRect().top >= top - 1; })[0] || shown[0] || null;
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
      if (!mine || !focus || (typeof viewMode !== 'undefined' && viewMode === 'raw')) return;
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
    if (!pinned) pinned = focus && focus.isConnected && inView(focus) ? { el: focus, top: focus.getBoundingClientRect().top } : {};
    queueSettle();
  }

  function queueSettle() {
    if (settleQueued) return;
    settleQueued = true;
    Promise.resolve().then(settle);
  }

  // Puts the focus back where the reader last saw it, so a page-made change above never moves it
  function keepAt(el, top) {
    var dy = el.getBoundingClientRect().top - top;
    if (Math.abs(dy) >= 1) window.scrollBy(0, dy);
    if (pinned && pinned.el === el) pinned.top = top;
    kept = { el: el, top: top, y: window.scrollY };
  }

  // A row or note can take its full height a frame late, as WebKit lays out a new host
  function onResize() {
    if (kept && kept.el === focus && kept.el.isConnected) keepAt(kept.el, kept.top);
  }

  function gone(el) {
    return !el.isConnected || (BR.isHost(el) && BR.flow.blankItem(el) == null);
  }

  /** After a frame, a patch or a row opening: focus that went is handed on, and focus that stayed keeps its place. */
  function settle() {
    settleQueued = false;
    var was = pinned || {};
    pinned = null;
    if (focus && gone(focus)) {
      var lost = focus;
      var next = replacement();
      setFocus(next.el);
      relist = false;
      // The block n landed on, rebuilt, is still where the walk is
      if (lost === cursorAt) cursorAt = focus;
      if (next.same && was.el === lost) keepAt(focus, was.top);
      return;
    }
    if (relist && focus) {
      focusList = targets();
      focusWas = describe(focus, focusList);
    }
    relist = false;
    if (focus && was.el === focus) keepAt(focus, was.top);
  }

  /**
   * What takes the place of a focus that went: a question folded into its note hands it to the
   * note's block; a rebuilt block is itself; else the next stop below, or the last above.
   */
  function replacement() {
    var all = targets();
    var w = focusWas || { at: -1 };
    if (w.item != null) {
      var state = BR.currentState();
      var item = state && (state.items || []).filter(function (i) { return i.n === w.item; })[0];
      if (item && item.status !== 'retracted' && item.status !== 'withdrawn') {
        if (!w.owner) return { el: null };
        var under = survivor(w.owner, all, w.at - 1);
        if (under) return { el: under };
      }
    }
    var same = refind(all);
    if (same) return { el: same, same: true };
    var old = focusList || [];
    for (var i = w.at + 1; i < old.length; i++) {
      var below = survivor(old[i], all, i);
      if (below) return { el: below };
    }
    for (var j = w.at - 1; j >= 0; j--) {
      var above = survivor(old[j], all, j);
      if (above) return { el: above };
    }
    return { el: null };
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
      paintDoc();
      document.addEventListener('selectionchange', onSelection);
      document.addEventListener('click', onClick);
      document.addEventListener('focusout', onFocusOut);
      document.addEventListener('focusin', onFocusIn);
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
          return apply.call(birtaPatch, html, o);
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
          showPill(REOPEN);
          return true;
        }
        if (action === 'note') {
          var on = target();
          BR.flow.open('note', on, null, 'new round · ' + BR.flow.label(on));
          return true;
        }
      }
      // An open change row takes its keys first, the whole-doc stop's included; a selection beats it
      var row = ROW_KEYS[action] && (action === 'revert_change' || !selectedRange()) ? BR.flow.openRow() : null;
      if (row) {
        BR.flow.rowKey(action, row);
        return true;
      }
      // Its own key with no row open: a revert needs a change to revert
      if (action === 'revert_change') return true;
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
        // A focused blank opens its own field first; then the picked link, a dot or half-typed words, the only link, else a pill
        // Enter on a link, summary or button keeps its own meaning, so a picked link opens natively
        var held = document.activeElement;
        if (held && held !== document.body && held.closest && held.closest('a, summary, button, input, select, textarea')) return false;
        if (isSummary(focus)) return false;
        if (BR.flow.openAt(focus)) return true;
        var links = focus ? linksIn(focus) : [];
        if (links.length === 1) links[0].click();
        else if (links.length > 1) showPill(links.length + ' links · Tab to pick');
        return links.length > 0;
      }
      return false;
    },
    /** Shows `text` in the pill, or hides it for ''. */
    pill: showPill,
    /** The focused stop's element, or null on the doc or once a click went elsewhere. */
    focused: function () { return pointedAway ? null : focus; },
    /** Writes verdict `name` on request target `at`, as its key would on that block. */
    verdictOn: verdictOn,
    /** The shared key away from a change row: the source view and back, focus kept. */
    toggleRaw: toggleRaw,
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
    /** Notes the focus's place before the page changes under it; it is put back once the change is done. */
    pin: pin,
    keyInput: keyInput,
    /** Puts an act of this page's on the z stack: `{cid, undo(n) -> op fields, item?, created?}`. */
    remember: remember,
    tab: tab,
    /** Drops one Esc layer: the phrase menu or key sheet, the composer, the selection, a picked link. False when none is there; focus stays. */
    escape: function () {
      if (BR.chrome.escape()) return true;
      if (BR.flow.escape()) return true;
      var s = getSelection();
      if (s && s.rangeCount && !s.isCollapsed) {
        s.removeAllRanges();
        onSelection();
        return true;
      }
      return unpick();
    }
  };
})();
