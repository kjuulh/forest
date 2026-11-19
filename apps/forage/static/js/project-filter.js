(function () {
  document.addEventListener("input", function (event) {
    var input = event.target.closest && event.target.closest("[data-project-filter]");
    if (!input) return;
    var q = input.value.toLowerCase();
    document.querySelectorAll(".project-card").forEach(function (card) {
      card.style.display = card.dataset.name.toLowerCase().includes(q) ? "" : "none";
    });
  });
})();
