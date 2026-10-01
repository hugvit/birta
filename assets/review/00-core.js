var BR = typeof BR === 'undefined' ? {} : BR;
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
  var BUFFER_MS = 500;
  // Ops that make or change a note: until the state shows them, the page's own drafts
  var DRAFT_OPS = { comment: 1, amend: 1, reply: 1 };

  // cid -> {n, dir, adds}: n is the acked line, null until the ack comes
  var pending = {};

  var seq = 0;
  var session = Date.now().toString(36) + Math.random().toString(36).slice(2, 10);

  function outboxKey() { return 'birta-outbox:' + CURRENT_PATH; }

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
    var box = readOutbox();
    box.push(frame);
    writeOutbox(box);
    birtaBus.send(frame);
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
    readOutbox().forEach(function (frame) { birtaBus.send(frame); });
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
        draw();
        break;
      case 'review_state':
        if (msg.path !== CURRENT_PATH || !msg.state || msg.rev !== msg.state.rev) break;
        take(msg.state);
        break;
      case 'ack':
      case 'nack':
        settle(msg.cid);
        break;
    }
  }

  /** The snapshot drawn for the page's rev, or null. */
  BR.currentState = function () { return state; };

  BR.boot = function () {
    if (BR.booted || typeof BOOT === 'undefined' || !BOOT.review || !BOOT.review.enabled) return;
    BR.booted = true;
    document.documentElement.setAttribute('data-review', '');
    shown[birtaBus.rev()] = true;
    BR.lane.init();
    birtaBus.on(onFrame);
    BR.keys.init();
    BR.pointing.init();
    BR.chrome.init();
    BR.dir.init();
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
