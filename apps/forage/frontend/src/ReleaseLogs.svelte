<svelte:options customElement="release-logs" />

<script>
  let { url = "" } = $props();

  // State
  let destinations = $state({});
  let activeTab = $state(null);
  let connected = $state(false);
  let done = $state(false);
  let autoScroll = $state(true);
  let showTimestamps = $state(true);
  let expanded = $state(false);

  let logContainer = $state(null);

  // Derived: sorted destination names
  let destNames = $derived(Object.keys(destinations).sort());
  let activeLines = $derived(activeTab && destinations[activeTab] ? destinations[activeTab] : []);

  function connect() {
    if (!url) return;
    const es = new EventSource(url);
    connected = true;

    es.addEventListener("log", (e) => {
      try {
        const data = JSON.parse(e.data);
        const dest = data.destination || "unknown";
        if (!destinations[dest]) {
          destinations[dest] = [];
          if (!activeTab) activeTab = dest;
        }
        destinations[dest] = [
          ...destinations[dest],
          {
            line: data.line,
            timestamp: data.timestamp,
            channel: data.channel || "stdout",
          },
        ];
        if (autoScroll) {
          requestAnimationFrame(() => {
            if (logContainer) {
              logContainer.scrollTop = logContainer.scrollHeight;
            }
          });
        }
      } catch (err) {
        console.warn("[release-logs] bad log event:", err);
      }
    });

    es.addEventListener("status", (e) => {
      try {
        const data = JSON.parse(e.data);
        const dest = data.destination || "unknown";
        if (!destinations[dest]) {
          destinations[dest] = [];
          if (!activeTab) activeTab = dest;
        }
        destinations[dest] = [
          ...destinations[dest],
          {
            line: `── ${data.status} ──`,
            timestamp: "",
            channel: "status",
          },
        ];
      } catch {}
    });

    es.addEventListener("done", () => {
      done = true;
    });

    es.addEventListener("error", () => {
      connected = false;
      es.close();
    });

    return () => {
      es.close();
      connected = false;
    };
  }

  $effect(() => {
    if (url) {
      const cleanup = connect();
      return cleanup;
    }
  });

  function handleScroll() {
    if (!logContainer) return;
    const atBottom =
      logContainer.scrollHeight - logContainer.scrollTop - logContainer.clientHeight < 40;
    autoScroll = atBottom;
  }

  function scrollToBottom() {
    if (logContainer) {
      logContainer.scrollTop = logContainer.scrollHeight;
      autoScroll = true;
    }
  }

  function parseTs(ts) {
    if (!ts) return null;
    const n = Number(ts);
    if (Number.isFinite(n) && n > 1e12) return n;
    const d = new Date(ts);
    return isNaN(d.getTime()) ? null : d.getTime();
  }

  function formatElapsed(ts, baseTs) {
    const ms = parseTs(ts);
    if (ms === null || baseTs === null) return "";
    const diff = ms - baseTs;
    if (diff < 0) return "0s";
    const totalSec = Math.floor(diff / 1000);
    if (totalSec < 60) return `${totalSec}s`;
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    return `${m}m${String(s).padStart(2, "0")}s`;
  }

  // Base timestamp per destination (first log line)
  let baseTimes = $derived.by(() => {
    const bt = {};
    for (const [dest, lines] of Object.entries(destinations)) {
      for (const line of lines) {
        if (line.timestamp) {
          bt[dest] = parseTs(line.timestamp);
          break;
        }
      }
    }
    return bt;
  });

  let activeBaseTime = $derived(activeTab ? baseTimes[activeTab] ?? null : null);

  function formatWallClock(ts) {
    const ms = parseTs(ts);
    if (ms === null) return "";
    const d = new Date(ms);
    const h = String(d.getHours()).padStart(2, "0");
    const m = String(d.getMinutes()).padStart(2, "0");
    const s = String(d.getSeconds()).padStart(2, "0");
    const frac = String(d.getMilliseconds()).padStart(3, "0");
    return `${h}:${m}:${s}.${frac}`;
  }
</script>

<div class="logs-root" class:expanded>
  {#if destNames.length === 0 && !done}
    <div class="logs-empty">
      {#if connected}
        <span class="logs-dot"></span> Waiting for logs…
      {:else}
        No logs available
      {/if}
    </div>
  {:else if destNames.length === 0 && done}
    <div class="logs-empty">No logs recorded for this release.</div>
  {:else}
    <!-- Header: tabs + controls -->
    <div class="logs-header">
      <div class="logs-tabs">
        {#each destNames as dest}
          <button
            class="logs-tab"
            class:active={activeTab === dest}
            onclick={() => (activeTab = dest)}
          >
            {dest}
            <span class="logs-count">{destinations[dest]?.length || 0}</span>
          </button>
        {/each}
      </div>
      <div class="logs-controls">
        {#if connected && !done}
          <span class="logs-live">
            <span class="logs-dot"></span> Live
          </span>
        {/if}
        <button
          class="logs-ctrl-btn"
          class:active={showTimestamps}
          onclick={() => (showTimestamps = !showTimestamps)}
          title="Toggle timestamps"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
        </button>
        <button
          class="logs-ctrl-btn"
          onclick={() => (expanded = !expanded)}
          title={expanded ? "Collapse" : "Expand"}
        >
          {#if expanded}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="4 14 10 14 10 20"/><polyline points="20 10 14 10 14 4"/><line x1="14" y1="10" x2="21" y2="3"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
          {:else}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
          {/if}
        </button>
      </div>
    </div>

    <!-- Log output -->
    <div class="logs-output" bind:this={logContainer} onscroll={handleScroll}>
      {#each activeLines as entry, i}
        <div
          class="logs-line"
          class:stderr={entry.channel === "stderr"}
          class:status-line={entry.channel === "status"}
        >
          {#if showTimestamps}
            <span class="logs-ts" title={formatWallClock(entry.timestamp)}>{formatElapsed(entry.timestamp, activeBaseTime)}</span>
          {/if}
          <span class="logs-text">{entry.line}</span>
        </div>
      {/each}
    </div>

    {#if !autoScroll}
      <button class="logs-scroll-btn" onclick={scrollToBottom}>
        ↓ Scroll to bottom
      </button>
    {/if}
  {/if}
</div>

<style>
  .logs-root {
    position: relative;
    border: 1px solid #e5e7eb;
    border-radius: 0.5rem;
    overflow: hidden;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.8125rem;
    line-height: 1.625;
    background: #111827;
    color: #d1d5db;
  }

  .logs-empty {
    padding: 2rem;
    text-align: center;
    color: #6b7280;
    font-family: system-ui, -apple-system, sans-serif;
    font-size: 0.875rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
  }

  .logs-header {
    display: flex;
    align-items: center;
    background: #1f2937;
    border-bottom: 1px solid #374151;
  }

  .logs-tabs {
    display: flex;
    gap: 0;
    overflow-x: auto;
    flex: 1;
    min-width: 0;
  }

  .logs-tab {
    padding: 0.5rem 1rem;
    font-size: 0.75rem;
    font-family: system-ui, -apple-system, sans-serif;
    color: #9ca3af;
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    transition: color 0.15s, border-color 0.15s;
  }

  .logs-tab:hover {
    color: #e5e7eb;
  }

  .logs-tab.active {
    color: #f9fafb;
    border-bottom-color: #3b82f6;
  }

  .logs-count {
    font-size: 0.625rem;
    padding: 0.0625rem 0.375rem;
    border-radius: 9999px;
    background: #374151;
    color: #9ca3af;
  }

  .logs-controls {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0 0.5rem;
    flex-shrink: 0;
  }

  .logs-ctrl-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.25rem;
    border: none;
    background: transparent;
    color: #6b7280;
    cursor: pointer;
    transition: color 0.15s, background 0.15s;
  }

  .logs-ctrl-btn:hover {
    color: #d1d5db;
    background: #374151;
  }

  .logs-ctrl-btn.active {
    color: #93c5fd;
    background: #1e3a5f;
  }

  .logs-live {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-family: system-ui, -apple-system, sans-serif;
    font-size: 0.6875rem;
    color: #34d399;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding-right: 0.5rem;
  }

  .logs-dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 9999px;
    background: #34d399;
    display: inline-block;
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  .logs-output {
    max-height: 60vh;
    overflow-y: auto;
    padding: 0.25rem 0;
  }

  .logs-root.expanded .logs-output {
    max-height: 85vh;
  }

  .logs-output::-webkit-scrollbar {
    width: 0.5rem;
  }

  .logs-output::-webkit-scrollbar-track {
    background: #1f2937;
  }

  .logs-output::-webkit-scrollbar-thumb {
    background: #4b5563;
    border-radius: 0.25rem;
  }

  .logs-line {
    display: flex;
    padding: 0 1rem 0 0;
    gap: 0;
    min-height: 1.5rem;
  }

  .logs-line:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .logs-line.stderr {
    color: #fca5a5;
    background: rgba(239, 68, 68, 0.06);
  }

  .logs-line.stderr:hover {
    background: rgba(239, 68, 68, 0.1);
  }

  .logs-line.status-line {
    color: #93c5fd;
    font-weight: 600;
    padding-top: 0.375rem;
    padding-bottom: 0.375rem;
    border-top: 1px solid #1e3a5f;
    margin-top: 0.25rem;
  }

  .logs-ts {
    color: #4b5563;
    white-space: nowrap;
    user-select: none;
    flex-shrink: 0;
    width: 3.5rem;
    text-align: right;
    padding-right: 1rem;
    padding-left: 0.75rem;
    border-right: 1px solid #1f2937;
    margin-right: 0.75rem;
  }

  .logs-text {
    white-space: pre-wrap;
    word-break: break-all;
    flex: 1;
    min-width: 0;
    padding-left: 1rem;
  }

  .logs-line .logs-ts + .logs-text {
    padding-left: 0;
  }

  .logs-scroll-btn {
    position: absolute;
    bottom: 0.75rem;
    left: 50%;
    transform: translateX(-50%);
    padding: 0.25rem 0.75rem;
    font-size: 0.6875rem;
    font-family: system-ui, -apple-system, sans-serif;
    color: #d1d5db;
    background: #374151;
    border: 1px solid #4b5563;
    border-radius: 9999px;
    cursor: pointer;
    opacity: 0.9;
    transition: opacity 0.15s;
  }

  .logs-scroll-btn:hover {
    opacity: 1;
    background: #4b5563;
  }
</style>
