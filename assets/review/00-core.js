var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};
  // Trust is membership, never a tag name: a doc can make any `birta-*` element it likes
  BR.FLOW_HOSTS = BR.FLOW_HOSTS || new WeakSet();
  BR.isHost = function (node) {
    return !!node && typeof node === 'object' && BR.FLOW_HOSTS.has(node);
  };

  // The server closes a socket past 256 KiB, and the outbox would resend it forever
  var MAX_OP_BYTES = 200 * 1024;
  // What a closing page may still send: never a doc write, a hand-back or an approve
  var BEACON_OPS = { comment: 1, amend: 1, retract: 1, reply: 1 };
  // What outlives a dropped socket or the page; an approve, hand-back or undo replayed later acts unasked
  var KEPT_OPS = { comment: 1, amend: 1, retract: 1, reply: 1, seen: 1 };
  var OUTBOX = 'birta-unacked:';
  // Held every op, shared by every tab of a doc; boot drops it unreplayed
  var OLD_OUTBOX = 'birta-outbox:';
  var BUFFER_MS = 500;
  // Ops that make or change a note: until the state shows them, the page's own drafts
  var DRAFT_OPS = { comment: 1, amend: 1, reply: 1 };

  // cid -> {n, dir, adds}: n is the acked line, null until the ack comes
  var pending = {};

  // cids of sent ops that are never kept, until their answer comes
  var fleeting = {};

  var seq = 0;
  var session = Date.now().toString(36) + Math.random().toString(36).slice(2, 10);

  // This tab's own: the tab id survives a reload, and another tab never replays it
  function outboxKey() { return OUTBOX + (birtaBus.tab ? birtaBus.tab() : session) + ':' + CURRENT_PATH; }

  function dropOldOutboxes() {
    try {
      Object.keys(localStorage).forEach(function (k) {
        if (k.indexOf(OLD_OUTBOX) === 0) localStorage.removeItem(k);
      });
    } catch (e) {}
  }

  // Read and written whole on every change, since other tabs share the key
  function readOutbox() {
    try {
      var box = JSON.parse(localStorage.getItem(outboxKey()) || '[]');
      return Array.isArray(box) ? box : [];
    } catch (e) {
      return [];
    }
  }

  function writeOutbox(box) {
    try {
      if (box.length) localStorage.setItem(outboxKey(), JSON.stringify(box));
      else localStorage.removeItem(outboxKey());
    } catch (e) {}
  }

  /** Sends a review op and keeps it in the outbox until the server acks or nacks its cid. */
  BR.op = function (fields) {
    var frame = {};
    for (var k in fields) frame[k] = fields[k];
    var cid = 'r' + session + '-' + (++seq);
    frame.type = 'op';
    frame.cid = cid;
    frame.path = CURRENT_PATH;
    frame.rev = birtaBus.rev();
    var json = JSON.stringify(frame);
    if (new Blob([json]).size > MAX_OP_BYTES) throw new Error('review op too large to send');
    var kept = !!KEPT_OPS[frame.op];
    if (kept) {
      var box = readOutbox();
      box.push(frame);
      writeOutbox(box);
    } else {
      fleeting[cid] = true;
    }
    if (!birtaBus.send(frame) && !kept) lose(cid);
    if (DRAFT_OPS[frame.op]) {
      pending[cid] = { n: null, dir: !!(frame.at && frame.at.k === 'dir'), adds: frame.op !== 'amend' };
      if (BR.chrome && BR.chrome.render) BR.chrome.render();
    }
    return cid;
  };

  /** The page's own notes the server hasn't shown back yet. */
  BR.pending = {
    /** New notes among them, for the turn button's count. */
    adds: function () {
      return Object.keys(pending).filter(function (c) { return pending[c].adds; }).length;
    },
    any: function () { return Object.keys(pending).length > 0; },
    /** Whether any is still waiting for its ack. */
    unacked: function () {
      return Object.keys(pending).some(function (c) { return pending[c].n === null; });
    }
  };

  /** Drops what a frame shows the server has: an ack's line once a state reaches it. */
  function settlePending(msg) {
    Object.keys(pending).forEach(function (cid) {
      var p = pending[cid];
      if (msg.type === 'nack' && msg.cid === cid) delete pending[cid];
      else if (msg.type === 'ack' && msg.cid === cid) {
        if (msg.n == null) delete pending[cid];
        else p.n = msg.n;
      } else if (p.n !== null && !p.dir && msg.type === 'review_state' && msg.path === CURRENT_PATH && msg.state) {
        if (((msg.state.log && msg.state.log.lines) || 0) >= p.n) delete pending[cid];
      } else if (p.n !== null && p.dir && msg.type === 'dir_state') {
        delete pending[cid];
      }
    });
  }

  function replay() {
    readOutbox().forEach(function (frame) { if (KEPT_OPS[frame.op]) birtaBus.send(frame); });
  }

  /** Refuses an op that is never replayed and has no answer coming, once its sender has the cid. */
  function lose(cid) {
    delete fleeting[cid];
    setTimeout(function () { birtaBus.emit({ type: 'nack', cid: cid, reason: 'not_sent' }); }, 0);
  }

  function settle(cid) {
    var box = readOutbox();
    var left = box.filter(function (f) { return f.cid !== cid; });
    if (left.length !== box.length) writeOutbox(left);
  }

  // Cids a beacon already carried, so hiding and then closing sends them once
  var flushed = {};

  /** Hands the server what the socket hasn't acked yet, as the page hides or goes. */
  function flush() {
    var seen = [];
    var ops = [];
    readOutbox().forEach(function (f) {
      if (f.path !== CURRENT_PATH || flushed[f.cid]) return;
      if (f.op === 'seen') seen = seen.concat(f.ids || []);
      else if (BEACON_OPS[f.op]) ops.push(f);
      else return;
      flushed[f.cid] = true;
    });
    if (!seen.length && !ops.length) return;
    var body = { token: BOOT.review.token, tab: birtaBus.tab(), path: CURRENT_PATH, seen: seen, ops: ops };
    navigator.sendBeacon('/review/flush', new Blob([JSON.stringify(body)], { type: 'application/json' }));
  }

  // --- Store and rev gate ---

  var state = null;
  var ahead = null;
  var aheadTimer = null;
  var shown = {};

  function draw() {
    var shown = state && state.rev === birtaBus.rev() ? state : null;
    // Flow first: its hosts move the blocks the lane measures
    BR.flow.reconcile(shown);
    BR.lane.draw(shown);
  }

  function dropAhead() {
    ahead = null;
    clearTimeout(aheadTimer);
    aheadTimer = null;
  }

  function take(next) {
    if (next.rev === birtaBus.rev()) {
      state = next;
      dropAhead();
      draw();
    } else if (!shown[next.rev]) {
      // The deadline runs from the first state ahead; newer ones only replace it
      ahead = next;
      if (!aheadTimer) aheadTimer = setTimeout(refetch, BUFFER_MS);
    }
  }

  function refetch() {
    aheadTimer = null;
    if (!ahead) return;
    birtaBus.refetch().then(function (rev) {
      if (rev) shown[rev] = true;
      if (ahead && ahead.rev === birtaBus.rev()) state = ahead;
      ahead = null;
      draw();
    });
  }

  function onFrame(msg) {
    settlePending(msg);
    BR.flow.frame(msg);
    switch (msg.type) {
      case 'open':
        replay();
        break;
      case 'close':
        Object.keys(fleeting).forEach(lose);
        break;
      case 'content':
        if (msg.path && msg.path !== CURRENT_PATH) break;
        shown[birtaBus.rev()] = true;
        if (ahead && ahead.rev === birtaBus.rev()) {
          state = ahead;
          dropAhead();
        }
        draw();
        break;
      case 'theme_update':
        readTheme();
        draw();
        break;
      case 'review_state':
        if (msg.path !== CURRENT_PATH || !msg.state || msg.rev !== msg.state.rev) break;
        take(msg.state);
        break;
      case 'ack':
      case 'nack':
        delete fleeting[msg.cid];
        settle(msg.cid);
        break;
    }
  }

  /** The snapshot drawn for the page's rev, or null. */
  BR.currentState = function () { return state; };

  // --- Blocks by sp ---

  // sp -> the doc's element for it, dropped whenever the doc's DOM changes
  var index = null;
  var watcher = null;
  var version = 0;

  function blockIndex() {
    var root = document.getElementById('content');
    if (!watcher) {
      watcher = new MutationObserver(function () { index = null; });
      watcher.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-sourcepos', 'open'] });
    }
    // A patch and the redraw after it run in one task, before the observer is told
    if (watcher.takeRecords().length) index = null;
    if (!index) {
      index = new Map();
      // A list shares its first item's sp and comes first, so the last match is the item
      root.querySelectorAll('[data-sourcepos]').forEach(function (el) { index.set(el.getAttribute('data-sourcepos'), el); });
      version++;
    }
    return index;
  }

  BR.blocks = {
    /** The element the doc shows now for `sp`, or null. */
    of: function (sp) { return blockIndex().get(sp) || null; },
    /** Moves on whenever the doc's blocks, their sps or which details are open change. */
    version: function () {
      blockIndex();
      return version;
    },
    /** The doc's elements now with tag `name` and exactly `text`. */
    alike: function (name, text) {
      var out = [];
      blockIndex().forEach(function (el) { if (el.localName === name && el.textContent === text) out.push(el); });
      return out;
    }
  };

  // --- Shadow roots ---

  // The colours shadow CSS reads, set inside every root, where the doc's CSS can't reach them
  var THEME_VARS = ['--review-you', '--review-agent', '--review-new', '--review-wash', '--review-quiet', '--review-new-bar',
    '--review-no', '--review-yes', '--review-card', '--birta-fg', '--birta-fg-muted', '--birta-bg', '--birta-border',
    '--bgColor-default', '--borderColor-muted'];
  var themeSheet = null;

  /** Takes the theme's colours from the page's own style sheets, with the doc's switched off. */
  function readTheme() {
    if (!themeSheet) return;
    var off = [];
    document.querySelectorAll('#content style, #content link[rel~="stylesheet"]').forEach(function (n) {
      if (n.sheet && !n.sheet.disabled) {
        n.sheet.disabled = true;
        off.push(n.sheet);
      }
    });
    var cs = getComputedStyle(document.documentElement);
    var decls = THEME_VARS.map(function (name) {
      var v = cs.getPropertyValue(name).trim();
      // An empty value would shadow the fallback a var() gives
      return v ? name + ': ' + v + ' !important;' : '';
    });
    off.forEach(function (sheet) { sheet.disabled = false; });
    themeSheet.replaceSync(':host { ' + decls.join(' ') + ' }');
  }

  /**
   * A review host: a `tag` element with a closed root that holds the shadow CSS and the theme's
   * colours, and whose keys reach review as the page's do. Returns `{host, root}`.
   */
  BR.shadowHost = function (tag) {
    var host = document.createElement(tag);
    var root = host.attachShadow({ mode: 'closed' });
    var style = document.createElement('style');
    style.textContent = BR.shadowCss || '';
    root.appendChild(style);
    if (themeSheet) root.adoptedStyleSheets = [themeSheet];
    // Bubbling, so a host's own handlers have the key first
    root.addEventListener('keydown', function (e) { BR.keys.fromHost(e); });
    BR.FLOW_HOSTS.add(host);
    return { host: host, root: root };
  };

  BR.boot = function () {
    if (BR.booted || typeof BOOT === 'undefined' || !BOOT.review || !BOOT.review.enabled) return;
    BR.booted = true;
    document.documentElement.setAttribute('data-review', '');
    shown[birtaBus.rev()] = true;
    dropOldOutboxes();
    // Review's in-flow hosts are not the doc's blocks
    if (typeof birtaPatch !== 'undefined') birtaPatch.skip = BR.isHost;
    try {
      themeSheet = new CSSStyleSheet();
    } catch (e) {}
    readTheme();
    new MutationObserver(readTheme).observe(document.documentElement, {
      attributes: true, attributeFilter: ['data-theme', 'data-birta-theme']
    });
    if (window.matchMedia) matchMedia('(prefers-color-scheme: dark)').addEventListener('change', readTheme);
    BR.lane.init();
    birtaBus.on(onFrame);
    BR.keys.init();
    BR.pointing.init();
    BR.chrome.init();
    BR.dir.init();
    BR.life.init();
    document.addEventListener('visibilitychange', function () {
      if (document.visibilityState === 'hidden') flush();
    });
    addEventListener('pagehide', flush);
  };

  // After the whole bundle has run, so the shadow CSS appended to it is in place
  if (typeof document !== 'undefined') {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', function () { BR.boot(); });
    } else {
      setTimeout(function () { BR.boot(); }, 0);
    }
  }
})();
