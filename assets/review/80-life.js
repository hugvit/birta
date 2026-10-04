var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};
  // How often a tab whose server stopped asks whether it is back
  var POLL_MS = 1000;
  var FOREIGN = 'another birta is on this port';
  var RECONNECTED = 'reconnected · nothing lost';
  var COPIED = 'copied · run it in a terminal';
  // Above the doc, clear of the header it would cover
  var BELOW_HEADER = 8;

  /** The banner while the server is down, naming the command that brings it back. */
  BR.pure.stoppedText = function (restart) { return 'birta stopped · run ' + restart + ' to reconnect'; };

  /** What the banner's copy button copies: the server's line as shown, which opens no second tab. */
  BR.pure.restartLine = function (restart) { return restart; };

  function replyTurn(s) { return s && s.phrase && s.phrase.replied ? s.phrase.replied.turn : null; }

  /** What Claude's reply says when it lands: its count, and where the walk key `n` starts. */
  function repliedText(s, n) {
    return 'Claude replied · ' + ((s.counts && s.counts.to_check) || 0) + ' to check · ' + n + ' starts at the top';
  }

  /**
   * What a tab coming back says, from the state it left (`before`) to the one it has (`after`):
   * a reply it hasn't shown, a hand-back still with Claude that moved on, or null when nothing changed.
   */
  BR.pure.backText = function (before, after, n) {
    if (!before || !after) return null;
    var turn = replyTurn(after);
    if (turn != null && turn !== replyTurn(before)) return repliedText(after, n);
    var stage = after.phrase && after.phrase.stage;
    if (stage !== 'sent' && stage !== 'picked_up') return null;
    var was = before.phrase || {};
    var pickedNow = after.phrase.picked_up ? after.phrase.picked_up.n : null;
    var pickedThen = was.picked_up ? was.picked_up.n : null;
    return stage !== was.stage || pickedNow !== pickedThen ? 'still with Claude' : null;
  };

  var banner = null;
  var box = null;
  // The reply turn this tab last showed; undefined until its first state
  var shownTurn;
  // The state the tab had when it went to the back, while it is there
  var away = null;
  var timer = null;
  var dial = null;
  var down = false;
  var leaving = false;
  // A drop while the page was going away, owed a redial if it comes back from the cache
  var missed = false;

  function show(on) {
    if (on) {
      var header = document.querySelector('header.header');
      box.style.top = (header ? Math.max(0, header.getBoundingClientRect().bottom) : 0) + BELOW_HEADER + 'px';
      banner.setAttribute('data-shown', '');
    } else banner.removeAttribute('data-shown');
  }

  function later() {
    clearTimeout(timer);
    timer = setTimeout(check, POLL_MS);
  }

  /** The BOOT of the page the restarted server renders here, or of its entry page. */
  function freshBoot() {
    function boot(url) {
      return fetch(url, { cache: 'no-store' }).then(function (r) {
        if (!r.ok) throw new Error(String(r.status));
        return r.text();
      }).then(function (html) {
        var el = new DOMParser().parseFromString(html, 'text/html').getElementById('birta-boot');
        return JSON.parse(el.textContent);
      });
    }
    return boot(location.pathname).catch(function () { return boot('/'); });
  }

  /** Reconnects only to a birta serving what this tab's server served. */
  function check() {
    fetch('/birta/whoami', { cache: 'no-store' }).then(function (r) {
      return r.ok ? r.json() : null;
    }).then(function (me) {
      if (!me || me.served !== BOOT.review.served) {
        if (me && !BR.chrome.says(FOREIGN)) BR.chrome.say(FOREIGN);
        return later();
      }
      // The restarted server checks the beacon against its own token
      return freshBoot().then(function (fresh) {
        if (!fresh.review || !fresh.review.token) return later();
        BOOT.review.token = fresh.review.token;
        dial();
      });
    }).catch(later);
  }

  function onDown(connect) {
    dial = connect;
    if (leaving) {
      missed = true;
      return;
    }
    down = true;
    show(true);
    BR.chrome.render();
    // Notes made now say they wait
    BR.flow.refresh();
    later();
  }

  function onFrame(msg) {
    if (msg.type === 'review_state') return onState(msg);
    if (msg.type !== 'open' || !down) return;
    down = false;
    clearTimeout(timer);
    show(false);
    BR.chrome.render();
    BR.flow.refresh();
    BR.chrome.moment(RECONNECTED);
  }

  function hidden() { return document.visibilityState === 'hidden'; }

  /** Claude's reply landing while the tab is in view says so; one that lands while it's away waits for it. */
  function onState(msg) {
    var s = msg.state;
    if (msg.path !== CURRENT_PATH || !s || msg.rev !== s.rev) return;
    var turn = replyTurn(s);
    if (shownTurn === undefined || hidden()) {
      if (shownTurn === undefined) shownTurn = turn;
      return;
    }
    if (turn != null && turn !== shownTurn) BR.chrome.moment(repliedText(s, BR.keyName('next_item')));
    shownTurn = turn;
  }

  function onVisibility() {
    var s = BR.currentState();
    if (hidden()) {
      away = s;
      return;
    }
    var said = BR.pure.backText(away, s, BR.keyName('next_item'));
    away = null;
    if (s && shownTurn !== undefined) shownTurn = replyTurn(s);
    if (said) BR.chrome.moment(said);
  }

  BR.life = {
    /** Whether the server went away and this tab waits for it. */
    stopped: function () { return down; },
    init: function () {
      birtaBus.on(onFrame);
      document.addEventListener('visibilitychange', onVisibility);
      var restart = BOOT.review.restart;
      // Without a served path there is nothing to check a restart against
      if (!restart || !BOOT.review.served) return;
      var made = BR.shadowHost('birta-stopped');
      banner = made.host;
      box = document.createElement('div');
      box.className = 'stopped';
      box.setAttribute('role', 'alert');
      var words = document.createElement('span');
      words.textContent = BR.pure.stoppedText(restart);
      var copy = document.createElement('button');
      copy.type = 'button';
      copy.textContent = 'copy';
      copy.addEventListener('click', function () {
        if (!navigator.clipboard) return;
        navigator.clipboard.writeText(BR.pure.restartLine(restart));
        BR.chrome.say(COPIED);
      });
      box.append(words, copy);
      made.root.appendChild(box);
      document.body.appendChild(banner);
      addEventListener('pagehide', function () { leaving = true; });
      addEventListener('pageshow', function () {
        leaving = false;
        if (missed) {
          missed = false;
          onDown(dial);
        }
      });
      birtaBus.redial = onDown;
    }
  };
})();
