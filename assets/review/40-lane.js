var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  function barStyle(c) {
    if (c.changes_your_edit || c.reverts_your_edit) return 'double';
    if (c.attr === 'outside' || (c.asked && c.asked.kind === 'unasked')) return 'dotted';
    return 'solid';
  }

  function newest(a, b) { return (a.op - b.op) || (a.idx - b.idx); }

  /**
   * What the lane draws for a state, before any geometry:
   * `{mark:'glyph', sp, verdict, role, n}`, `{mark:'veil', sp, section}` over a block or section
   * told no, `{mark:'bar', sp, style, role, weight, fresh, check}`, `{mark:'wash', sp, section?}`
   * over a block with unseen changes or a section with a verdict,
   * `{mark:'revert', sp}` where your revert put Claude's change back, and `{mark:'tick', after,
   * count}`, where `after` is null above the first block. A bar is 3 px and new until its change
   * is checked, then 1 px and muted.
   */
  BR.pure.laneMarks = function (state, opts) {
    // Inside a hand-back's hold nothing has left yet, so its verdicts keep your colour
    var now = opts && opts.now != null ? opts.now : state.server_now;
    var held = {};
    if (state.hold && state.hold.kind === 'hand_back' && now < state.hold.not_before) {
      (state.rounds || []).forEach(function (r) {
        if (r.submit === state.hold.n) (r.items || []).forEach(function (n) { held[n] = true; });
      });
    }
    var out = [];
    var sectionWashed = {};
    (state.items || [])
      .filter(function (i) {
        return i.verdict && i.status !== 'retracted' && i.at && i.at.loc && i.at.loc.sp;
      })
      .sort(function (a, b) { return a.n - b.n; })
      .forEach(function (i) {
        // A withdrawn round hands its notes back as drafts
        var yours = i.status === 'draft' || i.status === 'withdrawn' || !!held[i.n];
        out.push({ mark: 'glyph', sp: i.at.loc.sp, verdict: i.verdict, role: yours ? 'you' : 'agent', n: i.n });
        if (i.verdict === 'no') out.push({ mark: 'veil', sp: i.at.loc.sp, section: i.at.scope === 'section' });
        // A section's verdict reaches every block of it, so the wash shows that scope
        if (i.at.scope === 'section' && !sectionWashed[i.at.loc.sp]) {
          sectionWashed[i.at.loc.sp] = true;
          out.push({ mark: 'wash', sp: i.at.loc.sp, section: true });
        }
      });

    var bySp = {};
    var order = [];
    var undone = {};
    // The writes that reverted a change of Claude's: each shows as a ↺ head, never your bar
    var undoOps = {};
    (state.changes || []).forEach(function (c) { if (c.undone_by != null) undoOps[c.undone_by] = true; });
    var reverts = [];
    (state.changes || []).forEach(function (c) {
      var sp = c.range && c.range.sp;
      // A reverted change's text is gone; its revert marks the block
      if (c.undone_by != null) {
        undone[c.id] = true;
        return;
      }
      if (!sp) return;
      if (c.attr === 'human' && undoOps[c.op]) {
        if (reverts.indexOf(sp) < 0) reverts.push(sp);
        return;
      }
      if (!bySp[sp]) {
        bySp[sp] = [];
        order.push(sp);
      }
      bySp[sp].push(c);
    });
    order.forEach(function (sp) {
      var all = bySp[sp].slice().sort(newest);
      var live = all.filter(function (c) { return !c.settled; });
      // Claude's change stays new until you check it; yours until it is seen
      var fresh = live.filter(function (c) { return c.attr !== 'human' && c.check == null; });
      var unseen = live.filter(function (c) { return !c.seen && (c.attr === 'human' || c.check == null); });
      var pick = unseen.length ? unseen : live.length ? live : all;
      var win = pick[pick.length - 1];
      var thick = live.some(function (c) { return c.attr === 'human' || c.check == null; });
      out.push({ mark: 'bar', sp: sp, style: barStyle(win), role: win.attr === 'human' ? 'you' : 'agent',
        weight: thick ? 3 : 1, fresh: fresh.length > 0, check: fresh.length ? 'new' : 'checked' });
      if (unseen.length) out.push({ mark: 'wash', sp: sp });
    });
    reverts.forEach(function (sp) { out.push({ mark: 'revert', sp: sp }); });

    var ticks = {};
    var tickOrder = [];
    // A settled removal keeps its tick; one you reverted put its text back
    (state.deletions || []).forEach(function (d) {
      if (d.kind !== 'block' || undone[d.change]) return;
      var after = d.after_sp == null ? null : d.after_sp;
      var key = after === null ? '' : after;
      if (!ticks[key]) {
        ticks[key] = { mark: 'tick', after: after, count: 0 };
        tickOrder.push(key);
      }
      ticks[key].count++;
    });
    tickOrder.forEach(function (k) { out.push(ticks[k]); });
    return out;
  };

  // Octicons: check, x, question, clock and link for heading anchors; your call is a dot
  var ICONS = {
    yes: 'M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z',
    no: 'M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l3.22 3.22a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-3.22 3.22a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06Z',
    ask: 'M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.92 6.085h.001a.749.749 0 1 1-1.342-.67c.169-.339.436-.701.849-.977C6.845 4.16 7.369 4 8 4a2.756 2.756 0 0 1 1.637.525c.503.377.863.965.863 1.725 0 .448-.115.83-.329 1.15-.205.307-.47.513-.692.662-.109.072-.22.138-.313.195l-.006.004a6.24 6.24 0 0 0-.26.16.952.952 0 0 0-.276.245.75.75 0 0 1-1.248-.832c.184-.264.42-.489.692-.661.103-.067.207-.132.313-.195l.007-.004c.1-.061.182-.11.258-.161a.969.969 0 0 0 .277-.245C8.96 6.514 9 6.427 9 6.25a.612.612 0 0 0-.262-.525A1.27 1.27 0 0 0 8 5.5c-.369 0-.595.09-.74.187a1.01 1.01 0 0 0-.34.398ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z',
    later: 'M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Zm7-3.25v2.992l2.028.812a.75.75 0 0 1-.557 1.392l-2.5-1A.751.751 0 0 1 7 8.25v-3.5a.75.75 0 0 1 1.5 0Z',
    your_call: 'M8 5.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5Z',
    link: 'm7.775 3.275 1.25-1.25a3.5 3.5 0 1 1 4.95 4.95l-2.5 2.5a3.5 3.5 0 0 1-4.95 0 .751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018 1.998 1.998 0 0 0 2.83 0l2.5-2.5a2.002 2.002 0 0 0-2.83-2.83l-1.25 1.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042Zm-4.69 9.64a1.998 1.998 0 0 0 2.83 0l1.25-1.25a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042l-1.25 1.25a3.5 3.5 0 1 1-4.95-4.95l2.5-2.5a3.5 3.5 0 0 1 4.95 0 .751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018 1.998 1.998 0 0 0-2.83 0l-2.5 2.5a2.002 2.002 0 0 0 0 2.83Z'
  };

  // What each glyph says to a screen reader, as the page's copy names it
  var VERDICT_LABELS = { yes: 'yes ✓', no: 'no ×', ask: 'ask ?', later: 'later ◷', your_call: 'your call ·' };
  // A note's rule keeps this far from every glyph
  var GLYPH_CLEAR = 4;
  var HEADINGS = 'h1,h2,h3,h4,h5,h6';
  var frame = null;
  var content = null;
  var under = null;
  // The focus wash has its own layer: m hides the marks but not where the keys will act
  var focusLayer = null;
  // The focus outline, above the doc so no block's background hides it
  var focusOver = null;
  var focused = null;
  var hidden = false;
  var over = null;
  var overMarks = null;
  var overAnchors = null;
  var current = null;
  var queued = false;
  // Heading element -> its overlay anchor, rebuilt with the marks
  var anchors = new Map();
  // Icon -> where it was last put, so an unmoved icon is left untouched
  var placed = new WeakMap();
  var lit = null;

  // Doc CSS is global, so every property that could hide or move a host or the doc's box is pinned
  function pin(el, props) {
    for (var k in props) el.style.setProperty(k, props[k], 'important');
  }

  function makeHost(tag, z) {
    var made = BR.shadowHost(tag);
    var host = made.host;
    pin(host, {
      position: 'absolute', inset: '0', width: 'auto', height: 'auto', display: 'block',
      visibility: 'visible', opacity: '1', overflow: 'visible', 'clip-path': 'none', filter: 'none',
      'mask-image': 'none', '-webkit-mask-image': 'none', 'content-visibility': 'visible',
      transform: 'none', translate: 'none', rotate: 'none', scale: 'none', 'offset-path': 'none', zoom: '1',
      'mix-blend-mode': 'normal', margin: '0', padding: '0', 'pointer-events': 'none', 'z-index': String(z)
    });
    var layer = document.createElement('div');
    layer.className = 'layer';
    made.root.appendChild(layer);
    frame.appendChild(host);
    return layer;
  }

  function contain(printing) {
    pin(content, { contain: printing ? 'none' : 'paint' });
  }

  function rendered(el) {
    return !!el && el.getClientRects().length > 0;
  }

  function firstLine(el) {
    var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) { return n.textContent.trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP; }
    });
    var text = walker.nextNode();
    if (text) {
      var range = document.createRange();
      range.selectNodeContents(text);
      var first = range.getClientRects()[0];
      if (first) return first;
    }
    return el.getBoundingClientRect();
  }

  function skipToBlock(el) {
    while (el && (BR.isHost(el) || el.localName === 'style' || !rendered(el))) el = el.nextElementSibling;
    return el;
  }

  // The block after `el`: its next sibling, or the nearest ancestor's inside the doc
  function nextBlock(el) {
    if (!el) return skipToBlock(content.firstElementChild);
    for (var at = el; at && at !== content; at = at.parentElement) {
      var next = skipToBlock(at.nextElementSibling);
      if (next) return next;
    }
    return null;
  }

  /** The first note drawn under `el`, or in the doc slot for null. */
  function firstNote(el) {
    var note = null;
    if (!el) {
      for (var d = frame.firstElementChild; d && d !== content && !note; d = d.nextElementSibling) {
        // Whole doc's line is out of the flow while its rows stand in for it
        if (d.localName === 'birta-flow' && BR.isHost(d) && rendered(d)) note = d;
      }
    } else {
      for (var c = el.lastElementChild; c && BR.isHost(c); c = c.previousElementSibling) note = c;
      for (var at = el; !note && at && at !== content; at = at.parentElement) {
        var sib = at.nextElementSibling;
        if (sib && BR.isHost(sib)) note = sib;
        else if (sib) break;
      }
    }
    return note && rendered(note) ? note : null;
  }

  function svg(d) {
    var ns = 'http://www.w3.org/2000/svg';
    var s = document.createElementNS(ns, 'svg');
    s.setAttribute('viewBox', '0 0 16 16');
    s.setAttribute('aria-hidden', 'true');
    var p = document.createElementNS(ns, 'path');
    p.setAttribute('d', d);
    s.appendChild(p);
    return s;
  }

  function box(attrs, cls, left, top, width, height) {
    var el = document.createElement('div');
    for (var k in attrs) if (attrs[k] != null) el.setAttribute(k, attrs[k]);
    el.className = cls;
    el.style.cssText = 'left:' + left + 'px;top:' + top + 'px;width:' + width + 'px;height:' + height + 'px';
    return el;
  }

  // A block's own lines: notes that close a list item are not part of it
  function ownRect(el) {
    var r = el.getBoundingClientRect();
    var first = null;
    for (var c = el.lastElementChild; c && BR.isHost(c); c = c.previousElementSibling) first = c;
    if (!first) return r;
    var range = document.createRange();
    range.setStart(el, 0);
    range.setEndBefore(first);
    var bottom = range.getBoundingClientRect().bottom;
    return { top: r.top, bottom: bottom, left: r.left, right: r.right, width: r.width, height: bottom - r.top };
  }

  function render() {
    queued = false;
    var now = BR.chrome.now();
    // Approved, the doc reads clean: nothing of the review is drawn
    var clean = current && BR.pure.clean(current, now);
    var marks = current && !clean ? BR.pure.laneMarks(current, { now: now }) : [];
    // Section verdicts on their way wash at once, as their glyph's state is still to come
    if (!clean && BR.pending) {
      BR.pending.sections().forEach(function (sp) {
        if (!marks.some(function (m) { return m.mark === 'wash' && m.section && m.sp === sp; })) marks.push({ mark: 'wash', sp: sp, section: true });
      });
    }
    // A glyph shares its block's lane with the bar, so it moves to the slot beside it
    var barred = {};
    marks.forEach(function (m) { if (m.mark === 'bar') barred[m.sp] = m; });
    var F = frame.getBoundingClientRect();
    var C = content.getBoundingClientRect();
    var narrow = window.innerWidth < 768;
    var laneX = narrow ? 12 : 20;
    var size = narrow ? 12 : 16;
    var glyphs = {};
    marks.forEach(function (m) { if (m.mark === 'glyph') glyphs[m.sp] = true; });
    // The narrow lane is only as wide as a glyph, a gap and a bar, so a shared bar moves to its
    // edge, and a double rule there narrows to the agent's single rule to leave the glyph room
    function squeezed(m) { return narrow && glyphs[m.sp] && m.style === 'double'; }
    function barWidth(m) { return m.style === 'double' ? (squeezed(m) ? 2 : 6) : m.weight || 2; }
    function barX(sp, w) {
      if (narrow && glyphs[sp]) return laneX + 8 - w;
      // A 1 px bar starts on a whole pixel, so it paints its colour rather than a blur across two
      return w === 1 ? laneX - 1 : laneX - w / 2;
    }
    var stacked = {};
    var glyphBoxes = [];
    var underOut = document.createDocumentFragment();
    var overOut = document.createDocumentFragment();

    // All reads happen before the layers change, so the doc is laid out once
    marks.forEach(function (m) {
      if (m.mark === 'tick') {
        var after = m.after === null ? null : BR.blocks.of(m.after);
        if (m.after !== null && !rendered(after)) return;
        var next = nextBlock(after);
        var top = after ? ownRect(after).bottom : F.top;
        var bottom = next ? next.getBoundingClientRect().top : F.bottom;
        // A note under the block starts below the tick, so the tick never crosses its rule
        var note = firstNote(after);
        if (note) bottom = Math.min(bottom, note.getBoundingClientRect().top);
        overOut.appendChild(box({ 'data-mark': 'tick', 'data-after': m.after, 'data-count': m.count }, 'tick',
          laneX - 6, (top + bottom) / 2 - 1 - F.top, 12, 2));
        return;
      }
      var el = BR.blocks.of(m.sp);
      if (!rendered(el)) return;
      var r = ownRect(el);
      if (m.mark === 'bar') {
        var w = barWidth(m);
        overOut.appendChild(box({ 'data-mark': 'bar', 'data-sp': m.sp, 'data-style': m.style, 'data-role': m.role, 'data-weight': m.weight,
          'data-check': m.check },
          'bar ' + m.style + ' ' + m.role + (squeezed(m) ? ' squeezed' : '') + (m.fresh ? ' fresh' : '') + (m.weight === 1 ? ' thin' : ''),
          barX(m.sp, w), r.top - F.top, w, r.height));
      } else if (m.mark === 'veil') {
        // Dims the words a no asks to cut, without touching the doc's own nodes
        var vr = m.section ? sectionRect(el, C) : r;
        var vl = Math.max(el.localName === 'li' ? el.parentElement.getBoundingClientRect().left : vr.left, C.left) - 2;
        var vright = Math.min(vr.right, C.right) + 2;
        overOut.appendChild(box({ 'data-mark': 'veil', 'data-sp': m.sp }, 'veil', vl - F.left, vr.top - F.top, vright - vl, vr.bottom - vr.top));
      } else if (m.mark === 'wash') {
        // An item's wash reaches over its marker, and a wide table's stops at the doc's edge
        var wr = m.section ? sectionRect(el, C) : r;
        var left = Math.max(el.localName === 'li' ? el.parentElement.getBoundingClientRect().left : wr.left, C.left) - 8;
        var right = Math.min(m.section ? C.right : r.right, C.right) + 8;
        underOut.appendChild(box({ 'data-mark': 'wash', 'data-sp': m.sp }, 'wash',
          left - F.left, wr.top - F.top, right - left, wr.bottom - wr.top));
      } else if (m.mark === 'glyph') {
        var k = stacked[m.sp] = (stacked[m.sp] || 0) + 1;
        var line = firstLine(el);
        var gx = barred[m.sp] ? barX(m.sp, barWidth(barred[m.sp])) - 2 - size : laneX - size / 2;
        var gy = (line.top + line.bottom) / 2 - size / 2 + (k - 1) * (size + 2) - F.top;
        var g = box({ 'data-mark': 'glyph', 'data-sp': m.sp, 'data-verdict': m.verdict, 'data-role': m.role, 'data-n': m.n,
          'aria-label': VERDICT_LABELS[m.verdict] || m.verdict, role: 'img' }, 'glyph ' + m.role, gx, gy, size, size);
        g.appendChild(svg(ICONS[m.verdict] || ICONS.no));
        overOut.appendChild(g);
        glyphBoxes.push({ left: gx, right: gx + size, top: gy, bottom: gy + size });
      } else if (m.mark === 'revert') {
        // Stacks under any glyph on its line, so the two never cover each other
        var rk = stacked[m.sp] = (stacked[m.sp] || 0) + 1;
        var rl = firstLine(el);
        var rx = laneX - size / 2;
        var ry = (rl.top + rl.bottom) / 2 - size / 2 + (rk - 1) * (size + 2) - F.top;
        var head = box({ 'data-mark': 'revert', 'data-sp': m.sp, 'aria-label': 'reverted by you', role: 'img' }, 'revert', rx, ry, size, size);
        head.textContent = '↺';
        overOut.appendChild(head);
        glyphBoxes.push({ left: rx, right: rx + size, top: ry, bottom: ry + size });
      }
    });

    // A note's rule runs down the lane beside it, and a folded note leaves a dot in the right padding
    // A rule breaks around a glyph in its way, so the two never touch
    var ruleX = laneX - 1;
    var inWay = glyphBoxes.filter(function (g) { return g.left < ruleX + 2 + GLYPH_CLEAR && g.right > ruleX - GLYPH_CLEAR; });
    BR.flow.rules().forEach(function (f) {
      var r = f.el.getBoundingClientRect();
      if (!r.height) return;
      clear(r.top - F.top, r.bottom - F.top, inWay).forEach(function (seg) {
        overOut.appendChild(box({ 'data-mark': 'rule', 'data-style': f.style, 'data-role': f.role },
          'rule ' + f.style + ' ' + f.role, ruleX, seg[0], 2, seg[1] - seg[0]));
      });
    });
    BR.flow.dots().forEach(function (d) {
      var el = BR.blocks.of(d.sp);
      if (!rendered(el)) return;
      var line = firstLine(el);
      overOut.appendChild(box({ 'data-mark': 'dot', 'data-sp': d.sp, 'data-role': d.role, title: d.title }, 'dot ' + d.role,
        F.width - 24, (line.top + line.bottom) / 2 - 4 - F.top, 8, 8));
    });

    var places = [];
    content.querySelectorAll(HEADINGS).forEach(function (h) {
      var link = h.querySelector('a.anchor');
      if (!link || !h.hasAttribute('data-sourcepos') || !rendered(h)) return;
      var line = firstLine(h);
      // Under 768px the padding is only as wide as the icon
      var left = Math.max(h.getBoundingClientRect().left - 20, F.left);
      places.push({ h: h, link: link, left: left - F.left, top: (line.top + line.bottom) / 2 - 8 - F.top });
    });

    drawFocus(F, C);
    highlight();
    under.replaceChildren(underOut);
    overMarks.replaceChildren(overOut);
    placeAnchors(places);
  }

  /** The parts of `top`..`bottom` at least GLYPH_CLEAR from every box in `boxes`. */
  function clear(top, bottom, boxes) {
    var segs = [[top, bottom]];
    boxes.forEach(function (b) {
      var a = b.top - GLYPH_CLEAR;
      var z = b.bottom + GLYPH_CLEAR;
      segs = segs.reduce(function (out, s) {
        if (z <= s[0] || a >= s[1]) out.push(s);
        else {
          if (a > s[0]) out.push([s[0], a]);
          if (z < s[1]) out.push([z, s[1]]);
        }
        return out;
      }, []);
    });
    return segs.filter(function (s) { return s[1] - s[0] >= 1; });
  }

  // The outline where the keys act, and a heading's section washed as its scope; a move redraws them on its own
  function drawFocus(F, C) {
    var scope = document.createDocumentFragment();
    var outline = document.createDocumentFragment();
    var doc = BR.flow.isDocStop(focused) ? BR.flow.docRect() : null;
    if (doc) {
      // Whole doc: its line and rows, across the doc's width
      outline.appendChild(box({ 'data-mark': 'focus', 'data-target': 'doc' }, 'focus',
        C.left - 8 - F.left, doc.top - 2 - F.top, C.width + 16, doc.bottom - doc.top + 4));
    } else if (focused && focused.isConnected && BR.flow.removalOf(focused)) {
      // A removal stop: its gap, or its row while it is open there
      var gone = BR.flow.removalOf(focused);
      var gr = focused.getBoundingClientRect();
      outline.appendChild(box({ 'data-mark': 'focus', 'data-target': 'removal', 'data-after': gone.after, 'data-change': gone.change }, 'focus',
        C.left - 8 - F.left, gr.top - 2 - F.top, C.width + 16, gr.height + 4));
    } else if (focused && focused.isConnected && rendered(focused)) {
      var own = ownRect(focused);
      var fl = Math.max(focused.localName === 'li' ? focused.parentElement.getBoundingClientRect().left : own.left, C.left) - 8;
      var fright = Math.min(own.right, C.right) + 8;
      outline.appendChild(box({ 'data-mark': 'focus', 'data-sp': focused.getAttribute('data-sourcepos'),
        'data-target': focused.localName === 'summary' ? 'summary' : BR.isHost(focused) ? 'blank' : null }, 'focus',
        fl - F.left, own.top - 2 - F.top, fright - fl, own.bottom - own.top + 4));
      // A title's verdicts are on its own line, so its wash is too
      if (/^h[1-6]$/.test(focused.localName) && !BR.pointing.isTitle(focused)) {
        var sr = sectionRect(focused, C);
        scope.appendChild(box({ 'data-mark': 'scope', 'data-sp': focused.getAttribute('data-sourcepos') }, 'scope',
          fl - F.left, sr.top - F.top, fright - fl, sr.bottom - sr.top));
      }
    }
    focusLayer.replaceChildren(scope);
    focusOver.replaceChildren(outline);
  }

  // Icons are kept across redraws, so a click that spans one still lands on the same element
  function placeAnchors(places) {
    var fresh = new Map();
    places.forEach(function (p) {
      var icon = anchors.get(p.h);
      if (!icon) {
        icon = box({}, 'anchor', 0, 0, 16, 16);
        icon.setAttribute('data-mark', 'anchor');
        icon.appendChild(svg(ICONS.link));
        icon.addEventListener('mouseenter', function () { light(p.h); });
        icon.addEventListener('mouseleave', function () { if (lit === p.h) light(null); });
        icon.addEventListener('click', function () { location.hash = p.link.getAttribute('href'); });
        overAnchors.appendChild(icon);
      }
      var sp = p.h.getAttribute('data-sourcepos');
      if (icon.getAttribute('data-sp') !== sp) icon.setAttribute('data-sp', sp);
      var at = p.left + ',' + p.top;
      if (placed.get(icon) !== at) {
        placed.set(icon, at);
        icon.style.left = p.left + 'px';
        icon.style.top = p.top + 'px';
      }
      icon.classList.toggle('on', lit === p.h);
      fresh.set(p.h, icon);
    });
    anchors.forEach(function (icon, h) {
      if (!fresh.has(h)) icon.remove();
    });
    anchors = fresh;
  }

  // Only the icons that change are touched, so hovering never rebuilds the lane
  function light(h) {
    if (h === lit) return;
    if (lit && anchors.get(lit)) anchors.get(lit).classList.remove('on');
    lit = h;
    if (lit && anchors.get(lit)) anchors.get(lit).classList.add('on');
  }

  // A heading's box grows to the end of its section: the last block before a heading as high
  function sectionRect(el, C) {
    var r = ownRect(el);
    var level = /^h([1-6])$/.exec(el.localName);
    var bottom = r.bottom;
    if (level && el.parentElement === content) {
      for (var next = el.nextElementSibling; next; next = next.nextElementSibling) {
        var h = /^h([1-6])$/.exec(next.localName);
        if (h && Number(h[1]) <= Number(level[1])) break;
        if (!BR.isHost(next) && rendered(next)) bottom = Math.max(bottom, next.getBoundingClientRect().bottom);
      }
    }
    return { top: r.top, bottom: bottom, left: r.left, right: Math.min(r.right, C.right) };
  }

  function reading() {
    return document.body.classList.contains('reading-mode');
  }

  // Underlines from the server's offsets; a browser without the Highlight API keeps the bars
  function highlight() {
    if (typeof CSS === 'undefined' || !CSS.highlights || typeof Highlight === 'undefined') return;
    var ranges = [];
    if (current && !hidden && !reading() && !BR.pure.clean(current, BR.chrome.now())) {
      (current.items || []).forEach(function (i) {
        var loc = i.at && i.at.loc;
        if (!loc || !loc.off || i.status === 'retracted') return;
        var r = BR.quote.paint(loc.sp, loc.off);
        if (r) ranges.push(r);
      });
    }
    if (!ranges.length) {
      CSS.highlights.delete('birta-quote');
      return;
    }
    var hl = new Highlight();
    ranges.forEach(function (r) { hl.add(r); });
    CSS.highlights.set('birta-quote', hl);
  }

  function schedule() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(render);
  }

  function headingOf(target) {
    return target && target.closest ? target.closest(HEADINGS) : null;
  }

  BR.lane = {
    init: function () {
      frame = document.getElementById('lane-frame');
      content = document.getElementById('content');
      pin(frame, { position: 'relative', isolation: 'isolate' });
      pin(content, {
        position: 'relative', 'z-index': '1', 'background-color': 'transparent', margin: '0', padding: '0'
      });
      var print = window.matchMedia('print');
      contain(print.matches);
      print.addEventListener('change', function (e) { contain(e.matches); });
      window.addEventListener('beforeprint', function () { contain(true); });
      window.addEventListener('afterprint', function () { contain(print.matches); });
      under = makeHost('birta-under', 0);
      focusLayer = document.createElement('div');
      focusLayer.className = 'layer';
      under.parentNode.insertBefore(focusLayer, under);
      over = makeHost('birta-over', 2);
      focusOver = document.createElement('div');
      focusOver.className = 'layer';
      over.parentNode.appendChild(focusOver);
      overMarks = document.createElement('div');
      overAnchors = document.createElement('div');
      overMarks.className = overAnchors.className = 'group';
      over.append(overMarks, overAnchors);

      var ro = new ResizeObserver(schedule);
      ro.observe(content);
      ro.observe(frame);
      window.addEventListener('resize', schedule);
      if (document.fonts) {
        document.fonts.ready.then(schedule);
        document.fonts.addEventListener('loadingdone', schedule);
      }
      content.addEventListener('load', schedule, true);

      content.addEventListener('mouseover', function (e) { light(headingOf(e.target)); });
      content.addEventListener('mouseleave', function (e) {
        // Moving onto the heading's own icon keeps it lit
        var to = e.relatedTarget;
        if (!(to && to.localName === 'birta-over')) light(null);
      });
      content.addEventListener('focusin', function (e) {
        if (e.target.matches && e.target.matches('a.anchor')) light(headingOf(e.target));
      });
      content.addEventListener('focusout', function (e) {
        if (e.target.matches && e.target.matches('a.anchor')) light(null);
      });
      // Reading mode hides marks and notes; m is its own toggle and survives leaving it
      var syncReading = function () {
        var on = reading();
        [under, focusLayer, over, focusOver].forEach(function (layer) { layer.classList.toggle('reading', on); });
        BR.flow.hide(on);
        highlight();
        schedule();
      };
      new MutationObserver(syncReading).observe(document.body, { attributes: true, attributeFilter: ['class'] });
      syncReading();
      render();
    },
    focus: function (el) {
      focused = el;
      drawFocus(frame.getBoundingClientRect(), content.getBoundingClientRect());
    },
    toggle: function () {
      hidden = !hidden;
      under.classList.toggle('off', hidden);
      over.classList.toggle('off', hidden);
      // Link dots are marks too, and live in the page's own CSS
      if (hidden) document.documentElement.setAttribute('data-br-marks', 'off');
      else document.documentElement.removeAttribute('data-br-marks');
      highlight();
    },
    /** Measures again on the next frame, after the flow moved blocks. */
    redraw: function () { schedule(); },
    draw: function (state) {
      current = state;
      render();
    }
  };
})();
