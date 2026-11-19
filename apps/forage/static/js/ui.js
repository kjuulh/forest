(function () {
  if (window.__forageUi) return;
  window.__forageUi = true;

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
