var BR = typeof BR === 'undefined' ? {} : BR;
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
  BR.pure.laneMarks = function (state) {
    var out = [];
    (state.items || [])
      .filter(function (i) {
        return i.verdict && i.status !== 'retracted' && i.at && i.at.loc && i.at.loc.sp;
      })
      .sort(function (a, b) { return a.n - b.n; })
      .forEach(function (i) {
        // A withdrawn round hands its notes back as drafts
        var yours = i.status === 'draft' || i.status === 'withdrawn';
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
    var host = document.createElement(tag);
    pin(host, {
      position: 'absolute', inset: '0', width: 'auto', height: 'auto', display: 'block',
      visibility: 'visible', opacity: '1', overflow: 'visible', 'clip-path': 'none', filter: 'none',
      'mask-image': 'none', '-webkit-mask-image': 'none', 'content-visibility': 'visible',
      transform: 'none', margin: '0', padding: '0', 'pointer-events': 'none', 'z-index': String(z)
    });
    var root = host.attachShadow({ mode: 'closed' });
    var style = document.createElement('style');
    style.textContent = BR.shadowCss || '';
    root.appendChild(style);
    var layer = document.createElement('div');
    layer.className = 'layer';
    root.appendChild(layer);
    BR.FLOW_HOSTS.add(host);
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

  function render() {
    queued = false;
    var marks = current ? BR.pure.laneMarks(current) : [];
    var bySp = {};
    content.querySelectorAll('[data-sourcepos]').forEach(function (el) {
      // Document order puts a list item after a list that shares its sp
      bySp[el.getAttribute('data-sourcepos')] = el;
    });
    var F = frame.getBoundingClientRect();
    var C = content.getBoundingClientRect();
    var narrow = window.innerWidth < 768;
    var laneX = narrow ? 8 : 20;
    var size = narrow ? 12 : 16;
    var stacked = {};
    var underOut = document.createDocumentFragment();
    var overOut = document.createDocumentFragment();

    // All reads happen before the layers change, so the doc is laid out once
    marks.forEach(function (m) {
      if (m.mark === 'tick') {
        var after = m.after === null ? null : bySp[m.after];
        if (m.after !== null && !rendered(after)) return;
        var next = nextBlock(after);
        var top = after ? after.getBoundingClientRect().bottom : F.top;
        var bottom = next ? next.getBoundingClientRect().top : F.bottom;
        overOut.appendChild(box({ 'data-mark': 'tick', 'data-after': m.after, 'data-count': m.count }, 'tick',
          laneX - 6, (top + bottom) / 2 - 1 - F.top, 12, 2));
        return;
      }
      var el = bySp[m.sp];
      if (!rendered(el)) return;
      var r = el.getBoundingClientRect();
      if (m.mark === 'bar') {
        var w = m.style === 'double' ? 6 : 2;
        overOut.appendChild(box({ 'data-mark': 'bar', 'data-sp': m.sp, 'data-style': m.style, 'data-role': m.role },
          'bar ' + m.style + ' ' + m.role, laneX - w / 2, r.top - F.top, w, r.height));
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
          'glyph ' + m.role, laneX - size / 2, (line.top + line.bottom) / 2 - size / 2 + (k - 1) * (size + 2) - F.top, size, size);
        g.appendChild(svg(ICONS[m.verdict] || ICONS.no));
        overOut.appendChild(g);
      }
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

    under.replaceChildren(underOut);
    overMarks.replaceChildren(overOut);
    placeAnchors(places);
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
      document.addEventListener('keydown', function (e) {
        if (e.key !== 'm' || e.altKey || e.ctrlKey || e.metaKey || e.repeat) return;
        var t = e.target;
        if (t && (t.localName === 'input' || t.localName === 'textarea' || t.localName === 'select' || t.isContentEditable)) return;
        var off = !under.classList.contains('off');
        under.classList.toggle('off', off);
        over.classList.toggle('off', off);
      });
      render();
    },
    draw: function (state) {
      current = state;
      render();
    }
  };
})();
