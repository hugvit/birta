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
   * The next (`step` 1) or previous (-1) file with news after `path`; null past either end, as
   * nothing wraps. `withNotes` takes files with open notes too.
   */
  BR.pure.nextFile = function (dir, path, step, withNotes) {
    var files = (dir && dir.files) || [];
    var at = -1;
    files.forEach(function (f, i) { if (has(f, path)) at = i; });
    if (at < 0) at = step > 0 ? -1 : files.length;
    for (var i = at + step; i >= 0 && i < files.length; i += step) {
      var f = files[i];
      if ((f.news || (withNotes && BR.pure.hasNotes(f))) && !has(f, path)) return f.paths[0];
    }
    return null;
  };

  /** What ⇧J or ⇧K says on arriving at `path`: its name and its place among the files with notes or news. */
  BR.pure.fileStepSays = function (dir, path) {
    var withNotes = ((dir && dir.files) || []).filter(function (f) { return f.news || BR.pure.hasNotes(f); });
    var at = -1;
    withNotes.forEach(function (f, i) { if (has(f, path)) at = i; });
    return path + ' · ' + (at + 1) + ' of ' + withNotes.length + (withNotes.length === 1 ? ' file' : ' files') + ' with notes';
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

  /** Opens `path`, landing on its first or last walk stop (`at`), and saying `says` there. */
  function go(path, at, says) {
    try {
      if (at) sessionStorage.setItem(LANDING, JSON.stringify({ path: path, at: at, says: says || null }));
    } catch (e) {}
    location.href = BR.pure.fileHref(path);
  }

  // A followed link arrives on its file's first walk stop, as ⇧J does
  function onClick(e) {
    if (e.button !== 0 || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    var content = document.getElementById('content');
    if (!a || !content || !content.contains(a)) return;
    var to = target(a.getAttribute('href'));
    if (!to || to === CURRENT_PATH) return;
    try { sessionStorage.setItem(LANDING, JSON.stringify({ path: to, at: 'first', says: null })); } catch (err) {}
  }

  /** Lands where n or p meant to, once this page has its state; any other page just drops it. */
  function land() {
    if (!BR.currentState()) return;
    var landing = null;
    try {
      landing = JSON.parse(sessionStorage.getItem(LANDING) || 'null');
      sessionStorage.removeItem(LANDING);
    } catch (e) {}
    if (landing && landing.path === CURRENT_PATH) setTimeout(function () { BR.pointing.landAt(landing.at, landing.says); }, 0);
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
      document.addEventListener('click', onClick);
      land();
      var content = document.getElementById('content');
      // Every way the doc is redrawn (patch, fetch, theme) replaces links, so dots follow the DOM
      if (content) new MutationObserver(dots).observe(content, { childList: true, subtree: true });
      dots();
    },
    listing: function () { return dir; },
    /** Shift+J and Shift+K: the next file with news or open notes, never round the end. */
    step: function (step) {
      var path = BR.pure.nextFile(dir, CURRENT_PATH, step, true);
      if (path) return go(path, 'first', BR.pure.fileStepSays(dir, path));
      if (!BR.pure.nextFile(dir, CURRENT_PATH, -step, true)) return BR.chrome.say('no other file has notes or news');
      var back = BR.keyName(step > 0 ? 'prev_file' : 'next_file').replace(/^Shift\+/, '⇧');
      BR.chrome.say('no more files with notes · ' + back + ' back');
    },
    open: function (href) { location.href = href; }
  };
})();
