(function () {
  function highlight() {
    if (typeof hljs === "undefined") {
      var script = document.querySelector('script[src*="highlight.min.js"]');
      if (script) script.addEventListener("load", highlight, { once: true });
      return;
    }
    document.querySelectorAll(".prose pre code:not([data-highlighted])").forEach(function (el) {
      hljs.highlightElement(el);
    });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", highlight);
  else highlight();
  document.addEventListener("sedx:load", highlight);
})();
