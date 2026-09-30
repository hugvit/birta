var BR = typeof BR === 'undefined' ? {} : BR;
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

  var stops = [];
  var focus = null;
  // What the focused block looked like, to find it again when a patch replaces its node
  var focusWas = null;
  var active = false;
  // The last verdict this page sent, which z takes back once its ack names its line
  var last = null;
  var pill = null;
  var pillText = null;
  var scrollQueued = false;

  function content() { return document.getElementById('content'); }

  function blockOf(sp) {
    // A list shares its first item's sp and comes first, so the last match is the item
    var all = content().querySelectorAll('[data-sourcepos="' + sp + '"]');
    return all[all.length - 1] || null;
  }

  function isSummary(el) { return !!el && el.localName === 'summary'; }

  /** The focus stops in document order; a closed `<details>` stands in with its summary. */
  function targets() {
    var out = [];
    stops.forEach(function (s) {
      var el = blockOf(s.sp);
      if (!el) return;
      var closed = el.parentElement && el.parentElement.closest('details:not([open])');
      if (closed && content().contains(closed)) {
        var summary = closed.querySelector(':scope > summary');
        if (summary && !summary.contains(el)) {
          if (out[out.length - 1] !== summary) out.push(summary);
          return;
        }
      }
      if (el.getClientRects().length) out.push(el);
    });
    return out;
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
  }

  function setFocus(el) {
    focus = el;
    focusWas = el && { name: el.localName, text: el.textContent, at: targets().indexOf(el) };
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
      next = all.filter(function (el) { return el.getBoundingClientRect().top >= top; })[0] || all[all.length - 1];
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
    setFocus(next);
    reveal(next);
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

  // --- the pill ---

  function showPill(text) {
    if (!pill) {
      pill = document.createElement('birta-pop');
      pill.style.setProperty('position', 'fixed', 'important');
      pill.style.setProperty('z-index', '2147483647', 'important');
      pill.style.setProperty('left', '50%', 'important');
      pill.style.setProperty('bottom', '16px', 'important');
      pill.style.setProperty('transform', 'translateX(-50%)', 'important');
      var root = pill.attachShadow({ mode: 'closed' });
      var style = document.createElement('style');
      style.textContent = BR.shadowCss || '';
      pillText = document.createElement('div');
      pillText.className = 'pill';
      root.append(style, pillText);
      BR.FLOW_HOSTS.add(pill);
      document.body.appendChild(pill);
    }
    pillText.textContent = text;
    pill.style.setProperty('display', text ? 'block' : 'none', 'important');
  }

  function pillSays(text) { return !!pill && pillText.textContent === text; }

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

  function verdict(name) {
    var state = BR.currentState();
    var cid = BR.op({ op: 'comment', at: target(), verdict: name });
    last = { cid: cid, n: null, created: !state || !state.log || !state.log.lines };
  }

  function undo() {
    if (!last || last.n == null) return;
    BR.op({ op: 'retract', to: last.n });
    last = null;
    if (pillSays(CREATED)) showPill('');
  }

  function onFrame(msg) {
    if (msg.type === 'content') {
      if (msg.path && msg.path !== CURRENT_PATH) return;
      if (msg.stops) stops = msg.stops;
      if (focus && !focus.isConnected) setFocus(refind());
    } else if (msg.type === 'ack' && last && msg.cid === last.cid) {
      last.n = msg.n;
      if (last.created) showPill(CREATED);
    } else if (msg.type === 'nack' && last && msg.cid === last.cid) {
      last = null;
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
      return false;
    },
    /** Drops one Esc layer: the selection, then the focus. False when neither is there. */
    escape: function () {
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
