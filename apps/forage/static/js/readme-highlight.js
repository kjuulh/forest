(function () {
  function highlight() {
    if (typeof hljs === "undefined") return;
    document.querySelectorAll(".prose pre code").forEach(function (el) {
      hljs.highlightElement(el);
    });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", highlight);
  else highlight();
})();
