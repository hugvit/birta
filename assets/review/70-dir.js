var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};
  // Where n or p sent this tab, and which end of that page's walk to land on
  var LANDING = 'birta-landing';

  function has(f, path) { return (f.paths || []).indexOf(path) >= 0; }

  /** The listed files other than the one at `path`, under any of its paths, in listing order. */
  BR.pure.otherFiles = function (dir, path) {
    if (!dir || !dir.files) return [];
    return dir.files.filter(function (f) { return !has(f, path); });
  };

  /** A file row's note: its summary's phrase, `changed` for news alone, else nothing. */
  BR.pure.fileNote = function (f, opts) {
    if (!f.summary) return f.news ? 'changed' : '';
    // A summary carries no approve time
    if (f.summary.stage === 'approved') return 'approved';
    return BR.pure.formatPhrase({ phrase: f.summary.phrase, counts: f.summary.counts, owner: null }, { now: opts.now });
  };

  /** The page for a lexical path; each segment is encoded, so `#` and `?` stay in the name. */
  BR.pure.fileHref = function (path) {
    return '/view/' + path.split('/').map(encodeURIComponent).join('/');
  };

  /** Whether a listed file holds notes still open: drafts, or live notes on a round not yet approved. */
  BR.pure.hasNotes = function (f) {
    var s = f && f.summary;
    if (!s) return false;
    return (s.counts && s.counts.drafts > 0) || (s.notes > 0 && s.stage !== 'approved');
  };

  /**
   * The next (`step` 1) or previous (-1) file with news after `path`, round the end; null for none.
   * `withNotes` takes files with open notes too.
   */
  BR.pure.nextFile = function (dir, path, step, withNotes) {
    var files = (dir && dir.files) || [];
    var n = files.length;
    var at = -1;
    files.forEach(function (f, i) { if (has(f, path)) at = i; });
    var start = at >= 0 ? at : step > 0 ? -1 : n;
    for (var k = 1; k <= n; k++) {
      var f = files[(((start + step * k) % n) + n) % n];
      if ((f.news || (withNotes && BR.pure.hasNotes(f))) && !has(f, path)) return f.paths[0];
    }
    return null;
  };

  var dir = null;

  /** The doc a link opens, or null: only birta's own `/view/` links, decoded after the fragment goes. */
  function target(href) {
    if (!href || href.indexOf('/view/') !== 0) return null;
    try {
      return decodeURIComponent(href.slice(6).split('#')[0].split('?')[0]);
    } catch (e) {
      return null;
    }
  }

  function dots() {
    var content = document.getElementById('content');
    if (!content) return;
    var news = {};
    var notes = {};
    ((dir && dir.files) || []).forEach(function (f) {
      if (f.news) f.paths.forEach(function (p) { news[p] = true; });
      if (BR.pure.hasNotes(f)) f.paths.forEach(function (p) { notes[p] = true; });
    });
    content.querySelectorAll('a[href^="/view/"]').forEach(function (a) {
      var to = target(a.getAttribute('href'));
      mark(a, 'data-br-news', !!news[to]);
      mark(a, 'data-br-notes', !!notes[to]);
    });
  }

  function mark(a, attr, on) {
    if (on !== a.hasAttribute(attr)) a.toggleAttribute(attr, on);
  }

  function go(path, landing) {
    try {
      if (landing) sessionStorage.setItem(LANDING, JSON.stringify({ path: path, at: landing }));
    } catch (e) {}
    location.href = BR.pure.fileHref(path);
  }

  /** Lands where n or p meant to, once this page has its state; any other page just drops it. */
  function land() {
    if (!BR.currentState()) return;
    var landing = null;
    try {
      landing = JSON.parse(sessionStorage.getItem(LANDING) || 'null');
      sessionStorage.removeItem(LANDING);
    } catch (e) {}
    if (landing && landing.path === CURRENT_PATH) setTimeout(function () { BR.pointing.landAt(landing.at); }, 0);
  }

  function onFrame(msg) {
    if (msg.type === 'dir_state') {
      dir = msg;
      dots();
    } else if (msg.type === 'review_state' && msg.path === CURRENT_PATH) {
      land();
    }
  }

  BR.dir = {
    init: function () {
      birtaBus.on(onFrame);
      land();
      var content = document.getElementById('content');
      // Every way the doc is redrawn (patch, fetch, theme) replaces links, so dots follow the DOM
      if (content) new MutationObserver(dots).observe(content, { childList: true, subtree: true });
      dots();
    },
    listing: function () { return dir; },
    /** Shift+J and Shift+K: the next file with news or open notes. */
    step: function (step) {
      var path = BR.pure.nextFile(dir, CURRENT_PATH, step, true);
      if (path) go(path);
      else BR.chrome.pill('no other file has notes or news');
    },
    /**
     * n past this page's last stop, or p before its first: the next file with news, at its first
     * or last stop. False when there is none.
     */
    cross: function (step) {
      var path = BR.pure.nextFile(dir, CURRENT_PATH, step);
      if (path) go(path, step > 0 ? 'first' : 'last');
      return !!path;
    },
    open: function (href) { location.href = href; }
  };
})();
