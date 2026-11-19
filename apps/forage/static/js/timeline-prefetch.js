(function () {
  var el = document.currentScript;
  if (!el) return;
  var url = el.getAttribute("data-timeline-url");
  var store = (window.__forestTimeline = window.__forestTimeline || {});
  store[url] = fetch(url, { credentials: "same-origin" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .catch(function () { return null; });
})();
