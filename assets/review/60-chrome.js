var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  var MIN = 60000;
  var NAME_CHARS = 24;
  var NACK_MS = 4000;
  // A status pill that stayed would sit over what the reader looks at next
  var PILL_MS = 4000;
  // A ⌘⏎ this soon after the approve guard's jump is a reflex, not a second press
  var DWELL_MS = 1000;
  // Keys that only modify the next one never count as "another key"
  var MODIFIERS = { Meta: 1, Control: 1, Shift: 1, Alt: 1, AltGraph: 1, CapsLock: 1, Fn: 1 };
  var COPY = 'copy hand-off line';
  // How long a folder's hand-back sits unclaimed before the phrase points at the hand-off line
  var CUE_MS = 10000;
  var NACKS = {
    changed_since: 'changed since · try again',
    nothing_to_send: 'nothing to hand back',
    hold_pending: 'already handing back',
    // A read-only doc can't take an undo at all, so a refusal blames the review file
    read_only: 'review file is read-only',
    not_reversible: "can't undo this one · c instead",
    // The socket went before the op reached it or was answered, and it is never replayed
    not_sent: 'connection lost · try again'
  };

  function cap(name) { return name.charAt(0).toUpperCase() + name.slice(1); }
  function plural(n, one, many) { return n + ' ' + (n === 1 ? one : many); }
  function pad(n) { return String(n).padStart(2, '0'); }

  // Local time: the server's zone may not be the reader's
  function hhmm(ts) {
    var d = new Date(ts);
    return pad(d.getHours()) + ':' + pad(d.getMinutes());
  }

  function holding(s, now) { return !!s.hold && now < s.hold.not_before; }

  /** The state as the reader should see it at `now`: inside a hold, nothing has left yet. */
  function view(s, now) {
    var held = holding(s, now);
    var c = s.counts || {};
    var drafts = (c.drafts || 0) + (held && s.hold.kind === 'hand_back' ? s.hold.count : 0);
    var p = (held && s.phrase_before_hold) || s.phrase || { stage: 'idle' };
    // A note written after approve reopens the round: until it goes back, it is your turn again
    if (p.stage === 'approved' && drafts > 0) p = { stage: 'drafts' };
    return {
      held: held,
      p: p,
      drafts: drafts,
      decided: c.decided || 0,
      items: c.items || 0,
      notHandled: c.not_handled || 0,
      unseen: (c.unseen || 0) + unseenReplies(s),
      asks: c.open_asks || 0
    };
  }

  // The agent's replies the reader hasn't seen; the approve guard stops for them as for changes
  function unseenReplies(s) {
    var n = 0;
    (s.items || []).forEach(function (i) {
      (i.replies || []).forEach(function (r) { if (r.by && r.by.type === 'agent' && !r.seen) n++; });
    });
    return n;
  }

  /** Whether `s` is approved and past its hold at `now`: the page then shows only dots. */
  BR.pure.clean = function (s, now) {
    return !!s && s.log_writable !== false && view(s, now).p.stage === 'approved';
  };

  /**
   * What the approve guard's first press says, or null when nothing stops it: unseen changes
   * and replies, else the agent's open questions. `opts.key` is the turn key as the button shows it.
   */
  BR.pure.guardPill = function (s, opts) {
    var v = view(s, s.server_now);
    var again = ' · ' + opts.key + ' again approves anyway';
    if (v.unseen) return v.unseen + ' unseen · n walk' + again;
    if (!v.asks) return null;
    var ask = (s.items || []).filter(function (i) {
      return i.by && i.by.type === 'agent' && i.verdict === 'ask' && i.status === 'open';
    })[0];
    var who = (ask && ask.by.name) || holder(s, v.p);
    return (who ? cap(who) : 'The agent') + ' asked ' + plural(v.asks, 'question', 'questions') + ' · n go' + again;
  };

  /** Whole minutes of silence once the fold's threshold has passed, else null. */
  function silence(p, now) {
    var s = p.stage === 'picked_up' && p.picked_up && p.picked_up.silence;
    if (!s || now - s.since_ts < s.threshold_ms) return null;
    return Math.floor((now - s.since_ts) / MIN);
  }

  /** Who holds the turn: the agent that picked it up, else the drafter, else nobody known. */
  function holder(s, p) {
    if (p.stage === 'picked_up' && p.picked_up && p.picked_up.by) return p.picked_up.by;
    return (s.owner && s.owner.drafted_by) || null;
  }

  /** An agent seen in the doc's rounds, for marking a doc of yours as its draft. */
  function anyAgent(s) {
    var p = s.phrase || {};
    if (p.picked_up && p.picked_up.by) return p.picked_up.by;
    var claimed = (s.rounds || []).filter(function (r) { return r.claimed_by; });
    return claimed.length ? claimed[claimed.length - 1].claimed_by : null;
  }

  function short(name) {
    var chars = Array.from(name);
    return chars.length > NAME_CHARS ? chars.slice(0, NAME_CHARS).join('') + '…' : name;
  }

  // Counts the phrase shows before the rest go to its tooltip
  var SHOWN_COUNTS = 3;

  /** The header phrase for `s` at server time `opts.now` (design call 5, r-frontend §11). */
  BR.pure.formatPhrase = function (s, opts) {
    return phraseOf(s, opts, !!opts.full);
  };

  /** The phrase; `full` keeps every count and fact the header leaves to the tooltip. */
  function phraseOf(s, opts, full) {
    if (s.log_writable === false) return 'review unavailable · folder not writable';
    var parts = docPhrase(s, opts);
    var counts = full ? parts.counts : parts.counts.slice(0, SHOWN_COUNTS);
    var base = parts.head.concat(counts, full ? parts.rest : [], parts.tail).join(' · ');
    var others = opts.dir ? BR.pure.otherFiles(opts.dir, opts.path).length : 0;
    return others ? base + ' · ' + plural(others, 'other file', 'other files') + ' ▾' : base;
  }

  /**
   * The phrase in parts: `head`, then `counts` in priority order (unseen, notes, decided, then a
   * reply's tallies), then `rest`, which only the tooltip shows, then `tail`. Only the first few
   * counts show; the tooltip has them all.
   */
  function docPhrase(s, opts) {
    var v = view(s, opts.now);
    var p = v.p;
    function parts(head, counts, tail, rest) { return { head: head, counts: counts || [], tail: tail || [], rest: rest || [] }; }
    if (s.writable === false) return parts(['read-only · notes only']);
    // Unseen, then your notes, then decided blocks: a new count never pushes an old one out
    function yours() {
      var out = [];
      if (v.unseen) out.push(v.unseen + ' unseen');
      if (v.drafts) out.push('your turn · ' + plural(v.drafts, 'note', 'notes'));
      if (v.decided > 0) out.push(v.decided + ' of ' + plural(v.items, 'block', 'blocks') + ' decided');
      return out;
    }
    switch (p.stage) {
      case 'sent': {
        var sent = 'sent ' + hhmm(p.sent.ts) + ' · not picked up yet';
        return parts([cue(s, p, opts) ? sent + ' · ' + COPY : sent]);
      }
      case 'picked_up': {
        var out = [cap(p.picked_up.by) + ' picked up ' + hhmm(p.picked_up.ts)];
        if (v.drafts) {
          out.push((v.decided > 0 ? v.drafts + ' decided' : plural(v.drafts, 'note', 'notes')) + ' for next hand-back');
        }
        var quiet = silence(p, opts.now);
        if (quiet != null) out.push('no reply in ' + quiet + ' min', 'release');
        return parts(out);
      }
      case 'replied': {
        var r = p.replied;
        var tally = [[r.done, 'done'], [r.answered, 'answered'], [r.asked, 'for you'], [r.unasked, 'without a note'],
          [r.not_handled, 'not handled']];
        return parts([cap(r.by) + ' replied ' + hhmm(r.ts)], yours().concat(
          tally.filter(function (t) { return t[0] > 0; }).map(function (t) { return t[0] + ' ' + t[1]; })
        ));
      }
      case 'approved': {
        var a = s.approve;
        var head = ['approved ' + hhmm(a.ts)].concat(a.changed_since ? ['changed since'] : []);
        return parts(head, [], ['c to reopen'], a.later ? [a.later + ' later'] : []);
      }
      case 'drafts':
        return parts([], yours());
      default: {
        var drafter = s.owner && s.owner.drafted_by ? ['drafted by ' + cap(s.owner.drafted_by)] : [];
        return parts(drafter, yours(), ['your turn']);
      }
    }
  }

  /** The phrase's tooltip: the phrase with every fact it leaves out. */
  BR.pure.phraseTitle = function (s, opts) {
    var all = {};
    for (var k in opts) all[k] = opts[k];
    all.full = true;
    return BR.pure.formatPhrase(s, all);
  };

  /** A folder with no drafter has no agent waiting on it: a hand-back left unclaimed needs the line. */
  function cue(s, p, opts) {
    return !!opts.folder && !(s.owner && s.owner.drafted_by) && !!s.hand_off && opts.now - p.sent.ts >= CUE_MS;
  }

  /** This page's drafts as the listing last counted them; none while it doesn't list the page. */
  function listedHere(dir, path) {
    var f = (dir.files || []).filter(function (x) { return (x.paths || []).indexOf(path) >= 0; })[0];
    return (f && f.summary && f.summary.counts.drafts) || 0;
  }

  function button(text, kbd, disabled) { return { text: text, kbd: !!kbd, disabled: !!disabled }; }

  /** The turn button's label, whether it shows the key hint, and whether it acts; null for no button. */
  BR.pure.turnButton = function (s, opts) {
    if (s.log_writable === false) return null;
    var v = view(s, opts.now);
    var p = v.p;
    function approve() {
      // Unseen wins: the guard jumps there first
      if (v.unseen) return button('Approve · ' + v.unseen + ' unseen');
      if (v.asks) return button('Approve · ' + v.asks + ' unanswered');
      return button('Approve', true);
    }
    if (v.held && s.hold.kind === 'approve') return approve();
    var t = opts.dir && opts.dir.totals;
    // Notes the page kept that no state shows yet still count: ⌘⏎ must never approve past them
    var mine = v.drafts + (opts.pending || 0);
    // This doc's own state can be ahead of the throttled listing, which may not count its newest note
    var total = t ? Math.max(t.drafts + (opts.pending || 0), t.drafts - listedHere(opts.dir, opts.path) + mine) : mine;
    if (opts.busy && total === 0) total = 1;
    if (total > 0) {
      if (t && t.unseen_files) return button('Hand back · ' + total + ' · ' + plural(t.unseen_files, 'file', 'files') + ' unseen');
      return button('Hand back · ' + total, true);
    }
    if (p.stage === 'sent' || p.stage === 'picked_up') {
      var who = holder(s, p);
      return button('With ' + (who ? short(cap(who)) : 'the agent'), false, true);
    }
    // Text that moved on since the approve can be approved again
    if (p.stage === 'approved' && !s.approve.changed_since) return button('Approved', false, true);
    // A folder doc of yours that no agent has taken up has nothing to approve
    if (opts.folder && !(s.owner && s.owner.drafted_by) && !anyAgent(s)) return null;
    return approve();
  };

  /** Your answers to the agent that hand-back `n` carries: replies no earlier hand-back took. */
  function answersIn(s, n) {
    var before = (s.rounds || []).map(function (r) { return r.submit; }).filter(function (x) { return x < n; });
    var count = 0;
    (s.items || []).forEach(function (i) {
      if (!i.by || i.by.type !== 'agent') return;
      (i.replies || []).forEach(function (r) {
        var taken = before.some(function (x) { return x > r.n; });
        if (r.by && r.by.type === 'human' && r.n < n && !taken) count++;
      });
    });
    return count;
  }

  /** What the pill says during a hold, or null outside one. */
  BR.pure.holdPill = function (s, opts) {
    if (!holding(s, opts.now)) return null;
    if (s.hold.kind === 'approve') return 'approving · z to undo';
    var answers = answersIn(s, s.hold.n);
    var parts = s.hold.count || !answers ? [plural(s.hold.count, 'note', 'notes')] : [];
    if (answers) parts.push(plural(answers, 'answer', 'answers'));
    return 'handing back · ' + parts.join(' · ') + ' · z to undo';
  };

  /** The phrase menu's rows: `{id, text, op?, copy?}`, in order. */
  BR.pure.menuRows = function (s, opts) {
    if (s.log_writable === false) return [];
    var v = view(s, opts.now);
    var p = v.p;
    var rows = [];
    // Inside the hold z takes it back, so the menu only offers it after
    if (p.stage === 'sent' && p.sent && p.sent.can_withdraw)
      rows.push({ id: 'withdraw', text: 'withdraw', op: { op: 'withdraw', to: p.sent.n } });
    if (p.stage === 'approved' && s.approve && s.approve.can_withdraw)
      rows.push({ id: 'withdraw', text: 'withdraw', op: { op: 'withdraw', to: s.approve.n } });
    if (silence(p, opts.now) != null)
      rows.push({ id: 'release', text: 'release', op: { op: 'release', claim: p.picked_up.n } });
    if (s.hand_off) rows.push({ id: 'copy', text: COPY, copy: s.hand_off });
    var drafter = s.owner && s.owner.drafted_by;
    var agent = drafter ? null : anyAgent(s);
    if (drafter) rows.push({ id: 'owner', text: 'drafted by ' + cap(drafter) + ' · mark as yours', op: { op: 'owner', drafted_by: null } });
    else if (agent) rows.push({ id: 'owner', text: 'mark as drafted by ' + cap(agent), op: { op: 'owner', drafted_by: agent } });
    // A folder note can only be handed back where a folder is served
    if (opts.folder) {
      var f = opts.dir && opts.dir.folder;
      rows.push({ id: 'folder', text: 'note on this folder', note: f && f.summary ? BR.pure.fileNote({ summary: f.summary }, opts) : '' });
    }
    if (opts.dir) {
      BR.pure.otherFiles(opts.dir, opts.path).forEach(function (x) {
        rows.push({ id: 'file', text: x.paths[0], note: BR.pure.fileNote(x, opts), href: BR.pure.fileHref(x.paths[0]) });
      });
    }
    rows.push({ id: 'keys', text: 'keys', note: '?' });
    return rows;
  };

  // Literal keys in the sheet, which no binding names
  var TAB = { key: 'Tab' };
  // The sheet's groups: each row is `[actions, note?]`
  var SHEET = [
    ['decide', [[['yes', 'no', 'later', 'your_call']]]],
    ['write', [[['note', 'ask']]]],
    ['move', [[['focus_next', 'focus_prev', 'next_item', 'prev_item', 'next_file', 'prev_file', TAB]]]],
    ['take back', [[['undo']]]],
    ["Claude's changes", [[['revert_change'], "revert Claude's change (open row)"]]],
    ['hand back / approve', [[['turn']]]],
    ['view', [[['toggle_reading', 'toggle_dark', 'focus_theme', 'hide_marks']], [['toggle_raw'], 'source view']]]
  ];
  var KEY_NAMES = { Enter: '⏎', Escape: 'Esc', ' ': 'Space', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' };

  /** A binding as the sheet prints it: `Shift+J` is `⇧J`, `Mod+Enter` is `⌘⏎` on a Mac and `Ctrl⏎` elsewhere. */
  function keyName(binding, mac) {
    var parts = binding.split('+');
    var key = parts.pop();
    var shift = parts.indexOf('Shift') >= 0;
    var name = KEY_NAMES[key] || (key.length === 1 ? (shift ? key.toUpperCase() : key.toLowerCase()) : key);
    var mods = { Mod: mac ? '⌘' : 'Ctrl', Meta: '⌘', Ctrl: 'Ctrl', Alt: mac ? '⌥' : 'Alt', Shift: '⇧' };
    return parts.map(function (m) { return mods[m] || m; }).join('') + name;
  }

  /** The `?` sheet for `bindings`: `[{label, rows: [{keys, note?}]}]`, leaving out unbound keys and what they empty. */
  BR.pure.keySheet = function (bindings, mac) {
    var b = bindings || {};
    return SHEET.map(function (g) {
      var rows = g[1].map(function (r) {
        var keys = r[0].map(function (a) { return a === TAB ? TAB.key : b[a] ? keyName(b[a], mac) : ''; })
          .filter(function (k) { return k; });
        return r[1] ? { keys: keys, note: r[1] } : { keys: keys };
      }).filter(function (r) { return r.keys.length; });
      return { label: g[0], rows: rows };
    }).filter(function (g) { return g.rows.length; });
  };

  /** A refused op in words. `claimed` names whoever holds the turn in `s`. */
  BR.pure.nackText = function (reason, s) {
    if (reason === 'claimed') {
      var who = s ? holder(s, s.phrase || {}) : null;
      return (who ? cap(who) : 'the agent') + ' already picked it up';
    }
    return NACKS[reason] || String(reason).replace(/_/g, ' ');
  };

  // --- The header, the popovers and the tab ---

  var phrase, phraseText, phraseWords, turn, turnBtn, turnWords, turnKbd, pop, menu, pill, sheet;
  // Server time minus this page's clock; it only moves forward, so a passed hold never comes back
  var skew = null;
  var dir = null;
  var down = false;
  // A hand-back this page sent, until its ack: {cid, count}
  var handing = null;
  // Its batch once acked; another doc's hold the page's own state never shows
  var batch = null;
  var withdrawing = null;
  // The pill's own message: {text, until?, arm?} from pointing, or {nack, until} for a refused op
  var message = null;
  // The approve guard's jump, until a key other than ⌘⏎ or its pill going disarms it: {at, ids, unseen}
  var guard = null;
  var heldN = null;
  var rows = [];
  var cur = 0;
  var timer = null;
  var baseTitle = '';
  var plainIcon = null;
  var dottedIcon = null;

  function now() { return Date.now() + (skew || 0); }

  function folderMode() { return typeof BOOT !== 'undefined' && !!(BOOT.review && BOOT.review.dir); }

  function opts(t) { return { now: t, dir: dir, path: CURRENT_PATH, folder: folderMode() }; }

  function mac() { return /Mac/.test(navigator.platform); }

  /** The button's options: only in a folder do its count and hand-back take in every file. */
  function turnOpts(t) {
    var p = BR.pending;
    return { now: t, dir: folderMode() ? dir : null, path: CURRENT_PATH, folder: folderMode(), pending: p ? p.adds() : 0, busy: !!p && p.any() };
  }

  /** The active hand-back of all, from its ack: `{batch, n, count, not_before}`, or null. */
  function batchHold(t) { return batch && t < batch.not_before ? batch : null; }

  function host(tag) {
    var made = BR.shadowHost(tag);
    return { el: made.host, root: made.root };
  }

  function active() {
    try { return sessionStorage.getItem('birta-review-active') === '1'; } catch (e) { return false; }
  }

  /** Design call 4: a review file, a drafter, or review started in this tab; in a folder, a review file anywhere. */
  function shown(s) {
    return !!s && ((s.log && s.log.lines > 0) || !!(s.owner && s.owner.drafted_by) || active() || reviewedFolder());
  }

  function reviewedFolder() {
    if (!folderMode() || !dir) return false;
    return !!(dir.folder && dir.folder.summary) || (dir.files || []).some(function (f) { return !!f.summary; });
  }

  function show(el, on) {
    if (on) el.setAttribute('data-shown', '');
    else el.removeAttribute('data-shown');
  }

  function popover(el, on) {
    try {
      var open = el.matches(':popover-open');
      if (on && !open) el.showPopover();
      if (!on && open) el.hidePopover();
    } catch (e) {}
  }

  function render() {
    var s = BR.currentState();
    var t = now();
    var on = shown(s);
    show(phrase, on);
    var b = on ? BR.pure.turnButton(s, turnOpts(t)) : null;
    show(turn, !!b);
    // Text changes in place: WebKit drops a click whose pressed node was replaced before release
    if (on) {
      var text = BR.pure.formatPhrase(s, opts(t));
      var title = BR.pure.phraseTitle(s, opts(t));
      if (phraseWords.data !== text) phraseWords.data = text;
      if (phrase.getAttribute('title') !== title) phrase.setAttribute('title', title);
    }
    if (b) {
      var label = b.kbd ? b.text + ' ' : b.text;
      if (turnWords.data !== label) turnWords.data = label;
      if (b.kbd && !turnKbd.parentNode) turnBtn.appendChild(turnKbd);
      if (!b.kbd && turnKbd.parentNode) turnKbd.remove();
      var who = /^With /.test(b.text) && holder(s, (s.hold && t < s.hold.not_before && s.phrase_before_hold) || s.phrase || {});
      var title = who ? 'With ' + cap(who) : b.text;
      if (turnBtn.getAttribute('title') !== title) turnBtn.setAttribute('title', title);
      turnBtn.disabled = b.disabled;
    }
    renderPill(s, t);
    news(!!s && !!s.counts && s.counts.attention > 0);
    if (menuOpen()) fillMenu();
    schedule(s, t);
  }

  function renderPill(s, t) {
    var hold = s && BR.pure.holdPill(s, { now: t });
    var b = batchHold(t);
    // A folder's hand-back counts every file's notes, whichever doc it was sent from
    if (b && !(hold && s.hold.kind === 'approve')) hold = 'handing back · ' + plural(b.count, 'note', 'notes') + ' · z to undo';
    // A new hold speaks over whatever the pill said before it
    var n = b ? 'batch ' + b.batch : hold ? s.hold.n : null;
    if (n !== null && n !== heldN) say(null);
    heldN = n;
    // A deadline runs on the page's own clock
    if (message && message.until && Date.now() >= message.until) say(null);
    var text = hold ? (down ? 'connection lost · hand-back stands' : hold)
      : message ? (message.nack ? BR.pure.nackText(message.nack, s) : message.text) : '';
    if (pill.textContent !== text) pill.textContent = text;
    popover(pill, !!text);
    place();
  }

  /** Replaces the pill's message; an armed one going takes its arm with it. */
  function say(next) {
    var was = message;
    message = next;
    if (was && was.arm && was !== next && was.arm.off) was.arm.off();
  }

  /** Any key but the one an armed pill waits for disarms it; a bare modifier is not a key yet. */
  function onAnyKey(e) {
    // A held key's repeats are not new keys
    if (!message || !message.arm || MODIFIERS[e.key] || e.repeat) return;
    if (message.arm.keep && message.arm.keep(e)) return;
    say(null);
    render();
  }

  // A pill never covers an open composer: it moves above it, or below when there is no room
  function place() {
    pill.style.removeProperty('top');
    pill.style.removeProperty('bottom');
    var c = BR.flow.composerRect && BR.flow.composerRect();
    if (!c || !pill.textContent) return;
    var p = pill.getBoundingClientRect();
    if (!p.height || p.bottom <= c.top || p.top >= c.bottom || p.right <= c.left || p.left >= c.right) return;
    var top = c.top - p.height - 8;
    pill.style.setProperty('bottom', 'auto');
    pill.style.setProperty('top', (top >= 0 ? top : c.bottom + 8) + 'px');
  }

  var placing = false;

  function placeSoon() {
    if (placing) return;
    placing = true;
    requestAnimationFrame(function () {
      placing = false;
      place();
    });
  }

  /** Re-renders when the next hold ends, the silence threshold passes or its minute ticks, or a nack pill expires. */
  function schedule(s, t) {
    clearTimeout(timer);
    var waits = [];
    if (s && s.hold && s.hold.not_before > t) waits.push(s.hold.not_before - t);
    if (batchHold(t)) waits.push(batch.not_before - t);
    var p = (s && s.phrase) || {};
    var quiet = p.stage === 'picked_up' && p.picked_up && p.picked_up.silence;
    if (quiet) {
      var due = quiet.since_ts + quiet.threshold_ms;
      waits.push(due > t ? due - t : MIN - ((t - quiet.since_ts) % MIN));
    }
    if (message && message.until) waits.push(message.until - Date.now());
    if (folderMode() && p.stage === 'sent' && p.sent && p.sent.ts + CUE_MS > t) waits.push(p.sent.ts + CUE_MS - t);
    if (!waits.length) return;
    timer = setTimeout(function () {
      render();
      // A hold that ends turns an approve into the clean view
      BR.flow.refresh();
      BR.lane.redraw();
    }, Math.max(0, Math.min.apply(null, waits)) + 5);
  }

  function news(on) {
    document.title = on ? '● ' + baseTitle : baseTitle;
    var link = document.querySelector('link[rel~="icon"]');
    if (!link || plainIcon === null) return;
    var want = on ? dottedIcon : plainIcon;
    if (want && link.getAttribute('href') !== want) link.setAttribute('href', want);
    if (on && !dottedIcon) drawDot();
  }

  var drawing = false;

  function drawDot() {
    if (drawing) return;
    drawing = true;
    var img = new Image();
    img.onload = function () {
      var c = document.createElement('canvas');
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      var g = c.getContext('2d');
      g.drawImage(img, 0, 0);
      g.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--review-you').trim() || '#0969da';
      g.beginPath();
      g.arc(c.width * 0.78, c.height * 0.22, c.width * 0.2, 0, Math.PI * 2);
      g.fill();
      dottedIcon = c.toDataURL('image/png');
      // The news may have gone while the icon loaded; render applies whichever is due now
      render();
    };
    img.src = plainIcon;
  }

  // --- The phrase menu ---

  function menuOpen() {
    try { return menu.matches(':popover-open'); } catch (e) { return false; }
  }

  function rowKey(r) { return r ? r.id + ' ' + r.text : null; }

  function fillMenu() {
    var s = BR.currentState();
    var was = rowKey(rows[cur]);
    rows = s ? BR.pure.menuRows(s, opts(now())) : [];
    // The current row stays itself when a push moves it, so ⏎ opens what was shown
    var at = rows.map(rowKey).indexOf(was);
    cur = at >= 0 ? at : Math.max(0, Math.min(cur, rows.length - 1));
    // Rows are kept and rewritten in place, so a push mid-click doesn't lose the click
    while (menu.children.length > rows.length) menu.lastElementChild.remove();
    rows.forEach(function (r, i) {
      var row = menu.children[i];
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('role', 'menuitem');
        var label = document.createElement('span');
        label.className = 'label';
        label.appendChild(document.createTextNode(''));
        var note = document.createElement('span');
        note.className = 'note';
        note.appendChild(document.createTextNode(''));
        row.append(label, note);
        row.addEventListener('click', function () { pick(i); });
        menu.appendChild(row);
      }
      if (row.firstChild.firstChild.data !== r.text) row.firstChild.firstChild.data = r.text;
      if (row.lastChild.firstChild.data !== (r.note || '')) row.lastChild.firstChild.data = r.note || '';
      row.className = 'row' + (i === cur ? ' current' : '') + (r.id === 'file' ? ' file' : '');
    });
  }

  /** Opens the menu, on the row with id `at` when there is one. */
  function openMenu(at) {
    rows = [];
    cur = 0;
    fillMenu();
    var i = rows.map(function (r) { return r.id; }).indexOf(at);
    if (i > 0) {
      cur = i;
      fillMenu();
    }
    var r = phrase.getBoundingClientRect();
    menu.style.top = r.bottom + 4 + 'px';
    menu.style.left = r.left + 'px';
    popover(menu, true);
    menu.focus();
  }

  /** Closes the menu; false when it wasn't open. */
  function closeMenu() {
    if (!menuOpen()) return false;
    popover(menu, false);
    // Focus left in a hidden popover would hold the keys somewhere the reader can't see
    if (document.activeElement === pop.el) menu.blur();
    return true;
  }

  function pick(i) {
    var r = rows[i];
    closeMenu();
    if (!r) return;
    if (r.op) BR.op(r.op);
    if (r.copy && navigator.clipboard) navigator.clipboard.writeText(r.copy);
    if (r.href) BR.dir.open(r.href);
    if (r.id === 'folder') BR.flow.open('note', { k: 'dir' }, null);
    if (r.id === 'keys') openKeys();
  }

  // --- The key sheet ---

  function keysOpen() {
    try { return sheet.matches(':popover-open'); } catch (e) { return false; }
  }

  function openKeys() {
    closeMenu();
    sheet.textContent = '';
    var bindings = typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS;
    BR.pure.keySheet(bindings, mac()).forEach(function (g) {
      g.rows.forEach(function (r, i) {
        var row = document.createElement('div');
        row.className = 'row';
        var label = document.createElement('span');
        label.className = 'label';
        label.textContent = i === 0 ? g.label : '';
        var keys = document.createElement('span');
        keys.className = 'keys';
        r.keys.forEach(function (k) {
          var kbd = document.createElement('kbd');
          kbd.textContent = k;
          keys.appendChild(kbd);
        });
        var note = document.createElement('span');
        note.className = 'note';
        note.textContent = r.note || '';
        row.append(label, keys, note);
        sheet.appendChild(row);
      });
    });
    popover(sheet, true);
  }

  /** Closes the key sheet; false when it wasn't open. */
  function closeKeys() {
    if (!keysOpen()) return false;
    popover(sheet, false);
    return true;
  }

  /** j, k and ⏎ move and pick rows while the menu is open; true when the key was the menu's. */
  function menuKey(e) {
    if (!menuOpen() || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return false;
    if (e.key === 'j' || e.key === 'ArrowDown') cur = Math.min(cur + 1, rows.length - 1);
    else if (e.key === 'k' || e.key === 'ArrowUp') cur = Math.max(cur - 1, 0);
    else if (e.key === 'Enter') {
      pick(cur);
      return true;
    } else return false;
    fillMenu();
    return true;
  }

  function onMenuKey(e) {
    var mine = e.key === 'Escape' ? closeMenu() : menuKey(e);
    if (mine) {
      e.preventDefault();
      e.stopPropagation();
    } else {
      // Any other key closes the menu, then goes on to act as it would on the page
      closeMenu();
    }
  }

  // --- Copy ---

  var HOSTS = 'birta-flow, birta-phrase, birta-turn, birta-pop, birta-under, birta-over';

  /**
   * Native copy serialises review hosts' closed roots too, so a selection that spans one is
   * copied from a clone with the hosts taken out, laid out offscreen for its line breaks.
   */
  function onCopy(e) {
    var sel = getSelection();
    if (!e.clipboardData || BR.isHost(e.target) || !sel || !sel.rangeCount || sel.isCollapsed) return;
    var range = sel.getRangeAt(0);
    var spans = Array.prototype.some.call(document.querySelectorAll(HOSTS), function (h) {
      return BR.isHost(h) && range.intersectsNode(h);
    });
    if (!spans) return;
    var box = document.createElement('div');
    box.className = 'markdown-body';
    box.style.cssText = 'position:fixed;left:-99999px;top:0;width:800px;white-space:normal';
    box.appendChild(range.cloneContents());
    box.querySelectorAll(HOSTS).forEach(function (h) { h.remove(); });
    document.body.appendChild(box);
    var text = box.innerText;
    var html = box.innerHTML;
    box.remove();
    // A selection inside a note reaches the page as its host alone: the browser copies that
    if (!text.trim()) return;
    e.clipboardData.setData('text/plain', text);
    e.clipboardData.setData('text/html', html);
    e.preventDefault();
  }

  // --- The turn ---

  function act() {
    var s = BR.currentState();
    // A plain reader's ⌘⏎ never writes: review has to be on in the tab first
    if (!shown(s)) return;
    var b = BR.pure.turnButton(s, turnOpts(now()));
    if (!b || b.disabled) return;
    if (/^Hand back/.test(b.text)) handBackWhenAcked();
    // Inside an approve's hold the approve is already on its way
    else if (/^Approve/.test(b.text) && !(holding(s, now()) && s.hold.kind === 'approve')) approveGuarded(s);
  }

  /**
   * Approves, unless something unseen or an open question is left: then the first press goes
   * there and says so, and a press past the dwell, while that pill stands, approves anyway.
   */
  function approveGuarded(s) {
    if (guard) {
      if (Date.now() - guard.at < DWELL_MS) return;
      say(null);
      BR.op({ op: 'approve' });
      return;
    }
    var text = BR.pure.guardPill(s, { key: turnKbd.textContent });
    if (!text) {
      BR.op({ op: 'approve' });
      return;
    }
    // Counts that say unseen with no stop to land on still arm: approve is never silent
    BR.pointing.guard();
    var mine = guard = { at: Date.now(), ids: unseenIds(s), unseen: view(s, now()).unseen };
    say({ text: text, arm: { keep: turnKey, off: function () { if (guard === mine) guard = null; } } });
    render();
  }

  /** The unseen changes and replies in `s`, by the id their seen op names. */
  function unseenIds(s) {
    var out = {};
    BR.pure.attention(s).forEach(function (x) { if (x.kind === 'change' || x.kind === 'reply') out[x.id] = 1; });
    return out;
  }

  /**
   * Whether `s` brings unseen text the arm never covered: an id outside its set, or a count
   * above the lowest one since, which the jump's own seen echo lowers.
   */
  function newUnseen(s) {
    var ids = unseenIds(s);
    for (var id in ids) if (!guard.ids[id]) return true;
    var n = view(s, now()).unseen;
    if (n > guard.unseen) return true;
    guard.unseen = n;
    return false;
  }

  function turnKey(e) {
    return BR.pure.keyAction(e, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS) === 'turn';
  }

  function onFrame(msg) {
    switch (msg.type) {
      case 'open':
        down = false;
        break;
      case 'close':
        down = true;
        break;
      case 'review_state':
        if (msg.path === CURRENT_PATH && msg.state && msg.state.server_now) {
          var d = msg.state.server_now - Date.now();
          skew = skew === null ? d : Math.max(skew, d);
        }
        // New unseen text must be seen before approve anyway counts for it
        if (guard && msg.path === CURRENT_PATH && msg.state && newUnseen(msg.state)) say(null);
        break;
      case 'dir_state':
        dir = msg;
        break;
      case 'ack':
        if (message && message.nack) say(null);
        if (handBackDue && !BR.pending.unacked()) {
          handBackDue = false;
          handBack();
        }
        if (handing && msg.cid === handing.cid && msg.batch != null) {
          batch = { batch: msg.batch, n: msg.n, count: handing.count, not_before: msg.not_before };
        }
        if (handing && msg.cid === handing.cid) handing = null;
        if (msg.cid === withdrawing) batch = withdrawing = null;
        break;
      case 'nack':
        // A refused note leaves the others: hand back what was kept, and never approve instead
        if (handBackDue && !BR.pending.unacked()) {
          handBackDue = false;
          var b = BR.pure.turnButton(BR.currentState() || {}, turnOpts(now()));
          if (b && /^Hand back/.test(b.text)) handBack();
        }
        if (handing && msg.cid === handing.cid) handing = null;
        if (msg.cid === withdrawing) withdrawing = null;
        break;
    }
    render();
  }

  // A ⌘⏎ that came while the page's own notes were still on their way
  var handBackDue = false;

  /** Hands back once every note this page sent is acked, so the hand-back covers them. */
  function handBackWhenAcked() {
    if (BR.pending && BR.pending.unacked()) handBackDue = true;
    else handBack();
  }

  /** Hands back this doc's drafts, or in a folder every file's, as the button counts them. */
  function handBack() {
    var b = BR.pure.turnButton(BR.currentState() || {}, turnOpts(now()));
    var count = b && /^Hand back · (\d+)/.exec(b.text);
    var cid = BR.op({ op: 'hand_back', scope: folderMode() ? 'all' : 'doc' });
    handing = { cid: cid, count: count ? Number(count[1]) : 0 };
  }

  /** z inside a folder's hand-back: withdraws its whole batch. False when there is none. */
  function withdrawBatch() {
    var b = batchHold(now());
    if (!b) return false;
    withdrawing = BR.op({ op: 'withdraw', to: b.n, batch: b.batch });
    return true;
  }

  BR.chrome = {
    handBack: handBack,
    withdrawBatch: withdrawBatch,
    init: function () {
      baseTitle = document.title.replace(/^● /, '');
      var link = document.querySelector('link[rel~="icon"]');
      plainIcon = link ? link.getAttribute('href') : null;

      var p = host('birta-phrase');
      phrase = p.el;
      phraseText = document.createElement('span');
      phraseText.className = 'phrase';
      phraseText.setAttribute('role', 'button');
      phraseText.tabIndex = 0;
      phraseWords = document.createTextNode('');
      phraseText.appendChild(phraseWords);
      p.root.appendChild(phraseText);
      phrase.addEventListener('click', function () {
        if (!closeMenu()) openMenu(null);
      });
      phraseText.addEventListener('keydown', function (e) {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        if (!closeMenu()) openMenu(null);
      });
      document.querySelector('.file-header-left').appendChild(phrase);

      var t = host('birta-turn');
      turn = t.el;
      turnBtn = document.createElement('button');
      turnBtn.className = 'turn';
      turnWords = document.createTextNode('');
      turnKbd = document.createElement('kbd');
      turnKbd.textContent = /Mac/.test(navigator.platform) ? '⌘⏎' : 'Ctrl⏎';
      turnBtn.appendChild(turnWords);
      turnBtn.addEventListener('click', act);
      t.root.appendChild(turnBtn);
      var right = document.querySelector('.file-header-right');
      right.insertBefore(turn, right.firstChild);

      pop = host('birta-pop');
      pill = document.createElement('div');
      pill.className = 'pill';
      pill.setAttribute('role', 'status');
      pill.setAttribute('popover', 'manual');
      menu = document.createElement('div');
      menu.className = 'menu';
      menu.setAttribute('role', 'menu');
      menu.setAttribute('popover', 'manual');
      menu.tabIndex = -1;
      menu.addEventListener('keydown', onMenuKey);
      sheet = document.createElement('div');
      sheet.className = 'sheet';
      sheet.setAttribute('role', 'dialog');
      sheet.setAttribute('aria-label', 'keys');
      sheet.setAttribute('popover', 'manual');
      pop.root.append(pill, menu, sheet);
      document.body.appendChild(pop.el);
      // Pointer only: a scripted click elsewhere (the theme menu's) leaves it open
      document.addEventListener('pointerdown', function (e) {
        var path = e.composedPath();
        if (path.indexOf(pop.el) < 0 && path.indexOf(phrase) < 0) closeMenu();
        if (path.indexOf(pop.el) < 0) closeKeys();
      }, true);

      birtaBus.on(onFrame);
      document.addEventListener('copy', onCopy, true);
      // Capture, so a key a host or the composer keeps to itself still disarms
      document.addEventListener('keydown', onAnyKey, true);
      window.addEventListener('scroll', placeSoon, { passive: true });
      window.addEventListener('resize', placeSoon);
      document.addEventListener('input', placeSoon, true);
      render();
    },
    render: render,
    /** The turn button as it reads now, `{text, kbd, disabled}`, or null while it isn't shown. */
    button: function () {
      var s = BR.currentState();
      return shown(s) ? BR.pure.turnButton(s, turnOpts(now())) : null;
    },
    /** Server time, as this page knows it. */
    now: now,
    turn: act,
    openMenu: openMenu,
    menuKey: menuKey,
    closeMenu: closeMenu,
    /** Esc's layer: true when it closed the key sheet or the menu. */
    escape: function () { return closeKeys() || closeMenu(); },
    /** `?`: opens the key sheet, or closes it. */
    toggleKeys: function () {
      if (!closeKeys()) openKeys();
    },
    keysOpen: keysOpen,
    closeKeys: closeKeys,
    /** Whether the review chrome shows on this page (design call 4). */
    shown: function () { return shown(BR.currentState()); },
    /**
     * Shows `text` in the pill, or clears it for ''; it goes after 4 s. An armed pill passes `arm`:
     * `{until?, keep(e), off()}`; it goes when `until` passes or on any key `keep` doesn't claim, and `off` runs then.
     */
    pill: function (text, arm) {
      say(text ? { text: text, until: arm ? arm.until : Date.now() + PILL_MS, arm: arm || null } : null);
      render();
    },
    pillSays: function (text) { return !!message && message.text === text; },
    /** Shows a refused op's reason until the next acked op, a new hold, or 4 s. */
    nack: function (reason) {
      say({ nack: reason, until: Date.now() + NACK_MS });
      render();
    },
    /** Whether the page shows the clean view: approved, and past the approve's hold. */
    clean: function () { return BR.pure.clean(BR.currentState(), now()); },
    /** Moves the pill clear of a composer that just opened or moved. */
    place: place
  };
})();
