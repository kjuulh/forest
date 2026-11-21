<script>
  import { releaseFlows } from "./lib/release-flow.js";

  export let timeline = [];
  export let org = "";
  export let project = "";

  $: flows = releaseFlows(timeline, { org, project });

  function shortLabel(label) {
    return label.length > 19 ? `${label.slice(0, 18)}…` : label;
  }
</script>

{#snippet graph(flow)}
  {#if flow.nodes.length}
    <div class="release-flow-scroll" role="region" aria-label={`${flow.project} ${flow.release} pipeline`}>
      <svg class="release-flow-svg" width={flow.width} height={flow.height} viewBox={`0 0 ${flow.width} ${flow.height}`} role="img" aria-label={`${flow.project} ${flow.release}: ${flow.stageCount} stages, ${flow.destinationCount} destinations`}>
        <title>{flow.project} · {flow.release}</title>
        <desc>
          {flow.nodes.map(node => `${node.label}: ${node.context}, ${node.stateLabel}${node.reason ? `, ${node.reason}` : ""}`).join(". ")}.
          {flow.edges.map(edge => `${flow.nodes.find(node => node.key === edge.source)?.label} → ${flow.nodes.find(node => node.key === edge.target)?.label}`).join(". ")}
        </desc>
        {#each flow.edges as edge}
          <path class="release-flow-edge" class:release-flow-fanout={edge.kind === "destination"} d={edge.path} />
          <polygon class="release-flow-arrow" points={edge.arrow} />
        {/each}
        {#each flow.nodes as node (node.key)}
          <g class={`release-flow-node release-flow-${node.tone}`} class:release-flow-destination={node.kind === "destination"}>
            <title>{node.label} · {node.context} · {node.stateLabel}{node.reason ? ` · ${node.reason}` : ""}{node.dependencies?.length ? ` · Depends on ${node.dependencies.join(", ")}` : ""}</title>
            <rect x={node.x} y={node.y} width={flow.nodeWidth} height={node.height} rx="7" />
            <text class="release-flow-node-label" x={node.x + 12} y={node.y + 20}>{shortLabel(node.label)}</text>
            {#if node.kind !== "destination"}<text class="release-flow-node-context" x={node.x + 12} y={node.y + 39}>{shortLabel(node.context)}</text>{/if}
            <text class="release-flow-node-state" x={node.x + 12} y={node.y + (node.kind === "destination" ? 37 : 57)}>{node.stateLabel}</text>
          </g>
        {/each}
      </svg>
    </div>
  {:else}
    <p class="release-flow-empty">No pipeline stages or destination states recorded for this release.</p>
  {/if}
  {#if flow.incompleteDependencies}
    <p class="release-flow-note">Some stage dependencies are unavailable.</p>
  {/if}
{/snippet}

{#if flows.length}
  <section class="release-flows" aria-label="Latest release flow per project">
    <h2 class="release-flows-heading">Latest release flow per project</h2>
    {#each flows as flow (flow.key)}
      <article class="release-flow">
        <header class="release-flow-header">
          <h3><span>{flow.project}</span><span class="release-flow-release">{flow.release}</span></h3>
          {#if flow.href}
            <a class="release-flow-link" href={flow.href} sedx-ignore>Full release graph <span aria-hidden="true">↗</span></a>
          {/if}
        </header>
        {#if flow.dense}
          <details class="release-flow-disclosure">
            <summary>{flow.stageCount} stages · {flow.destinationCount} destinations</summary>
            {@render graph(flow)}
          </details>
        {:else}
          {@render graph(flow)}
        {/if}
      </article>
    {/each}
  </section>
{/if}
