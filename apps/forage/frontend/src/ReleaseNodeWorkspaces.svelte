<script>
  import { onMount } from "svelte";
  import { createLazyReleaseGraph } from "./lib/lazy-release-graph.js";

  export let html = "";
  export let url = "";
  export let active = false;
  export let expanded = true;
  export let revision = 0;

  let host;
  let loader;
  let loadedHTML = "";
  let error = null;
  let inViewport = false;
  let documentVisible = false;
  let fullscreen = false;
  let openedActive = false;
  $: openedActive = expanded && (active || openedActive);
  $: displayedHTML = url ? loadedHTML : html;
  $: loader?.update({ expanded, active, revision, visible: documentVisible && (inViewport || fullscreen) });

  onMount(() => {
    if (!url) return;
    loader = createLazyReleaseGraph(url, state => {
      loadedHTML = state.html;
      error = state.error;
    });
    const visibilityChanged = () => { documentVisible = document.visibilityState === "visible"; };
    visibilityChanged();
    document.addEventListener("visibilitychange", visibilityChanged);
    const observer = new IntersectionObserver(entries => {
      inViewport = entries.some(entry => entry.isIntersecting);
    });
    observer.observe(host);
    const fullscreenChanged = () => {
      fullscreen = !!(document.fullscreenElement && host.contains(document.fullscreenElement))
        || !!host.querySelector('[data-fullscreen="true"]');
    };
    host.addEventListener("release-graph:fullscreen", fullscreenChanged);
    document.addEventListener("fullscreenchange", fullscreenChanged);
    const stop = () => { documentVisible = false; };
    window.addEventListener("pagehide", stop);
    window.addEventListener("pageshow", visibilityChanged);
    return () => {
      observer.disconnect();
      document.removeEventListener("visibilitychange", visibilityChanged);
      host.removeEventListener("release-graph:fullscreen", fullscreenChanged);
      document.removeEventListener("fullscreenchange", fullscreenChanged);
      window.removeEventListener("pagehide", stop);
      window.removeEventListener("pageshow", visibilityChanged);
      loader.destroy();
    };
  });

  function workspace(node, html) {
    let currentRoot = null;
    let currentHTML = null;

    function update(nextHTML) {
      if (nextHTML === currentHTML) return;
      currentHTML = nextHTML;
      const nextRoot = new DOMParser().parseFromString(nextHTML, "text/html")
        .querySelector("[data-release-graph]");
      if (!nextRoot) {
        node.replaceChildren();
        currentRoot = null;
        return;
      }

      const initialize = window.__forageReleaseGraph;
      if (currentRoot && currentRoot.id === nextRoot.id && typeof initialize?.update === "function") {
        initialize.update(currentRoot, nextRoot);
      } else {
        node.replaceChildren(nextRoot);
        currentRoot = nextRoot;
        initialize?.();
      }
    }

    update(html);
    return {
      update,
      destroy() {
        node.replaceChildren();
        currentRoot = null;
      },
    };
  }
</script>

<div bind:this={host}>
  {#if displayedHTML}
    <div use:workspace={displayedHTML}></div>
  {/if}
  {#if error && url && expanded}
    <p role="alert">{error}</p>
  {:else if !displayedHTML && url && expanded && openedActive}
    <p role="status">Loading pipeline graph…</p>
  {/if}
</div>
