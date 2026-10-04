var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  var ACTIONS = ['focus_next', 'focus_prev', 'note', 'ask', 'yes', 'no', 'later', 'your_call', 'next_item',
    'prev_item', 'next_file', 'prev_file', 'undo', 'hide_marks', 'open', 'turn', 'revert_change', 'focus_first', 'focus_last'];
  // A held key repeats; these write to the review file or flip a toggle, so they act once
  var ONCE = { yes: 1, no: 1, later: 1, your_call: 1, undo: 1, turn: 1, hide_marks: 1, revert_change: 1 };
  // What a composing IME or a dead key reports instead of a character
  var NOT_KEYS = { Dead: 1, Process: 1, Unidentified: 1 };
  // Held on their way to another key, so they never close the key sheet
  var MODIFIERS = { Shift: 1, Control: 1, Alt: 1, Meta: 1, CapsLock: 1, AltGraph: 1 };
  // Letters that already pilled `? for keys` in this tab
  var UNBOUND_KEY = 'birta-unbound-pilled';

  function has(parts, mod) { return parts.indexOf(mod) >= 0; }

  // The typed character decides, never the physical key, so every layout gets its own letters
  function matches(e, binding) {
    if (!binding) return false;
    var parts = binding.split('+');
    var key = parts.pop();
    if (/^[a-z]$/i.test(key)) {
      // Caps Lock types upper case without Shift, so letters compare case-blind and Shift decides
      if (!e.key || e.key.length !== 1 || e.key.toLowerCase() !== key.toLowerCase()) return false;
      if (e.shiftKey !== has(parts, 'Shift')) return false;
    } else {
      if (e.key !== key) return false;
      // A typed character may need Shift on some layouts; a named key takes Shift as a modifier
      if (key.length > 1 && e.shiftKey !== has(parts, 'Shift')) return false;
    }
    if (has(parts, 'Mod')) {
      if (!e.metaKey && !e.ctrlKey) return false;
    } else if (e.ctrlKey !== has(parts, 'Ctrl') || e.metaKey !== has(parts, 'Meta')) {
      return false;
    }
    return e.altKey === has(parts, 'Alt');
  }

  /** Whether a keydown matches one binding, as review's actions match theirs. */
  BR.pure.keyMatches = matches;

  var NAMED = { yes: 'y', no: 'x', note: 'c', undo: 'z', focus_next: 'j', revert_change: 'u', open: 'Enter' };

  /** How a pill names `action`'s key under `bindings`: a letter as typed, Enter as ⏎. */
  BR.pure.keyName = function (action, bindings) {
    var b = (bindings && bindings[action]) || NAMED[action] || '';
    if (b === 'Enter') return '⏎';
    return /^[a-z]$/i.test(b) ? b.toLowerCase() : b;
  };

  /** The page's own name for `action`'s key. */
  BR.keyName = function (action) {
    return BR.pure.keyName(action, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS);
  };

  /** The review action a keydown asks for under `bindings`, or null. Reads only the event. */
  BR.pure.keyAction = function (e, bindings) {
    if (!bindings || e.isComposing || e.keyCode === 229 || NOT_KEYS[e.key]) return null;
    for (var i = 0; i < ACTIONS.length; i++) {
      var action = ACTIONS[i];
      if (matches(e, bindings[action])) return e.repeat && ONCE[action] ? null : action;
    }
    return null;
  };

  function typing(t) {
    return !!t && (t.localName === 'input' || t.localName === 'textarea' || t.localName === 'select' || t.isContentEditable);
  }

  // A key pressed on the page's own controls (theme menu, buttons) is meant for that control
  function onControl(t) {
    return !!t && !!t.closest && !t.closest('#content') &&
      !!t.closest('button, select, input, textarea, [role="button"], [role="menu"], .theme-dropdown');
  }

  function reviewable() {
    var state = BR.currentState();
    return !(state && state.log_writable === false);
  }

  function bindings() { return typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS; }

  /** Runs the review action a key asks for; true when it acted. */
  function run(e) {
    var action = BR.pure.keyAction(e, bindings());
    if (!action || !reviewable()) return false;
    // Source view and reading mode hide the doc the key acts on
    BR.pointing.leaveModes();
    var row = action === 'revert_change' && BR.flow.openRow();
    var shown = !row && action === 'revert_change' && BR.flow.focusedRow();
    if (shown && shown.undone_by != null) {
      BR.chrome.pill('already reverted · ' + BR.keyName('note') + ' note to ask for it back');
      return true;
    }
    // The menu and what a key opens are never open together
    BR.chrome.closeMenu();
    return BR.pointing.act(action);
  }

  // Enter and Space on a button or link press it
  function presses(e, t) {
    return (e.key === 'Enter' || e.key === ' ') && !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey &&
      !!t && !!t.closest && !!t.closest('button, a, [role="button"]');
  }

  // When the last Tab was pressed, so a control it lands on can say how back
  var tabbedAt = -Infinity;
  var CONTROL_SAYS = 'on page controls · esc back to the doc';

  // A control reached by Tab names the way back to the doc
  function onFocusIn() {
    if (Date.now() - tabbedAt > 500 || !BR.pointing.pageControl()) return;
    BR.chrome.pill(CONTROL_SAYS);
  }

  // A review host's keys reach its own handlers and then fromHost, never this
  function onKey(e) {
    if (e.defaultPrevented || typing(e.target) || BR.isHost(e.target)) return;
    handle(e, false);
  }

  function plain(e) { return !e.altKey && !e.ctrlKey && !e.metaKey; }

  // A key from a note or a blank may cycle links; one from the header's controls keeps Tab's own meaning
  function inFlow(e) {
    return e.composedPath().some(function (n) { return n.localName === 'birta-flow'; });
  }

  /** Pills `? for keys` for a letter no binding uses, once per letter per tab, while review shows. */
  function unbound(e) {
    if (!plain(e) || e.shiftKey || e.repeat || !/^[a-z]$/i.test(e.key) || !BR.chrome.shown()) return;
    var letter = e.key.toLowerCase();
    var bindings = typeof KEYBINDINGS === 'undefined' ? {} : KEYBINDINGS;
    for (var action in bindings) {
      if (typeof bindings[action] === 'string' && bindings[action].toLowerCase() === letter) return;
    }
    var done = [];
    try { done = JSON.parse(sessionStorage.getItem(UNBOUND_KEY) || '[]'); } catch (err) {}
    if (done.indexOf(letter) >= 0) return;
    try { sessionStorage.setItem(UNBOUND_KEY, JSON.stringify(done.concat([letter]))); } catch (err) {}
    BR.chrome.pill('? for keys');
  }

  /**
   * The header control a key was pressed on, which a review key hands back to the doc: a page
   * control, or review's phrase or turn button. Null for anything else.
   */
  function headerControl(e, own) {
    if (!own) return onControl(e.target) ? e.target : null;
    var t = e.composedPath()[0];
    var root = t && t.getRootNode && t.getRootNode();
    return root && root.host && root.host.closest('.file-header, .header') ? t : null;
  }

  /** Acts on a key; `own` is for a key from inside a review host, whose controls are review's. */
  function handle(e, own) {
    if (e.key === 'Tab') tabbedAt = Date.now();
    if (BR.chrome.menuKey(e)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    // Any other key closes the sheet, then acts as it would
    if (e.key !== '?' && e.key !== 'Escape' && !MODIFIERS[e.key]) BR.chrome.closeKeys();
    if (e.key === 'Escape') {
      if (e.isComposing || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
      // One layer per press; an unconsumed Esc falls through to the viewer's reading mode
      if (BR.pointing.escape()) {
        e.preventDefault();
        e.stopPropagation();
      }
      return;
    }
    var ctl = headerControl(e, own);
    if (ctl) {
      // Tab moves between controls, and ⏎ or Space press one
      if (e.key === 'Tab' || presses(e, ctl)) return;
      var sheet = e.key === '?' && plain(e);
      if (!sheet && !(BR.pure.keyAction(e, bindings()) && reviewable())) {
        if (!own) return;
      } else {
        // The theme menu a click opened would stay over the page the key moves on
        var menu = ctl.closest && ctl.closest('.theme-dropdown');
        if (menu) menu.classList.remove('open');
        ctl.blur();
      }
    }
    // Shift or not, as layouts differ on whether ? needs it
    if (e.key === '?' && plain(e)) {
      BR.chrome.toggleKeys();
      e.preventDefault();
      return;
    }
    if (e.key === 'Tab' && plain(e)) {
      if ((!own || inFlow(e)) && BR.pointing.tab(e.shiftKey ? -1 : 1)) e.preventDefault();
      return;
    }
    if (run(e)) e.preventDefault();
    else unbound(e);
  }

  BR.keys = {
    /** A key inside a review host that its own handlers left: acts as it would on the page. */
    fromHost: function (e) {
      BR.pointing.keyInput(e);
      var t = e.composedPath()[0];
      if (e.defaultPrevented || typing(t) || presses(e, t)) return;
      handle(e, true);
    },
    init: function () {
      // Capture, so the viewer's own handler sees a consumed key as defaultPrevented
      document.addEventListener('keydown', onKey, true);
      document.addEventListener('focusin', onFocusIn);
    }
  };
})();
