var BR = BR && BR.__birta === true ? BR : { __birta: true };
(function () {
  BR.pure = BR.pure || {};
  // How often a tab whose server stopped asks whether it is back
  var POLL_MS = 1000;
  var RECONNECTED_MS = 3000;
  var FOREIGN = 'another birta is on this port';
  var RECONNECTED = 'reconnected';

  /** The banner while the server is down, naming the command that brings it back. */
  BR.pure.stoppedText = function (restart) { return 'birta stopped · run ' + restart + ' to reconnect'; };

  /** What the banner's copy button copies: the restart, without opening a second tab. */
  BR.pure.restartLine = function (restart) { return restart + ' --no-open'; };

  var banner = null;
  var timer = null;
  var dial = null;
  var down = false;
  var leaving = false;
  // A drop while the page was going away, owed a redial if it comes back from the cache
  var missed = false;

  function show(on) {
    if (on) banner.setAttribute('data-shown', '');
    else banner.removeAttribute('data-shown');
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
    BR.chrome.renderStrip();
    later();
  }

  function onFrame(msg) {
    if (msg.type !== 'open' || !down) return;
    down = false;
    clearTimeout(timer);
    show(false);
    BR.chrome.renderStrip();
    BR.chrome.say(RECONNECTED, { ms: RECONNECTED_MS });
  }

  BR.life = {
    /** Whether the server went away and this tab waits for it. */
    stopped: function () { return down; },
    init: function () {
      var restart = BOOT.review.restart;
      // Without a served path there is nothing to check a restart against
      if (!restart || !BOOT.review.served) return;
      var made = BR.shadowHost('birta-stopped');
      banner = made.host;
      var box = document.createElement('div');
      box.className = 'stopped';
      box.setAttribute('role', 'alert');
      var words = document.createElement('span');
      words.textContent = BR.pure.stoppedText(restart);
      var copy = document.createElement('button');
      copy.type = 'button';
      copy.textContent = 'copy';
      copy.addEventListener('click', function () {
        if (navigator.clipboard) navigator.clipboard.writeText(BR.pure.restartLine(restart));
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
      birtaBus.on(onFrame);
    }
  };
})();
