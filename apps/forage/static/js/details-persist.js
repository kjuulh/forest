/**
 * Persists the open/closed state of <details> elements inside [data-release]
 * cards across page reloads using sessionStorage.
 *
 * Key format: `details:<page-path>:<release-slug>`
 */
(function () {
  const prefix = "details:" + location.pathname + ":";

  // Restore open state on load
  document.querySelectorAll("[data-release][data-release-slug]").forEach((card) => {
    const slug = card.dataset.releaseSlug;
    const details = card.querySelector("details");
    if (!details || !slug) return;

    if (sessionStorage.getItem(prefix + slug) === "1") {
      details.open = true;
    }
  });

  // Listen for toggle events (works for both open and close)
  document.addEventListener("toggle", (e) => {
    const details = e.target;
    if (details.tagName !== "DETAILS") return;

    const card = details.closest("[data-release][data-release-slug]");
    if (!card) return;

    const slug = card.dataset.releaseSlug;
    if (!slug) return;

    if (details.open) {
      sessionStorage.setItem(prefix + slug, "1");
    } else {
      sessionStorage.removeItem(prefix + slug);
    }
  }, true);
})();
