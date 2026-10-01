var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};

  var ACTIONS = ['focus_next', 'focus_prev', 'note', 'ask', 'yes', 'no', 'later', 'your_call', 'next_item',
    'prev_item', 'next_file', 'prev_file', 'undo', 'hide_marks', 'open', 'turn'];
  // A held key repeats; these write to the review file or flip a toggle, so they act once
  var ONCE = { yes: 1, no: 1, later: 1, your_call: 1, undo: 1, turn: 1, hide_marks: 1 };
  // What a composing IME or a dead key reports instead of a character
  var NOT_KEYS = { Dead: 1, Process: 1, Unidentified: 1 };

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
    if (state && state.log_writable === false) return false;
    // The raw view shows no doc to point at
    return typeof viewMode === 'undefined' || viewMode !== 'raw';
  }

  /** Runs the review action a key asks for; true when it acted. */
  function run(e) {
    var action = BR.pure.keyAction(e, typeof KEYBINDINGS === 'undefined' ? null : KEYBINDINGS);
    if (!action || !reviewable()) return false;
    // The menu and what a key opens are never open together
    BR.chrome.closeMenu();
    return BR.pointing.act(action);
  }

  // Enter and Space on a button or link press it
  function presses(e, t) {
    return (e.key === 'Enter' || e.key === ' ') && !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey &&
      !!t && !!t.closest && !!t.closest('button, a, [role="button"]');
  }

  // A review host's keys reach its own handlers and then fromHost, never this
  function onKey(e) {
    if (e.defaultPrevented || typing(e.target) || BR.isHost(e.target)) return;
    handle(e, false);
  }

  /** Acts on a key; `own` is for a key from inside a review host, whose controls are review's. */
  function handle(e, own) {
    if (BR.chrome.menuKey(e)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (e.key === 'Escape') {
      if (e.isComposing || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
      // One layer per press; an unconsumed Esc falls through to the viewer's reading mode
      if (BR.pointing.escape()) {
        e.preventDefault();
        e.stopPropagation();
      }
      return;
    }
    if (!own && onControl(e.target)) return;
    if (run(e)) e.preventDefault();
  }

  BR.keys = {
    /** A key inside a review host that its own handlers left: acts as it would on the page. */
    fromHost: function (e) {
      var t = e.composedPath()[0];
      if (e.defaultPrevented || typing(t) || presses(e, t)) return;
      handle(e, true);
    },
    init: function () {
      // Capture, so the viewer's own handler sees a consumed key as defaultPrevented
      document.addEventListener('keydown', onKey, true);
    }
  };
})();
