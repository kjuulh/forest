(function () {
  if (window.__forageUi) return;
  window.__forageUi = true;

  function localiseTimes() {
    document.querySelectorAll("time[datetime]").forEach(function (el) {
      var d = new Date(el.getAttribute("datetime"));
      if (!isNaN(d)) el.title = d.toLocaleString();
    });
  }

  function showLoadTime() {
    var nav = performance.getEntriesByType("navigation")[0];
    var slot = document.getElementById("client-load-ms");
    if (!nav || !slot || !(nav.duration > 0)) return;
    var ms = nav.duration;
    slot.textContent = ms >= 10 ? Math.round(ms) + "ms" : ms.toFixed(1) + "ms";
    document.getElementById("client-load-time").hidden = false;
  }

  function closeNavMenus() {
    document.querySelectorAll("nav details[open]").forEach(function (el) {
      el.open = false;
    });
  }

  localiseTimes();
  document.addEventListener("sedx:load", localiseTimes);
  document.addEventListener("sedx:load", closeNavMenus);
  if (document.readyState === "complete") showLoadTime();
  else window.addEventListener("load", function () { setTimeout(showLoadTime, 0); });

  document.addEventListener("click", function (event) {
    var copy = event.target.closest("[data-copy]");
    if (copy) {
      var source = document.getElementById(copy.getAttribute("data-copy"));
      if (!source || !navigator.clipboard) return;
      navigator.clipboard.writeText(source.textContent.trim());
      copy.textContent = "Copied!";
      setTimeout(function () { copy.textContent = "Copy"; }, 1500);
      return;
    }
    var toggle = event.target.closest("[data-toggle]");
    if (toggle) {
      var target = document.getElementById(toggle.getAttribute("data-toggle"));
      if (target) target.classList.toggle("hidden");
    }
  });

  document.addEventListener(
    "submit",
    function (event) {
      var form = event.target;
      var message = form.getAttribute && form.getAttribute("data-confirm");
      if (message && !window.confirm(message)) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    },
    true
  );
})();
