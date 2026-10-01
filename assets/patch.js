    // --- Block patcher ---
    // Applies a fresh render by swapping only the top-level blocks that changed,
    // so unchanged blocks keep their nodes, selection, typeset math and diagrams.
    var birtaPatch = (function() {
      // Key of the rendered HTML each live block came from. The live DOM can't be
      // re-keyed: KaTeX, mermaid and the checkbox wiring all mutate it.
      var keys = new WeakMap();
      var root = null;
      var opts = {};
      var composing = false;
      var pending = null;
      // Block kept in place by the last patch, re-applied on late growth until the user scrolls.
      var held = null;
      var ownScrollY = null;

      /**
       * Longest common prefix, then the longest common suffix of what remains,
       * so the two never overlap on repeated keys. The middle is swapped wholesale.
       */
      function planPatch(oldKeys, newKeys) {
        var oldLen = oldKeys.length;
        var newLen = newKeys.length;
        var room = Math.min(oldLen, newLen);
        var prefix = 0;
        while (prefix < room && oldKeys[prefix] === newKeys[prefix]) prefix++;
        var suffix = 0;
        while (suffix < room - prefix && oldKeys[oldLen - 1 - suffix] === newKeys[newLen - 1 - suffix]) suffix++;
        return { prefix: prefix, suffix: suffix };
      }

      // Sourcepos shifts with every edit above, and attribute order is not stable across renders.
      function keyOf(node) {
        if (node.nodeType === 3) return JSON.stringify(node.data);
        if (node.nodeType !== 1) return '<!' + node.nodeType + JSON.stringify(node.data || '') + '>';
        var attrs = [];
        for (var i = 0; i < node.attributes.length; i++) {
          var a = node.attributes[i];
          if (a.name !== 'data-sourcepos') attrs.push(a.name + '=' + JSON.stringify(a.value));
        }
        attrs.sort();
        var out = '<' + node.nodeName + ' ' + attrs.join(' ') + '>';
        var kids = node.nodeName === 'TEMPLATE' ? node.content.childNodes : node.childNodes;
        for (var j = 0; j < kids.length; j++) out += keyOf(kids[j]);
        return out + '</>';
      }

      function isBlank(node) {
        return node.nodeType === 3 && !/\S/.test(node.data);
      }

      function blocksOf(parent) {
        var out = [];
        for (var n = parent.firstChild; n; n = n.nextSibling) {
          // Review's in-flow hosts are not the doc's blocks
          if (!isBlank(n) && !api.skip(n)) out.push(n);
        }
        return out;
      }

      function keyOfLive(node) {
        var k = keys.get(node);
        if (k === undefined) {
          k = keyOf(node);
          keys.set(node, k);
        }
        return k;
      }

      function withSourcepos(node) {
        if (node.nodeType !== 1) return [];
        var list = Array.prototype.slice.call(node.querySelectorAll('[data-sourcepos]'));
        if (node.hasAttribute('data-sourcepos')) list.unshift(node);
        return list;
      }

      // False when the live block no longer lines up with its render, so it must be replaced.
      function copySourcepos(live, fresh) {
        var from = withSourcepos(fresh);
        var to = withSourcepos(live);
        if (from.length !== to.length) return false;
        for (var i = 0; i < from.length; i++) {
          if (from[i].nodeName !== to[i].nodeName) return false;
        }
        for (var j = 0; j < from.length; j++) {
          var sp = from[j].getAttribute('data-sourcepos');
          if (to[j].getAttribute('data-sourcepos') !== sp) to[j].setAttribute('data-sourcepos', sp);
        }
        return true;
      }

      function mustRedraw(fresh, selector) {
        return !!selector && fresh.nodeType === 1 &&
          (fresh.matches(selector) || !!fresh.querySelector(selector));
      }

      /** Bottom of the sticky page and file headers, which cover the top of the doc. */
      function coveredTop() {
        var bottom = 0;
        var bars = document.querySelectorAll('.header, .file-header');
        for (var i = 0; i < bars.length; i++) {
          var r = bars[i].getBoundingClientRect();
          if (r.height > 0 && r.bottom > bottom) bottom = r.bottom;
        }
        return bottom;
      }

      function isShown() {
        return root.getClientRects().length > 0;
      }

      /** Blocks that intersect the viewport, top first; any one of them can hold the view. */
      function visibleBlocks() {
        var covered = coveredTop();
        var out = [];
        var list = blocksOf(root);
        for (var i = 0; i < list.length && out.length < 50; i++) {
          if (list[i].nodeType !== 1) continue;
          var r = list[i].getBoundingClientRect();
          if (r.bottom <= covered) continue;
          if (r.top >= window.innerHeight) break;
          out.push({ el: list[i], key: keyOfLive(list[i]), index: i, top: r.top });
        }
        return out;
      }

      /** The same block after a patch: the kept node, else a fresh one with the same key. */
      function find(candidate, shift) {
        if (candidate.el.isConnected && candidate.el.parentNode === root) return candidate.el;
        var list = blocksOf(root);
        var want = candidate.index + shift;
        var best = null;
        for (var i = 0; i < list.length; i++) {
          if (keyOfLive(list[i]) !== candidate.key) continue;
          if (!best || Math.abs(i - want) < Math.abs(best.i - want)) best = { i: i, el: list[i] };
        }
        return best && best.el;
      }

      function scrollByDelta(delta) {
        if (Math.abs(delta) < 0.5) return;
        window.scrollTo({ top: window.scrollY + delta, behavior: 'instant' });
        ownScrollY = window.scrollY;
      }

      /** Runs `fn`, then scrolls so the first visible block that survived is where it was. */
      function withScrollAnchor(fn) {
        if (window.scrollY === 0 || !isShown()) {
          held = null;
          fn();
          return;
        }
        var before = visibleBlocks();
        var count = blocksOf(root).length;
        fn();
        var shift = blocksOf(root).length - count;
        held = null;
        for (var i = 0; i < before.length; i++) {
          var el = find(before[i], shift);
          if (!el) continue;
          scrollByDelta(el.getBoundingClientRect().top - before[i].top);
          held = { el: el, top: before[i].top };
          ownScrollY = window.scrollY;
          return;
        }
      }

      // Images, fonts and diagrams that finish after the patch grow the doc above the reader.
      function reanchor() {
        if (!held || !held.el.isConnected || !isShown()) return;
        scrollByDelta(held.el.getBoundingClientRect().top - held.top);
      }

      function run(html, o) {
        var tpl = document.createElement('template');
        tpl.innerHTML = html;
        var incoming = blocksOf(tpl.content);
        var newKeys = incoming.map(keyOf);
        var live = blocksOf(root);
        var plan = planPatch(live.map(keyOfLive), newKeys);

        function place(fresh, key, ref) {
          root.insertBefore(fresh, ref);
          keys.set(fresh, key);
        }

        withScrollAnchor(function() {
          var keptEnd = live.length - plan.suffix;
          var ref = keptEnd < live.length ? live[keptEnd] : null;
          for (var i = plan.prefix; i < keptEnd; i++) root.removeChild(live[i]);
          for (var j = plan.prefix; j < incoming.length - plan.suffix; j++) {
            place(incoming[j], newKeys[j], ref);
          }
          // Kept blocks take the fresh sourcepos, or are swapped when they can't
          for (var k = 0; k < incoming.length; k++) {
            var inMiddle = k >= plan.prefix && k < incoming.length - plan.suffix;
            if (inMiddle) continue;
            var old = k < plan.prefix ? live[k] : live[k - (incoming.length - live.length)];
            if (mustRedraw(incoming[k], o.redraw) || !copySourcepos(old, incoming[k])) {
              place(incoming[k], newKeys[k], old);
              root.removeChild(old);
            }
          }
        });

        if (o.source != null && opts.source) opts.source.innerHTML = o.source;
        if (o.stats && opts.stats) opts.stats.textContent = o.stats;
        if (opts.onPatched) opts.onPatched();
      }

      /**
       * Swaps the doc for `html`. Waits while an IME composition is open, and then
       * applies only the latest content. `redraw` names blocks to rebuild even when unchanged.
       */
      function apply(html, o) {
        o = o || {};
        if (composing) {
          // Only the latest content is kept, but a redraw asked for by a skipped one still applies
          var redraw = o.redraw || (pending && pending.o.redraw);
          pending = { html: html, o: { source: o.source, stats: o.stats, redraw: redraw } };
          return;
        }
        run(html, o);
      }

      /** Takes over `doc`, keying its server-rendered blocks before anything mutates them. */
      function attach(doc, options) {
        root = doc;
        opts = options || {};
        blocksOf(root).forEach(keyOfLive);

        document.addEventListener('compositionstart', function() { composing = true; }, true);
        document.addEventListener('compositionend', function() {
          composing = false;
          if (!pending) return;
          var p = pending;
          pending = null;
          run(p.html, p.o);
        }, true);

        // Any scroll we didn't make means the reader moved, so late growth is theirs to keep.
        window.addEventListener('scroll', function() {
          if (ownScrollY === null || Math.abs(window.scrollY - ownScrollY) > 1) held = null;
        }, { passive: true });

        if (typeof ResizeObserver !== 'undefined') {
          new ResizeObserver(reanchor).observe(root);
        }
        root.addEventListener('load', reanchor, true);
      }

      var api = {
        planPatch: planPatch,
        attach: attach,
        apply: apply,
        withScrollAnchor: withScrollAnchor,
        // Set by review at boot, so the patcher never looks up a global a doc element's id can take
        skip: function() { return false; }
      };
      return api;
    })();
