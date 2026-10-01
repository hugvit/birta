var BR = typeof BR === 'undefined' ? {} : BR;
(function () {
  BR.pure = BR.pure || {};

  var MIN = 60000;
  var NAME_CHARS = 24;
  var NACK_MS = 4000;
  var COPY = 'copy hand-off line';
  var NACKS = {
    changed_since: 'changed since · try again',
    nothing_to_send: 'nothing to hand back',
    hold_pending: 'already handing back',
    // A read-only doc can't take an undo at all, so a refusal blames the review file
    read_only: 'review file is read-only',
    not_reversible: "can't undo this one · c instead"
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
    return {
      held: held,
      p: (held && s.phrase_before_hold) || s.phrase || { stage: 'idle' },
      drafts: (c.drafts || 0) + (held && s.hold.kind === 'hand_back' ? s.hold.count : 0),
      decided: c.decided || 0,
      items: c.items || 0,
      notHandled: c.not_handled || 0,
      unseen: c.unseen || 0,
      asks: c.open_asks || 0
    };
  }

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

  /** The header phrase for `s` at server time `opts.now` (design call 5, r-frontend §11). */
  BR.pure.formatPhrase = function (s, opts) {
    if (s.log_writable === false) return 'review unavailable · folder not writable';
    if (s.writable === false) return 'read-only · notes only';
    var v = view(s, opts.now);
    var p = v.p;
    // Mode flips on the first item verdict; doc-level notes never flip it
    function yours() {
      return v.decided > 0 ? v.decided + ' of ' + v.items + ' decided' : 'your turn · ' + plural(v.drafts, 'note', 'notes');
    }
    switch (p.stage) {
      case 'sent':
        return 'sent ' + hhmm(p.sent.ts) + ' · not picked up yet';
      case 'picked_up': {
        var out = [cap(p.picked_up.by) + ' picked up ' + hhmm(p.picked_up.ts)];
        if (v.drafts) {
          out.push((v.decided > 0 ? v.drafts + ' decided' : plural(v.drafts, 'note', 'notes')) + ' for next hand-back');
        }
        var quiet = silence(p, opts.now);
        if (quiet != null) out.push('no reply in ' + quiet + ' min', 'release');
        return out.join(' · ');
      }
      case 'replied': {
        if (v.drafts) return yours() + (v.notHandled ? ' · ' + v.notHandled + ' not handled' : '');
        var r = p.replied;
        var tally = [[r.done, 'done'], [r.answered, 'answered'], [r.asked, 'for you'], [r.unasked, 'unasked'],
          [r.not_handled, 'not handled']];
        return [cap(r.by) + ' replied ' + hhmm(r.ts)].concat(
          tally.filter(function (t) { return t[0] > 0; }).map(function (t) { return t[0] + ' ' + t[1]; })
        ).join(' · ');
      }
      case 'approved': {
        var a = s.approve;
        return 'approved ' + hhmm(a.ts) + (a.changed_since ? ' · changed since' : '') + (a.later ? ' · ' + a.later + ' later' : '');
      }
      case 'drafts':
        return yours();
      default: {
        var drafter = s.owner && s.owner.drafted_by ? 'drafted by ' + cap(s.owner.drafted_by) + ' · ' : '';
        return drafter + (v.items ? v.items + ' items · your turn' : 'your turn');
      }
    }
  };

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
    if (v.drafts > 0) {
      var t = opts.dir && opts.dir.totals;
      if (t && t.unseen_files) return button('Hand back · ' + t.drafts + ' · ' + plural(t.unseen_files, 'file', 'files') + ' unseen');
      return button('Hand back · ' + v.drafts, true);
    }
    if (p.stage === 'sent' || p.stage === 'picked_up') {
      var who = holder(s, p);
      return button('With ' + (who ? short(cap(who)) : 'the agent'), false, true);
    }
    if (p.stage === 'approved') return s.approve.changed_since ? button('Hand back · 0', false, true) : button('Approved', false, true);
    return approve();
  };

  /** What the pill says during a hold, or null outside one. */
  BR.pure.holdPill = function (s, opts) {
    if (!holding(s, opts.now)) return null;
    return s.hold.kind === 'approve' ? 'approving · z to undo' : 'handing back · ' + plural(s.hold.count, 'note', 'notes') + ' · z to undo';
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
    return rows;
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

  var phrase, phraseText, phraseWords, turn, turnBtn, turnWords, turnKbd, pop, menu, pill;
  // Server time minus this page's clock; it only moves forward, so a passed hold never comes back
  var skew = null;
  var dir = null;
  var down = false;
  // The pill's own message: {text} from pointing, or {nack, until} for a refused op
  var message = null;
  var heldN = null;
  var rows = [];
  var cur = 0;
  var timer = null;
  var baseTitle = '';
  var plainIcon = null;
  var dottedIcon = null;

  function now() { return Date.now() + (skew || 0); }

  function host(tag) {
    var el = document.createElement(tag);
    var root = el.attachShadow({ mode: 'closed' });
    var style = document.createElement('style');
    style.textContent = BR.shadowCss || '';
    root.appendChild(style);
    BR.FLOW_HOSTS.add(el);
    return { el: el, root: root };
  }

  function active() {
    try { return sessionStorage.getItem('birta-review-active') === '1'; } catch (e) { return false; }
  }

  /** Design call 4: a review file, a drafter, or review started in this tab. */
  function shown(s) {
    return !!s && ((s.log && s.log.lines > 0) || !!(s.owner && s.owner.drafted_by) || active());
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
    var b = on ? BR.pure.turnButton(s, { now: t, dir: dir }) : null;
    show(turn, !!b);
    // Text changes in place: WebKit drops a click whose pressed node was replaced before release
    if (on) {
      var text = BR.pure.formatPhrase(s, { now: t, dir: dir });
      if (phraseWords.data !== text) phraseWords.data = text;
      if (phrase.getAttribute('title') !== text) phrase.setAttribute('title', text);
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
    // A new hold speaks over whatever the pill said before it
    var n = hold ? s.hold.n : null;
    if (n !== null && n !== heldN) message = null;
    heldN = n;
    // A refusal's deadline runs on the page's own clock
    if (message && message.until && Date.now() >= message.until) message = null;
    var text = hold ? (down ? 'connection lost · hand-back stands' : hold)
      : message ? (message.nack ? BR.pure.nackText(message.nack, s) : message.text) : '';
    pill.textContent = text;
    popover(pill, !!text);
  }

  /** Re-renders when the next hold ends, the silence threshold passes or its minute ticks, or a nack pill expires. */
  function schedule(s, t) {
    clearTimeout(timer);
    var waits = [];
    if (s && s.hold && s.hold.not_before > t) waits.push(s.hold.not_before - t);
    var p = (s && s.phrase) || {};
    var quiet = p.stage === 'picked_up' && p.picked_up && p.picked_up.silence;
    if (quiet) {
      var due = quiet.since_ts + quiet.threshold_ms;
      waits.push(due > t ? due - t : MIN - ((t - quiet.since_ts) % MIN));
    }
    if (message && message.until) waits.push(message.until - Date.now());
    if (!waits.length) return;
    timer = setTimeout(function () {
      render();
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

  function fillMenu() {
    var s = BR.currentState();
    rows = s ? BR.pure.menuRows(s, { now: now(), dir: dir }) : [];
    cur = Math.max(0, Math.min(cur, rows.length - 1));
    // Rows are kept and rewritten in place, so a push mid-click doesn't lose the click
    while (menu.children.length > rows.length) menu.lastElementChild.remove();
    rows.forEach(function (r, i) {
      var row = menu.children[i];
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('role', 'menuitem');
        row.appendChild(document.createTextNode(''));
        row.addEventListener('click', function () { pick(i); });
        menu.appendChild(row);
      }
      if (row.firstChild.data !== r.text) row.firstChild.data = r.text;
      row.className = 'row' + (i === cur ? ' current' : '');
    });
  }

  function openMenu() {
    cur = 0;
    fillMenu();
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
    // Focus left in the popover's host would make the next key a host's, which review ignores
    if (document.activeElement === pop.el) menu.blur();
    return true;
  }

  function pick(i) {
    var r = rows[i];
    closeMenu();
    if (!r) return;
    if (r.op) BR.op(r.op);
    if (r.copy && navigator.clipboard) navigator.clipboard.writeText(r.copy);
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
    // Any other review key closes the menu, then acts as it would on the page
    if (!mine) {
      closeMenu();
      mine = BR.keys.run(e);
    }
    if (mine) {
      e.preventDefault();
      e.stopPropagation();
    }
  }

  // --- The turn ---

  function act() {
    var s = BR.currentState();
    // A plain reader's ⌘⏎ never writes: review has to be on in the tab first
    if (!shown(s)) return;
    var b = BR.pure.turnButton(s, { now: now(), dir: dir });
    if (!b || b.disabled) return;
    if (/^Hand back/.test(b.text)) BR.op({ op: 'hand_back', scope: 'doc' });
    // With unseen changes the guard steps through them instead
    else if (/ unseen$/.test(b.text)) BR.pointing.toUnseen();
    else if (b.text === 'Approve' || / unanswered$/.test(b.text)) BR.op({ op: 'approve' });
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
        break;
      case 'dir_state':
        dir = msg;
        break;
      case 'ack':
        if (message && message.nack) message = null;
        break;
    }
    render();
  }

  BR.chrome = {
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
        if (!closeMenu()) openMenu();
      });
      phraseText.addEventListener('keydown', function (e) {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        if (!closeMenu()) openMenu();
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
      pop.root.append(pill, menu);
      document.body.appendChild(pop.el);
      // Pointer only: a scripted click elsewhere (the theme menu's) leaves it open
      document.addEventListener('pointerdown', function (e) {
        var path = e.composedPath();
        if (path.indexOf(pop.el) < 0 && path.indexOf(phrase) < 0) closeMenu();
      }, true);

      birtaBus.on(onFrame);
      render();
    },
    render: render,
    /** Server time, as this page knows it. */
    now: now,
    turn: act,
    menuKey: menuKey,
    closeMenu: closeMenu,
    /** Esc's menu layer: true when it closed the menu. */
    escape: closeMenu,
    /** Shows `text` in the pill, or clears it for ''. */
    pill: function (text) {
      message = text ? { text: text } : null;
      render();
    },
    pillSays: function (text) { return !!message && message.text === text; },
    /** Shows a refused op's reason until the next acked op, a new hold, or 4 s. */
    nack: function (reason) {
      message = { nack: reason, until: Date.now() + NACK_MS };
      render();
    }
  };
})();
