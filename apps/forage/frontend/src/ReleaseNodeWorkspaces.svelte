<script>
  import { latestReleases } from "./lib/release-flow.js";

  export let timeline = [];
  export let project = "";

  $: releases = latestReleases(timeline, { project }).filter(release => release.graph_html);

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

{#if releases.length}
  <section class="release-flows" aria-label="Latest release workspaces">
    <h2 class="release-flows-heading">Latest release workspaces</h2>
    {#each releases as release (release.project_name || project)}
      <div use:workspace={release.graph_html}></div>
    {/each}
  </section>
{/if}
