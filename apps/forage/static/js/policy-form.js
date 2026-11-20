(function () {
  var controller = null;

  function init() {
    var typeSelect = document.getElementById("policy-type");
    if (typeSelect && typeSelect.dataset.policyFormReady) return;
    if (controller) controller.abort();
    controller = null;
    if (!typeSelect) return;
    typeSelect.dataset.policyFormReady = "1";
    controller = new AbortController();

    var desc = document.getElementById("policy-type-desc");
    var sections = {
      soak_time: document.getElementById("soak-time-fields"),
      branch_restriction: document.getElementById("branch-restriction-fields"),
      approval: document.getElementById("approval-fields"),
      supersede_pending: document.getElementById("supersede-pending-fields"),
    };

    function toggle() {
      var value = typeSelect.value;
      Object.keys(sections).forEach(function (key) {
        var el = sections[key];
        if (!el) return;
        var active = key === value;
        el.classList.toggle("hidden", !active);
        el.querySelectorAll("input, select").forEach(function (input) {
          input.disabled = !active;
        });
      });
      var option = typeSelect.options[typeSelect.selectedIndex];
      if (desc) desc.textContent = (option && option.getAttribute("data-description")) || "";
    }

    typeSelect.addEventListener("change", toggle, { signal: controller.signal });
    toggle();
  }

  init();
  document.addEventListener("sedx:load", init);
})();
