var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  var MIN = 60000;
  var NAME_CHARS = 24;
  var NACK_MS = 4000;
  // A plain message gives the keys back after this long; a moment passes its own
  var SAY_MS = 4000;
  // A moment's message: an approve, a reply, a reconnect
  var MOMENT_MS = 6000;
  var SENT = 'sent · you can keep reading';
  var APPROVED = 'approved ✓ · this text is final · nothing is published without you';
  var COPIED = 'copied · paste it to Claude';
  // A ⌘⏎ this soon after the approve guard's jump is a reflex, not a second press
  var DWELL_MS = 1000;
  // Keys that only modify the next one never count as "another key"
  var MODIFIERS = { Meta: 1, Control: 1, Shift: 1, Alt: 1, AltGraph: 1, CapsLock: 1, Fn: 1 };
  var COPY = 'copy hand-off line';
  var COPY_BUTTON = 'Copy hand-off line';
  // How long a hand-back sits unclaimed before the button offers the hand-off line
  var CUE_MS = 10000;
  var NACKS = {
    changed_since: 'changed since · try again',
    nothing_to_send: 'nothing to hand back',
    hold_pending: 'already handing back',
    // A read-only doc can't take an undo at all, so a refusal blames the review file
    read_only: 'review file is read-only',
    not_reversible: "can't undo this one · c instead",
    // From the hand-back to the reply only Claude writes the doc
    held: 'Claude has the doc · this works again after its reply',
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
    var handing = held && s.hold.kind === 'hand_back';
    // A held hand-back counts what it carries, your answers too, plus what you wrote since
    var drafts = handing ? s.hold.count + answersIn(s, s.hold.n) + since(s) : c.drafts || 0;
    var p = (held && s.phrase_before_hold) || s.phrase || { stage: 'idle' };
    return {
      held: held,
      p: p,
      drafts: drafts,
      // A note written after approve reopens the round: until it goes back, it is your turn again
      reopened: p.stage === 'approved' && drafts > 0
    };
  }

  /** Whether `s` is approved and past its hold at `now`: the page then shows only dots. */
  BR.pure.clean = function (s, now) {
    if (!s || s.log_writable === false) return false;
    var v = view(s, now);
    return v.p.stage === 'approved' && !v.reopened;
  };

  /**
   * What the approve guard's first press says, or null when nothing stops it: the fold's count of
   * stops still holding something to check, questions included. `opts.key` is the turn key as the
   * button shows it.
   */
  BR.pure.guardPill = function (s, opts) {
    var n = (s.counts && s.counts.to_check) || 0;
    if (!n) return null;
    return n + ' to check · n next · ' + opts.key + ' again approves as is';
  };

  /**
   * A composer's keys: `⌘⏎` saves, then does what the turn button does now, so it names that and
   * goes while the button is disabled. With no button yet, a saved note makes it a hand-back.
   */
  BR.pure.composerKeys = function (b, mac) {
    var key = mac ? '⌘⏎' : 'Ctrl⏎';
    var turn = b && b.disabled ? [] : [key + (b && b.text === COPY_BUTTON ? ' save & copy hand-off line' : ' save & hand back')];
    return ['⏎ save', '⇧⏎ new line'].concat(turn, ['esc save & close']);
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

  /** The header phrase for `s` at server time `opts.now`: one event and at most one count. */
  BR.pure.formatPhrase = function (s, opts) {
    return phraseOf(s, opts, !!opts.full);
  };

  /** The phrase; `full` keeps the facts the header leaves to the tooltip. */
  function phraseOf(s, opts, full) {
    if (s.log_writable === false) return 'review unavailable · folder not writable';
    if (opts.stopped) return 'birta stopped · notes wait here';
    var parts = docPhrase(s, opts);
    var base = parts.head.concat(parts.counts, full ? parts.rest : [], parts.tail).join(' · ');
    return opts.dir ? base + folderItem(s, opts) : base;
  }

  // Hand-backs written this close together came from one ⌘⏎ over the folder
  var BATCH_MS = 2000;

  /**
   * The phrase's one folder menu item: a file of this doc's hand-back that no agent has picked up,
   * else how many other files there are, else nothing.
   */
  function folderItem(s, opts) {
    var others = BR.pure.otherFiles(opts.dir, opts.path);
    var p = view(s, opts.now).p;
    var mine = (p.stage === 'sent' || p.stage === 'picked_up') && p.sent ? p.sent.ts : null;
    var stranded = mine === null ? null : others.filter(function (f) {
      var o = f.summary;
      return !!o && o.stage === 'sent' && !!o.phrase && !!o.phrase.sent && Math.abs(o.phrase.sent.ts - mine) <= BATCH_MS;
    })[0];
    if (stranded) return ' · ' + stranded.paths[0] + ' not picked up ▾';
    return others.length ? ' · ' + plural(others.length, 'other file', 'other files') + ' ▾' : '';
  }

  /**
   * The phrase in parts: `head` (the event), `counts` (at most one), then `rest`, which only the
   * tooltip shows, then `tail`. Your notes are counted on the button, never here.
   */
  function docPhrase(s, opts) {
    var v = view(s, opts.now);
    var p = v.p;
    function parts(head, counts, tail, rest) { return { head: head, counts: counts || [], tail: tail || [], rest: rest || [] }; }
    if (s.writable === false) return parts(['read-only · notes only']);
    // A hand-back held after approving has no approve left to name
    var stage = p.stage === 'approved' && !s.approve ? 'drafts' : p.stage;
    switch (stage) {
      case 'sent':
        return parts(['sent ' + hhmm(p.sent.ts) + ' · not picked up yet']);
      case 'picked_up': {
        // The button can't count while the agent has the turn, so the phrase does
        var waiting = v.drafts ? [plural(v.drafts, 'note', 'notes') + ' for next hand-back'] : [];
        var quiet = silence(p, opts.now);
        return parts([cap(p.picked_up.by) + ' picked up ' + hhmm(p.picked_up.ts)], waiting,
          quiet != null ? ['no reply in ' + quiet + ' min'] : []);
      }
      case 'replied': {
        var r = p.replied;
        var left = (s.counts && s.counts.to_check) || 0;
        return parts([cap(r.by) + ' replied ' + hhmm(r.ts)], [left ? left + ' to check' : 'all checked']);
      }
      case 'approved': {
        var a = s.approve;
        var at = 'approved ' + hhmm(a.ts);
        var later = a.later ? [a.later + ' later'] : [];
        if (v.reopened) return parts([at, 'reopened'], [], [], later);
        if (a.changed_since) return parts([at, 'changed since'], [], [], later);
        return parts([at], [], ['c to reopen'], later);
      }
      default: {
        var drafter = s.owner && s.owner.drafted_by;
        return parts([drafter ? 'drafted by ' + cap(drafter) + ' · your turn' : 'your turn']);
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

  /** Whether a hand-back has sat unclaimed long enough that the agent needs the hand-off line. */
  function cue(s, p, now) {
    return p.stage === 'sent' && !!p.sent && !!s.hand_off && now - p.sent.ts >= CUE_MS;
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
    var b = turnButtonOf(s, opts);
    // With birta stopped nothing can be sent: the button keeps its words and takes no press
    if (b && opts.stopped) return button(b.text, false, true);
    return b;
  };

  function turnButtonOf(s, opts) {
    var v = view(s, opts.now);
    var p = v.p;
    if (v.held && s.hold.kind === 'approve') return button('Approve', true);
    // Inside a folder's hold the button counts the hand-back and what was written since
    if (opts.batch != null) return button('Hand back · ' + opts.batch, true);
    var t = opts.dir && opts.dir.totals;
    // Notes the page kept that no state shows yet still count: ⌘⏎ must never approve past them
    var mine = v.drafts + (opts.pending || 0);
    // This doc's own state can be ahead of the throttled listing, which may not count its newest note
    var total = t ? Math.max(t.drafts + (opts.pending || 0), t.drafts - listedHere(opts.dir, opts.path) + mine) : mine;
    if (opts.busy && total === 0) total = 1;
    // From the hand-back to the reply the turn is the agent's: your new notes wait for the next one
    var away = p.stage === 'sent' || p.stage === 'picked_up';
    if (total > 0 && (!away || total > mine)) return button('Hand back · ' + total, true);
    if (away) {
      if (cue(s, p, opts.now)) return button(COPY_BUTTON, true);
      var who = holder(s, p);
      return button('With ' + (who ? short(cap(who)) : 'the agent'), false, true);
    }
    // Text that moved on since the approve can be approved again
    if (p.stage === 'approved' && s.approve && !s.approve.changed_since) return button('Approved ✓', false, true);
    // A folder doc of yours that no agent has taken up has nothing to approve
    if (opts.folder && !(s.owner && s.owner.drafted_by) && !anyAgent(s)) return null;
    return button('Approve', true);
  }

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

  /** Notes written since the hold began: the drafts count leaves out what the hold covers. */
  function since(s) {
    return (s.counts && s.counts.drafts) || 0;
  }

  /** What the strip says during a hold, or null outside one: the same count the button shows. */
  BR.pure.holdPill = function (s, opts) {
    if (!holding(s, opts.now)) return null;
    if (s.hold.kind === 'approve') return 'approving · z to undo';
    var answers = answersIn(s, s.hold.n);
    // A note written during the hold joins it, as the button counts it
    var notes = s.hold.count + since(s);
    var parts = notes || !answers ? [plural(notes, 'note', 'notes')] : [];
    if (answers) parts.push(plural(answers, 'answer', 'answers'));
    return 'handing back · ' + parts.join(' · ') + ' · z to undo';
  };

  // A settled note folds to its dot; an agent question still taking answers never does
  var FOLDED = { handled: 1, answered: 1, later: 1 };

  /** Claude's news as `{key, kind, check}` per item, in walk order, keyed by the stop the walk puts it on. */
  function newsItems(s) {
    return BR.pure.walk(s).filter(function (w) { return BR.pure.isNews(w.kind); }).map(function (w) {
      return { key: w.stop, kind: w.kind, check: w.check };
    });
  }

  function focusKey(f) {
    if (!f || f.k === 'doc') return 'doc';
    if (f.k === 'removal') return 'r:' + f.change;
    if (f.k === 'blank') return 'q:' + f.id;
    return 'b:' + f.sp;
  }

  function onStop(i, f) {
    var at = i.at || {};
    if (f.k === 'doc') return at.kind === 'doc';
    return f.k === 'block' && at.kind !== 'doc' && !!at.loc && at.loc.sp === f.sp;
  }

  /**
   * The strip's two sides for the focused stop: `where` names it, and `keys` says what works there.
   * `focus` is `{k:"doc"}`, `{k:"block", sp, heading, kind}`, `{k:"removal", change, heading, kind}`
   * or `{k:"blank", id}`; `opts` is `{now, mode, composer, button, mac, selection, stopped, keys}`,
   * `mode` one of "raw", "reading" and "control". `N of M` counts the stops holding Claude's news.
   */
  BR.pure.stripText = function (s, focus, opts) {
    var o = opts || {};
    var f = focus || { k: 'doc' };
    function key(action) { return BR.pure.keyName(action, o.keys); }
    function row(where, keys) { return { where: where, keys: keys.join(' · ') }; }
    if (o.mode === 'raw') return row('source view', ['esc back']);
    if (o.mode === 'reading') return row('reading mode', ['esc back']);
    if (o.mode === 'control') return row('on page controls', ['esc back to the doc']);
    if (o.stopped) return row('birta stopped', ['notes wait here', '? keys']);
    if (o.composer) return row(o.composer, BR.pure.composerKeys(o.button, o.mac));
    if (s && BR.pure.clean(s, o.now)) return row('approved', [key('note') + ' reopen', '? keys']);
    var crumb = f.k === 'doc' ? 'whole doc'
      : f.k === 'blank' ? "Claude's question"
      : f.k === 'removal' ? (f.heading ? f.heading + ' › ' : '') + 'removed ' + (f.kind || 'block')
      : f.heading ? (f.kind ? f.heading + ' › ' + f.kind : f.heading) : f.kind || 'block';
    if (o.selection) return row(crumb, [key('note') + ' note', key('ask') + ' ask', key('no') + ' cut these words']);
    var stage = s ? view(s, o.now).p.stage : null;
    if (stage === 'sent' || stage === 'picked_up') {
      return row(crumb + ' · with Claude', [key('note') + ' note for next hand-back', '? keys']);
    }
    var items = s ? newsItems(s) : [];
    var stops = [];
    items.forEach(function (i) { if (stops.indexOf(i.key) < 0) stops.push(i.key); });
    var here = focusKey(f);
    var at = stops.indexOf(here);
    if (at >= 0) {
      var first = items.filter(function (i) { return i.key === here; })[0].kind;
      var place = (at + 1) + ' of ' + stops.length + ' · ';
      var next = key('next_item') + ' next';
      if (first === 'question') return row(place + crumb, [key('open') + ' answer', key('yes') + ' yes', key('no') + ' no', next]);
      if (first === 'removal') {
        return row(place + crumb, [key('yes') + ' keep the cut', key('revert_change') + ' put it back', key('note') + ' reply', next]);
      }
      if (first === 'change' && f.k !== 'doc') {
        var mine = items.filter(function (i) { return i.key === here && i.kind === 'change'; });
        // Reverted once each change there is, kept once each is kept or restored
        var reverted = mine.every(function (i) { return i.check === 'reverted'; });
        var kept = mine.every(function (i) { return i.check === 'kept' || i.check === 'restored'; });
        return row(place + crumb + " · Claude's change",
          [reverted ? 'reverted' : kept ? 'kept' : key('yes') + ' keep', key('revert_change') + (reverted ? ' restore' : ' revert'), key('note') + ' reply', next]);
      }
      return row(place + crumb + ' · Claude replied', [key('yes') + ' ok', key('note') + ' reply', next]);
    }
    var notes = s && f.k !== 'blank' && f.k !== 'removal' ? (s.items || []).filter(function (i) { return onStop(i, f); }) : [];
    if (notes.some(function (i) { return i.by && i.by.type === 'human' && i.status === 'draft'; })) {
      return row(crumb + ' · your note', [key('note') + ' edit note', key('undo') + ' undo', '? more']);
    }
    if (notes.some(function (i) { return FOLDED[i.status] && !(i.by && i.by.type === 'agent' && i.verdict === 'ask'); })) {
      return row(crumb + ' · settled note', [key('open') + ' open note', '? more']);
    }
    if (f.k === 'doc') return row(crumb, [key('note') + ' note on the whole doc', key('focus_next') + ' down', '? keys']);
    return row(crumb, [key('note') + ' note', key('ask') + ' ask', key('no') + ' cut', key('yes') + ' keep as is', '? more']);
  };

  /** The phrase menu's rows: `{id, text, op?, copy?}`, in order. */
  BR.pure.menuRows = function (s, opts) {
    if (s.log_writable === false) return [];
    var v = view(s, opts.now);
    var p = v.p;
    var rows = [];
    // Inside the hold z takes it back, so the menu only offers it after; last, as the menu opens on its first row
    var withdraw = null;
    if (p.stage === 'sent' && p.sent && p.sent.can_withdraw)
      withdraw = { id: 'withdraw', text: 'withdraw', op: { op: 'withdraw', to: p.sent.n } };
    if (p.stage === 'approved' && !v.reopened && s.approve && s.approve.can_withdraw)
      withdraw = { id: 'withdraw', text: 'withdraw', op: { op: 'withdraw', to: s.approve.n } };
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
    if (withdraw) rows.push(withdraw);
    return rows;
  };

  // Literal keys in the sheet, which no binding names
  var TAB = { key: 'Tab' };
  var ESC = { key: 'Esc' };
  var HELP = { key: '?' };
  // The sheet's groups: each row is `[actions, words]`; a row with no actions is words alone
  var SHEET = [
    ['decide', [
      [['yes'], 'keep as is / keep / ok'],
      [['no'], 'no: Claude cuts it'],
      [['later'], 'later: Claude leaves it'],
      [['your_call'], 'your call: Claude decides'],
      [[], 'on a block, a verdict is a one-word note to Claude']
    ]],
    ['write', [[['note'], 'note, or reply on Claude\'s change'], [['ask'], 'question to Claude']]],
    ['move', [
      [['focus_next', 'focus_prev'], 'next / previous stop'],
      [['next_item', 'prev_item'], 'next / previous to check'],
      [['focus_first', 'focus_last'], 'top / bottom'],
      [['next_file', 'prev_file'], 'next / previous file with notes'],
      [[TAB], 'the block\'s links, then the page controls'],
      [['open'], 'open / follow'],
      [[ESC], 'close']
    ]],
    ['take back', [[['undo'], 'take back your last action']]],
    ["Claude's changes", [[['revert_change'], "revert / restore Claude's change"]]],
    ['hand back / approve', [[['turn'], 'press the button']]],
    ['view', [
      [['toggle_reading'], 'reading mode'],
      [['toggle_dark'], 'dark'],
      [['focus_theme'], 'theme'],
      [['hide_marks'], 'hide or show marks'],
      [['toggle_raw'], 'source view'],
      [[HELP], 'this sheet']
    ]]
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
        var keys = r[0].map(function (a) { return a.key ? a.key : b[a] ? keyName(b[a], mac) : ''; })
          .filter(function (k) { return k; });
        // A row of words alone stays; a row whose keys are all unbound goes
        return { keys: keys, note: r[1], gone: r[0].length > 0 && !keys.length };
      }).filter(function (r) { return !r.gone; }).map(function (r) { return { keys: r.keys, note: r.note }; });
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

  var phrase, phraseText, phraseWords, turn, turnBtn, turnWords, turnKbd, pop, menu, sheet;
  var strip, stripBox, stripWhere, stripKeys;
  // Server time minus this page's clock; it only moves forward, so a passed hold never comes back
  var skew = null;
  var dir = null;
  var down = false;
  // A hand-back this page sent, until its ack: {cid, count}
  var handing = null;
  // Its batch once acked; another doc's hold the page's own state never shows
  var batch = null;
  var withdrawing = null;
  // The hold z withdrew: `{cid, n, acked}`
  var withdrawn = null;
  // The strip's message: {text, until?, arm?}, or {nack, until} for a refused op; an armed one waits for a key
  var message = null;
  // The approve guard's jump, until a key other than ⌘⏎ or its message going disarms it: {at, ids, left}
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

  function stopped() { return !!BR.life && BR.life.stopped(); }

  function opts(t) { return { now: t, dir: dir, path: CURRENT_PATH, folder: folderMode(), stopped: stopped() }; }

  function mac() { return /Mac/.test(navigator.platform); }

  /** The button's options: only in a folder do its count and hand-back take in every file. */
  function turnOpts(t) {
    var p = BR.pending;
    var b = batchHold(t);
    return { now: t, dir: folderMode() ? dir : null, path: CURRENT_PATH, folder: folderMode(), pending: p ? p.adds() : 0, busy: !!p && p.any(),
      stopped: stopped(), batch: b ? batchCount(b, t) : null };
  }

  /** This doc's notes not yet handed back, the page's own on their way included. */
  function ownDrafts() {
    var s = BR.currentState();
    return ((s && s.counts && s.counts.drafts) || 0) + (BR.pending ? BR.pending.adds() : 0);
  }

  /**
   * A folder hold's count: what it handed back, plus the notes this page has written since. Until
   * this doc's state shows its own hold, its drafts still hold what the batch took at the press.
   */
  function batchCount(b, t) {
    var s = BR.currentState();
    var fresh = !!s && !!s.hold && s.hold.kind === 'hand_back' && t < s.hold.not_before;
    return b.count + (fresh ? ownDrafts() : Math.max(0, ownDrafts() - b.base));
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
    holdEnd(s, t);
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
    renderStrip();
    // The composer's ⌘⏎ hint follows the button
    if (BR.flow && BR.flow.refreshHint) BR.flow.refreshHint();
    news(!!s && !!s.counts && s.counts.to_check > 0);
    if (menuOpen()) fillMenu();
    schedule(s, t);
  }

  // A hold this page watched run, `{n, kind, not_before}`, so its end gets its moment
  var watched = null;

  /** A hold this page watched ending says so: sent, or the approved moment. One withdrawn says nothing. */
  function holdEnd(s, t) {
    var h = s && s.hold;
    if (h && t < h.not_before) {
      watched = { n: h.n, kind: h.kind, not_before: h.not_before };
      return;
    }
    var w = watched;
    if (!w) return;
    if (t < w.not_before) {
      if (!h || h.n !== w.n) watched = null;
      return;
    }
    watched = null;
    if ((h && h.n !== w.n) || (withdrawn && withdrawn.n === w.n)) return;
    var stage = s && s.phrase ? s.phrase.stage : null;
    if (w.kind === 'approve') {
      if (stage === 'approved') say(APPROVED, { ms: MOMENT_MS, moment: true });
    } else if (stage === 'sent' || stage === 'picked_up' || stage === 'replied') say(SENT);
  }

  /** The message the strip shows now, or '' for none: a hold speaks over any other. */
  function messageText(s, t) {
    var hold = s && BR.pure.holdPill(s, { now: t });
    // A hold the page withdrew says so at once, before the state that drops it
    if (hold && withdrawn && withdrawn.n === s.hold.n && withdrawn.acked) hold = null;
    var b = batchHold(t);
    // A folder's hand-back counts every file's notes, whichever doc it was sent from
    if (b && !(hold && s.hold.kind === 'approve')) hold = 'handing back · ' + plural(batchCount(b, t), 'note', 'notes') + ' · z to undo';
    // A new hold speaks over whatever was said before it
    var n = b ? 'batch ' + b.batch : hold ? s.hold.n : null;
    if (n !== null && n !== heldN) setMessage(null);
    heldN = n;
    // A deadline runs on the page's own clock
    if (message && message.until && Date.now() >= message.until) setMessage(null);
    return hold ? (down ? 'connection lost · hand-back stands' : hold)
      : message ? (message.nack ? BR.pure.nackText(message.nack, s) : message.text) : '';
  }

  /** The viewer's mode the strip names, read from its globals, or a header control Tab reached. */
  function stripMode() {
    if (typeof viewMode !== 'undefined' && viewMode === 'raw') return 'raw';
    if (typeof isReadingMode !== 'undefined' && isReadingMode === true) return 'reading';
    return BR.pointing.keyedControl() ? 'control' : null;
  }

  /**
   * The strip: where focus is on the left, its keys or the message on the right. Before review
   * shows it appears only to carry a message or a selection's hint, and names no place.
   */
  function renderStrip() {
    if (!strip) return;
    var s = BR.currentState();
    var t = now();
    var text = messageText(s, t);
    var selecting = BR.pointing.selecting();
    var mode = stripMode();
    var chrome = shown(s);
    var on = chrome || !!text || selecting;
    strip.hidden = !on;
    document.documentElement.toggleAttribute('data-review-strip', on);
    if (!on) return;
    var r = BR.pure.stripText(s, BR.pointing.stripFocus(), {
      now: t,
      mode: mode,
      composer: BR.flow.composerLabel(),
      button: s ? BR.pure.turnButton(s, turnOpts(t)) : null,
      mac: mac(),
      selection: selecting,
      stopped: stopped(),
      keys: typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS
    });
    strip.toggleAttribute('data-message', !!text);
    strip.toggleAttribute('data-dim', mode === 'reading');
    var where = chrome ? r.where : '';
    if (stripWhere.textContent !== where) stripWhere.textContent = where;
    var parts = text ? [text] : r.keys.split(' · ');
    var was = Array.prototype.map.call(stripKeys.children, function (k) { return k.getAttribute('data-key'); });
    if (was.join('\n') !== parts.join('\n')) {
      stripKeys.textContent = '';
      parts.forEach(function (p, i) {
        var k = document.createElement('span');
        k.className = 'key';
        k.setAttribute('data-key', p);
        if (i) k.appendChild(document.createElement('span')).textContent = ' · ';
        k.appendChild(document.createTextNode(p));
        stripKeys.appendChild(k);
      });
    }
    fit(!!text);
  }

  /** Narrow widths drop keys from the right, but the last, `? keys`, always stays. */
  function fit(saying) {
    var keys = Array.prototype.slice.call(stripKeys.children);
    keys.forEach(function (k) { k.hidden = false; });
    if (saying) return;
    for (var i = keys.length - 2; i >= 0 && cut(); i--) keys[i].hidden = true;
    // The first key shown leads with no separator
    var lead = true;
    keys.forEach(function (k) {
      if (k.hidden) return;
      if (k.firstChild && k.firstChild.nodeType === 1) k.firstChild.hidden = lead;
      lead = false;
    });
  }

  function cut() {
    return stripWhere.scrollWidth > stripWhere.clientWidth + 1 || stripBox.scrollWidth > stripBox.clientWidth + 1;
  }

  var stripping = false;

  /** Draws the strip again once the change in hand is done: a focus, a key, a mode, a selection. */
  function stripSoon() {
    if (stripping) return;
    stripping = true;
    Promise.resolve().then(function () {
      stripping = false;
      renderStrip();
    });
  }

  /** Replaces the message; an armed one going takes its arm with it. */
  function setMessage(next) {
    var was = message;
    message = next;
    if (was && was.arm && was !== next && was.arm.off) was.arm.off();
  }

  /** Any key but the one an armed message waits for disarms it; a bare modifier is not a key yet. */
  function onAnyKey(e) {
    // A held key's repeats are not new keys
    if (!message || !message.arm || MODIFIERS[e.key] || e.repeat) return;
    if (message.arm.keep && message.arm.keep(e)) return;
    setMessage(null);
    render();
  }

  /** Re-renders when the next hold ends, the silence threshold passes or its minute ticks, or a message expires. */
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
    if (p.stage === 'sent' && p.sent && p.sent.ts + CUE_MS > t) waits.push(p.sent.ts + CUE_MS - t);
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
    if (r.copy && navigator.clipboard) copy(r.copy);
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

  /**
   * Shows `text` in the strip in place of the keys, or clears it for ''. It goes after 4 s, or
   * `opts.ms`; an armed one, `opts.arm` `{keep(e), off()}`, goes on the next key `keep` doesn't claim, and `off` runs then.
   */
  function say(text, opts) {
    var o = opts || {};
    setMessage(text ? { text: text, until: o.arm ? null : Date.now() + (o.ms || SAY_MS), arm: o.arm || null, moment: !!o.moment } : null);
    render();
  }

  /** Copies the hand-off line, and says what to do with it. */
  function copy(text) {
    navigator.clipboard.writeText(text);
    say(COPIED);
  }

  function act() {
    var s = BR.currentState();
    // A plain reader's ⌘⏎ never writes: review has to be on in the tab first
    if (!shown(s)) return;
    var b = BR.pure.turnButton(s, turnOpts(now()));
    if (!b || b.disabled) return;
    if (b.text === COPY_BUTTON) {
      if (s.hand_off && navigator.clipboard) copy(s.hand_off);
      return;
    }
    if (/^Hand back/.test(b.text)) handBackWhenAcked();
    // Inside an approve's hold the approve is already on its way
    else if (/^Approve/.test(b.text) && !(holding(s, now()) && s.hold.kind === 'approve')) approveGuarded(s);
  }

  /**
   * Approves, unless something is left to check: then the first press goes to its first stop and
   * says so, and a press past the dwell, while that message stands, approves as is.
   */
  function approveGuarded(s) {
    if (guard) {
      if (Date.now() - guard.at < DWELL_MS) return;
      setMessage(null);
      BR.op({ op: 'approve' });
      return;
    }
    var text = BR.pure.guardPill(s, { key: turnKbd.textContent });
    if (!text) {
      BR.op({ op: 'approve' });
      return;
    }
    // A count with no stop to land on still arms: approve is never silent
    BR.pointing.guard();
    var mine = guard = { at: Date.now(), ids: uncheckedIds(s), left: s.counts.to_check };
    setMessage({ text: text, arm: { keep: turnKey, off: function () { if (guard === mine) guard = null; } } });
    render();
  }

  /** Claude's news in `s` still to check, by the id its seen op names. */
  function uncheckedIds(s) {
    var out = {};
    BR.pure.attention(s).forEach(function (x) { out[x.id] = 1; });
    return out;
  }

  /**
   * Whether `s` brings news the arm never covered: an item outside its set, or a count above the
   * lowest one since, which your own checks lower.
   */
  function newNews(s) {
    var ids = uncheckedIds(s);
    for (var id in ids) if (!guard.ids[id]) return true;
    var n = (s.counts && s.counts.to_check) || 0;
    if (n > guard.left) return true;
    guard.left = n;
    return false;
  }

  function turnKey(e) {
    return BR.pure.keyAction(e, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS) === 'turn';
  }

  var clockMoved = false;

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
          var was = skew;
          skew = skew === null ? d : Math.max(skew, d);
          // The flow drew the first state before the server's time was known, which can put a hold already past
          clockMoved = was === null;
        }
        // News the arm never covered must be reached before approve as is counts for it
        if (guard && msg.path === CURRENT_PATH && msg.state && newNews(msg.state)) setMessage(null);
        break;
      case 'dir_state':
        dir = msg;
        break;
      case 'ack':
        if (message && message.nack) setMessage(null);
        if (handBackDue && !BR.pending.unacked()) {
          handBackDue = false;
          handBack();
        }
        // z withdraws this tab's own hand-back until it's picked up
        if (handing && msg.cid === handing.cid && msg.n != null) BR.pointing.remember({ kind: 'handback', cid: msg.cid, n: msg.n });
        if (handing && msg.cid === handing.cid && msg.batch != null) {
          batch = { batch: msg.batch, n: msg.n, count: handing.count, base: handing.base, not_before: msg.not_before };
        }
        if (handing && msg.cid === handing.cid) handing = null;
        if (msg.cid === withdrawing) batch = withdrawing = null;
        if (withdrawn && msg.cid === withdrawn.cid) withdrawn.acked = true;
        break;
      case 'nack':
        // The viewer's own writes, such as a task tick, are refused this way too
        if (msg.reason === 'held') setMessage({ nack: 'held', until: Date.now() + NACK_MS });
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
    if (clockMoved) {
      clockMoved = false;
      BR.flow.refresh();
      BR.lane.redraw();
    }
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
    handing = { cid: cid, count: count ? Number(count[1]) : 0, base: ownDrafts() };
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
    /** z withdrew the hold on line `n` with op `cid`: once acked, its message gives way to z's. */
    withdrew: function (cid, n) { withdrawn = { cid: cid, n: n, acked: false }; },
    init: function () {
      baseTitle = document.title.replace(/^● /, '');
      // The viewer's copy button speaks in the strip, as every other copy does
      var md = document.getElementById('copy-md-btn');
      if (md) md.addEventListener('click', function () { say('markdown copied'); });
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
      pop.root.append(menu, sheet);
      document.body.appendChild(pop.el);

      var st = host('birta-strip');
      strip = st.el;
      strip.hidden = true;
      stripBox = document.createElement('div');
      stripBox.className = 'strip';
      stripBox.setAttribute('role', 'status');
      stripWhere = document.createElement('span');
      stripWhere.className = 'where';
      stripWhere.setAttribute('data-part', 'where');
      stripKeys = document.createElement('span');
      stripKeys.className = 'keys';
      stripKeys.setAttribute('data-part', 'keys');
      stripBox.append(stripWhere, stripKeys);
      st.root.appendChild(stripBox);
      document.body.appendChild(strip);
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
      // What the strip names changes with the focus, the keys, the mode and the selection
      window.addEventListener('resize', stripSoon);
      document.addEventListener('input', stripSoon, true);
      document.addEventListener('focusin', stripSoon, true);
      document.addEventListener('focusout', stripSoon, true);
      document.addEventListener('keyup', stripSoon, true);
      document.addEventListener('selectionchange', stripSoon);
      render();
    },
    render: render,
    /** The turn button as it reads now, `{text, kbd, disabled}`, or null while it isn't shown. */
    button: function () {
      var s = BR.currentState();
      return shown(s) ? BR.pure.turnButton(s, turnOpts(now())) : null;
    },
    /** The turn button for the state as it stands, shown yet or not; null with no state. */
    buttonNow: function () {
      var s = BR.currentState();
      return s ? BR.pure.turnButton(s, turnOpts(now())) : null;
    },
    /**
     * What a composer's ⌘⏎ does once its note is saved (`saving`): the button's press, read before
     * the save. Approve becomes the hand-back of that note; a disabled button does nothing.
     */
    afterSave: function (saving) {
      var s = BR.currentState();
      var b = s ? BR.pure.turnButton(s, turnOpts(now())) : null;
      if (b && b.disabled) return null;
      if (b && b.text === COPY_BUTTON) {
        // Copied while the key's press still counts as the reader's own act
        if (saving && s.hand_off && navigator.clipboard) {
          navigator.clipboard.writeText(s.hand_off);
          // After the save's own message, which this one replaces
          setTimeout(function () { say(COPIED); }, 0);
        }
        return null;
      }
      return handBack;
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
    say: say,
    /** Shows `text` as a moment, for 6 s; without it, whether a moment shows now, which no tip may cover. */
    moment: function (text) {
      if (arguments.length) say(text, { ms: MOMENT_MS, moment: true });
      return !!message && !!message.moment && (!message.until || Date.now() < message.until);
    },
    says: function (text) { return !!message && message.text === text; },
    /** Whether Claude has the doc: from this page's hand-back, its hold included, to the reply. */
    held: function () {
      var s = BR.currentState();
      return !!handing || !!(s && s.held);
    },
    /** Shows a refused op's reason until the next acked op, a new hold, or 4 s. */
    nack: function (reason) {
      setMessage({ nack: reason, until: Date.now() + NACK_MS });
      render();
    },
    /** Whether the page shows the clean view: approved, and past the approve's hold. */
    clean: function () { return BR.pure.clean(BR.currentState(), now()); },
    renderStrip: renderStrip,
    /** The strip's top while it shows, else the window's bottom: the view ends there. */
    stripTop: function () {
      return strip && !strip.hidden && strip.getClientRects().length ? strip.getBoundingClientRect().top : innerHeight;
    }
  };
})();
