(function () {
  var el = document.currentScript;
  if (!el) return;
  var url = el.getAttribute("data-timeline-url");
  if (el.getAttribute("data-include-graph") === "true") url += "?include_graph=true";
  var store = (window.__forestTimeline = window.__forestTimeline || {});
  function prime() {
    if (document.visibilityState !== "visible") return;
    document.removeEventListener("visibilitychange", prime);
    store[url] = store[url] || fetch(url, { credentials: "same-origin" })
      .then(function (r) { return r.ok ? r.json() : null; })
      .catch(function () { return null; });
  }
  document.addEventListener("visibilitychange", prime);
  prime();
})();
