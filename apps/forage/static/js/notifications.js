(function () {
  var container = document.getElementById("notifications-list");
  if (!container) return;

  var userToggled = new Set();
  container.addEventListener("toggle", function (e) {
    var details = e.target;
    if (details.tagName !== "DETAILS") return;
    var slug = details.dataset.slug;
    if (!slug) return;
    if (details.open) {
      userToggled.add(slug);
    } else {
      userToggled.delete(slug);
    }
  }, true);

  function findDetails(slug) {
    var all = container.querySelectorAll("details[data-slug]");
    for (var i = 0; i < all.length; i++) {
      if (all[i].dataset.slug === slug) return all[i];
    }
    return null;
  }

  async function refresh() {
    try {
      var res = await fetch("/notifications?_partial=1", { credentials: "same-origin" });
      if (!res.ok) return;
      container.innerHTML = await res.text();
      userToggled.forEach(function (slug) {
        var el = findDetails(slug);
        if (el) {
          el.open = true;
        } else {
          userToggled.delete(slug);
        }
      });
    } catch (e) {}
  }

  var timer = setInterval(refresh, 10000);

  document.addEventListener("visibilitychange", function () {
    if (document.hidden) {
      clearInterval(timer);
      timer = null;
    } else {
      refresh();
      timer = setInterval(refresh, 10000);
    }
  });
})();
