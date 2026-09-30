var BR = typeof BR === 'undefined' ? {} : BR;

// A selection's text as the server's anchor resolver reads it: the owner block's rendered
// text with whitespace collapsed, counted in code points.
BR.quote = (function () {
  var OWNER = 'p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,pre';
  // Nested blocks are their own owners, or no one's
  var BLOCK = 'p,li,h1,h2,h3,h4,h5,h6,td,th,dt,dd,ul,ol,table,blockquote,div,pre,section,details';
  var ATOMIC = '[data-math-style],sup.footnote-ref,img,svg';
  var DIAGRAM = 'pre.mermaid,pre[data-math-style]';
  var OBJ = '￼';
  var CONTEXT = 32;
  // JS \s, the same set the server collapses
  var WS = /\s/;

  function content() {
    return document.getElementById('content');
  }

  function isOwner(el) {
    return el.matches(OWNER) && el.hasAttribute('data-sourcepos');
  }

  function ownerOf(node) {
    var el = node.nodeType === 1 ? node : node.parentElement;
    var root = content();
    if (!el || !root || !root.contains(el)) return null;
    for (; el && el !== root; el = el.parentElement) {
      if (isOwner(el)) return el;
    }
    return null;
  }

  function hidden(el) {
    if (el.hasAttribute('hidden') || el.getAttribute('aria-hidden') === 'true') return true;
    var style = getComputedStyle(el);
    return style.display === 'none' || style.visibility === 'hidden';
  }

  /** The owner's text pieces in order: `{text}` nodes, and `{el, ch}` for one-character elements. */
  function pieces(owner) {
    var out = [];
    if (owner.matches(DIAGRAM)) return [{ el: owner, ch: OBJ }];
    (function walk(node) {
      for (var n = node.firstChild; n; n = n.nextSibling) {
        if (n.nodeType === 3) {
          out.push({ text: n });
        } else if (n.nodeType === 1) {
          if (n.matches(BLOCK) || n.matches('a.footnote-backref') || hidden(n)) continue;
          if (n.matches(ATOMIC)) out.push({ el: n, ch: OBJ });
          else if (n.tagName === 'BR') out.push({ el: n, ch: ' ' });
          else walk(n);
        }
      }
    })(owner);
    return out;
  }

  /**
   * The owner's text `T` as code points, each with where it sits in the DOM and whether
   * `inRange` claims it. Outside `pre`, whitespace runs collapse to one space and ends trim.
   */
  function units(owner, inRange) {
    var keep = owner.tagName === 'PRE';
    var out = [];
    pieces(owner).forEach(function (p) {
      if (p.el) {
        push(p.ch, { el: p.el }, inRange ? inRange(p.el) : false);
        return;
      }
      var at = 0;
      for (var ch of p.text.data) {
        var end = at + ch.length;
        push(ch, { node: p.text, start: at, end: end }, inRange ? inRange(p.text, at, end) : false);
        at = end;
      }
    });
    if (!keep) {
      while (out.length && out[out.length - 1].ch === ' ') out.pop();
    }
    return out;

    function push(ch, where, picked) {
      if (!keep && WS.test(ch)) {
        var last = out[out.length - 1];
        if (!last) return;
        if (last.ch === ' ') {
          last.picked = last.picked || picked;
          return;
        }
        ch = ' ';
      }
      out.push({ ch: ch, where: where, picked: picked });
    }
  }

  function textOf(list, a, b) {
    return list.slice(a, b).map(function (u) { return u.ch; }).join('');
  }

  /** The node a boundary point sits on: the child after it, or the container. */
  function nodeAt(container, offset, before) {
    if (container.nodeType !== 1) return container;
    var child = container.childNodes[before ? offset - 1 : offset];
    return child || container;
  }

  /** Whether `range` covers code units `[a, b)` of text node `node`, or the element `node`. */
  function covers(range, node, a, b) {
    if (a === undefined) return range.intersectsNode(node);
    var from = node === range.startContainer ? range.startOffset : (range.comparePoint(node, 0) >= 0 ? 0 : node.length);
    var to = node === range.endContainer ? range.endOffset : (range.comparePoint(node, node.length) <= 0 ? node.length : 0);
    return a >= from && b <= to;
  }

  function firstOwnerIn(range) {
    var root = content();
    if (!root) return null;
    var all = root.querySelectorAll(OWNER);
    for (var i = 0; i < all.length; i++) {
      if (isOwner(all[i]) && range.intersectsNode(all[i])) return all[i];
    }
    return null;
  }

  function collapse(s) {
    return s.replace(/\s+/g, ' ').trim();
  }

  /**
   * What a selection points at: `{sp, quote, pre, suf, off, tl, crossBlock}`, with offsets in
   * code points of the start owner's text. A selection in raw HTML has no owner, so only its
   * text goes: `{sp: null, quote}`. Null when nothing is selected.
   */
  function fromRange(range) {
    if (!range || range.collapsed) return null;
    var start = nodeAt(range.startContainer, range.startOffset, false);
    // A drag that starts between blocks belongs to the first block it reaches
    var owner = ownerOf(start) || firstOwnerIn(range);
    if (!owner) {
      var root = content();
      if (!root || !root.contains(start)) return null;
      var raw = collapse(range.toString());
      return raw ? { sp: null, quote: raw, pre: '', suf: '', off: null, tl: null, crossBlock: false } : null;
    }
    var list = units(owner, function (node, a, b) { return covers(range, node, a, b); });
    var a = 0;
    while (a < list.length && !list[a].picked) a++;
    var b = list.length;
    while (b > a && !list[b - 1].picked) b--;
    if (owner.tagName !== 'PRE') {
      while (a < b && list[a].ch === ' ') a++;
      while (b > a && list[b - 1].ch === ' ') b--;
    }
    if (a >= b) return null;
    var end = ownerOf(nodeAt(range.endContainer, range.endOffset, true));
    return {
      sp: owner.getAttribute('data-sourcepos'),
      quote: textOf(list, a, b),
      pre: textOf(list, Math.max(0, a - CONTEXT), a),
      suf: textOf(list, b, b + CONTEXT),
      off: a,
      tl: list.length,
      crossBlock: end !== owner
    };
  }

  /** A DOM range over code points `[off[0], off[1])` of the owner `sp`, or null if it's gone. */
  function paint(sp, off) {
    var root = content();
    if (!root) return null;
    var all = root.querySelectorAll('[data-sourcepos]');
    for (var i = 0; i < all.length; i++) {
      if (all[i].getAttribute('data-sourcepos') !== sp || !all[i].matches(OWNER)) continue;
      var list = units(all[i]);
      var a = off[0], b = off[1];
      if (!(a >= 0 && a < b && b <= list.length)) return null;
      var range = document.createRange();
      var first = list[a].where, last = list[b - 1].where;
      if (first.node) range.setStart(first.node, first.start);
      else range.setStartBefore(first.el);
      if (last.node) range.setEnd(last.node, last.end);
      else range.setEndAfter(last.el);
      return range;
    }
    return null;
  }

  return { fromRange: fromRange, paint: paint };
})();
