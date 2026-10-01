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
   * `{mark:'glyph', sp, verdict, role, n}`, `{mark:'bar', sp, style, role}`, `{mark:'wash', sp}`
   * and `{mark:'tick', after, count}`, where `after` is null above the first block.
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
    (state.items || [])
      .filter(function (i) {
        return i.verdict && i.status !== 'retracted' && i.at && i.at.loc && i.at.loc.sp;
      })
      .sort(function (a, b) { return a.n - b.n; })
      .forEach(function (i) {
        // A withdrawn round hands its notes back as drafts
        var yours = i.status === 'draft' || i.status === 'withdrawn' || !!held[i.n];
        out.push({ mark: 'glyph', sp: i.at.loc.sp, verdict: i.verdict, role: yours ? 'you' : 'agent', n: i.n });
      });
    // A rewrite speaks through its own row, so per-block marks would only repeat it
    if (state.rewrite) return out;

    var bySp = {};
    var order = [];
    var settled = {};
    (state.changes || []).forEach(function (c) {
      if (c.settled) settled[c.id] = true;
      var sp = c.range && c.range.sp;
      if (!sp || c.settled) return;
      if (!bySp[sp]) {
        bySp[sp] = [];
        order.push(sp);
      }
      bySp[sp].push(c);
    });
    order.forEach(function (sp) {
      var all = bySp[sp].slice().sort(newest);
      var unseen = all.filter(function (c) { return !c.seen; });
      var pick = unseen.length ? unseen : all;
      var win = pick[pick.length - 1];
      out.push({ mark: 'bar', sp: sp, style: barStyle(win), role: win.attr === 'human' ? 'you' : 'agent' });
      if (unseen.length) out.push({ mark: 'wash', sp: sp });
    });

    var ticks = {};
    var tickOrder = [];
    (state.deletions || []).forEach(function (d) {
      if (d.kind !== 'block' || settled[d.change]) return;
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

  // Octicons: check, x, question, clock, arrow-right, and link for heading anchors
  var ICONS = {
    yes: 'M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z',
    no: 'M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l3.22 3.22a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-3.22 3.22a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06Z',
    ask: 'M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.92 6.085h.001a.749.749 0 1 1-1.342-.67c.169-.339.436-.701.849-.977C6.845 4.16 7.369 4 8 4a2.756 2.756 0 0 1 1.637.525c.503.377.863.965.863 1.725 0 .448-.115.83-.329 1.15-.205.307-.47.513-.692.662-.109.072-.22.138-.313.195l-.006.004a6.24 6.24 0 0 0-.26.16.952.952 0 0 0-.276.245.75.75 0 0 1-1.248-.832c.184-.264.42-.489.692-.661.103-.067.207-.132.313-.195l.007-.004c.1-.061.182-.11.258-.161a.969.969 0 0 0 .277-.245C8.96 6.514 9 6.427 9 6.25a.612.612 0 0 0-.262-.525A1.27 1.27 0 0 0 8 5.5c-.369 0-.595.09-.74.187a1.01 1.01 0 0 0-.34.398ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z',
    later: 'M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Zm7-3.25v2.992l2.028.812a.75.75 0 0 1-.557 1.392l-2.5-1A.751.751 0 0 1 7 8.25v-3.5a.75.75 0 0 1 1.5 0Z',
    your_call: 'M8.22 2.97a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042l2.97-2.97H3.75a.75.75 0 0 1 0-1.5h7.44L8.22 4.03a.75.75 0 0 1 0-1.06Z',
    link: 'm7.775 3.275 1.25-1.25a3.5 3.5 0 1 1 4.95 4.95l-2.5 2.5a3.5 3.5 0 0 1-4.95 0 .751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018 1.998 1.998 0 0 0 2.83 0l2.5-2.5a2.002 2.002 0 0 0-2.83-2.83l-1.25 1.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042Zm-4.69 9.64a1.998 1.998 0 0 0 2.83 0l1.25-1.25a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042l-1.25 1.25a3.5 3.5 0 1 1-4.95-4.95l2.5-2.5a3.5 3.5 0 0 1 4.95 0 .751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018 1.998 1.998 0 0 0-2.83 0l-2.5 2.5a2.002 2.002 0 0 0 0 2.83Z'
  };

  var HEADINGS = 'h1,h2,h3,h4,h5,h6';
  var frame = null;
  var content = null;
  var under = null;
  // The focus wash has its own layer: m hides the marks but not where the keys will act
  var focusLayer = null;
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
        if (d.localName === 'birta-flow' && BR.isHost(d)) note = d;
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
    var marks = current ? BR.pure.laneMarks(current, { now: BR.chrome.now() }) : [];
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
    function barWidth(m) { return m.style === 'double' && !squeezed(m) ? 6 : 2; }
    function barX(sp, w) {
      return narrow && glyphs[sp] ? laneX + 8 - w : laneX - w / 2;
    }
    var stacked = {};
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
        overOut.appendChild(box({ 'data-mark': 'bar', 'data-sp': m.sp, 'data-style': m.style, 'data-role': m.role },
          'bar ' + m.style + ' ' + m.role + (squeezed(m) ? ' squeezed' : ''), barX(m.sp, w), r.top - F.top, w, r.height));
      } else if (m.mark === 'wash') {
        // An item's wash reaches over its marker, and a wide table's stops at the doc's edge
        var left = Math.max(el.localName === 'li' ? el.parentElement.getBoundingClientRect().left : r.left, C.left) - 8;
        var right = Math.min(r.right, C.right) + 8;
        underOut.appendChild(box({ 'data-mark': 'wash', 'data-sp': m.sp }, 'wash',
          left - F.left, r.top - F.top, right - left, r.height));
      } else if (m.mark === 'glyph') {
        var k = stacked[m.sp] = (stacked[m.sp] || 0) + 1;
        var line = firstLine(el);
        var g = box({ 'data-mark': 'glyph', 'data-sp': m.sp, 'data-verdict': m.verdict, 'data-role': m.role, 'data-n': m.n },
          'glyph ' + m.role, barred[m.sp] ? barX(m.sp, barWidth(barred[m.sp])) - 2 - size : laneX - size / 2, (line.top + line.bottom) / 2 - size / 2 + (k - 1) * (size + 2) - F.top, size, size);
        g.appendChild(svg(ICONS[m.verdict] || ICONS.no));
        overOut.appendChild(g);
      }
    });

    // A note's rule runs down the lane beside it, and a folded note leaves a dot in the right padding
    BR.flow.rules().forEach(function (f) {
      var r = f.el.getBoundingClientRect();
      if (!r.height) return;
      overOut.appendChild(box({ 'data-mark': 'rule', 'data-style': f.style, 'data-role': f.role },
        'rule ' + f.style + ' ' + f.role, laneX - 1, r.top - F.top, 2, r.height));
    });
    BR.flow.dots().forEach(function (d) {
      var el = BR.blocks.of(d.sp);
      if (!rendered(el)) return;
      var line = firstLine(el);
      overOut.appendChild(box({ 'data-mark': 'dot', 'data-sp': d.sp, 'data-role': d.role }, 'dot ' + d.role,
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

  // The wash where the keys act, which a move redraws on its own
  function drawFocus(F, C) {
    var out = document.createDocumentFragment();
    if (focused && focused.isConnected && rendered(focused)) {
      var fr = sectionRect(focused, C);
      var fl = Math.max(focused.localName === 'li' ? focused.parentElement.getBoundingClientRect().left : fr.left, C.left) - 8;
      var fright = Math.min(fr.right, C.right) + 8;
      out.appendChild(box({ 'data-mark': 'focus', 'data-sp': focused.getAttribute('data-sourcepos'),
        'data-target': focused.localName === 'summary' ? 'summary' : null }, 'focus',
        fl - F.left, fr.top - F.top, fright - fl, fr.bottom - fr.top));
    }
    focusLayer.replaceChildren(out);
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
    if (current && !hidden && !reading()) {
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
        [under, focusLayer, over].forEach(function (layer) { layer.classList.toggle('reading', on); });
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
