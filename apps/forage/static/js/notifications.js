(function () {
  var userToggled = new Set();
  var timer = null;

  function container() {
    return document.getElementById("notifications-list");
  }

  document.addEventListener("toggle", function (e) {
    var details = e.target;
    if (!details || details.tagName !== "DETAILS") return;
    if (!details.closest("#notifications-list")) return;
    var slug = details.dataset.slug;
    if (!slug) return;
    if (details.open) {
      userToggled.add(slug);
    } else {
      userToggled.delete(slug);
    }
  }, true);

  function findDetails(list, slug) {
    var all = list.querySelectorAll("details[data-slug]");
    for (var i = 0; i < all.length; i++) {
      if (all[i].dataset.slug === slug) return all[i];
    }
    return null;
  }

  function stop() {
    if (timer) clearInterval(timer);
    timer = null;
  }

  function start() {
    if (timer || document.hidden || !container()) return;
    timer = setInterval(refresh, 10000);
  }

  async function refresh() {
    if (!container()) {
      stop();
      userToggled.clear();
      return;
    }
    try {
      var res = await fetch("/notifications?_partial=1", { credentials: "same-origin" });
      if (!res.ok) return;
      var html = await res.text();
      var list = container();
      if (!list) {
        stop();
        return;
      }
      list.innerHTML = html;
      userToggled.forEach(function (slug) {
        var el = findDetails(list, slug);
        if (el) {
          el.open = true;
        } else {
          userToggled.delete(slug);
        }
      });
    } catch (e) {}
  }

  document.addEventListener("visibilitychange", function () {
    if (document.hidden) {
      stop();
    } else if (container()) {
      refresh();
      start();
    }
  });

  document.addEventListener("sedx:load", function () {
    if (container()) {
      start();
    } else {
      stop();
      userToggled.clear();
    }
  });

  start();
})();
