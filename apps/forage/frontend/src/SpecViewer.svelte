<svelte:options customElement="spec-viewer" />

<script>
  let { content = "", filename = "forest.cue" } = $props();

  let expanded = $state(false);
  let highlighted = $state("");

  // Simple CUE syntax highlighter
  function highlightCue(src) {
    // Escape HTML first
    let html = src
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");

    // Process tokens via regex replacement
    // Order matters: comments first, then strings, then keywords/numbers
    html = html
      // Line comments
      .replace(/(\/\/.*)/g, '<span class="hl-comment">$1</span>')
      // Strings (double-quoted, with escapes)
      .replace(/"(?:[^"\\]|\\.)*"/g, '<span class="hl-string">$&</span>')
      // Keywords
      .replace(
        /\b(package|import|let|if|for|in|true|false|null|enabled|path)\b/g,
        '<span class="hl-keyword">$1</span>'
      )
      // Numbers
      .replace(/\b(\d+)\b/g, '<span class="hl-number">$1</span>');

    return html;
  }

  $effect(() => {
    if (expanded && content && !highlighted) {
      highlighted = highlightCue(content);
    }
  });

  function toggle() {
    expanded = !expanded;
  }

  // Count lines for display
  let lineCount = $derived(content ? content.split("\n").length : 0);
</script>

<div class="spec-root" class:expanded>
  <button class="spec-header" onclick={toggle}>
    <div class="spec-header-left">
      <svg
        class="spec-chevron"
        class:rotated={expanded}
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
      >
        <polyline points="9 18 15 12 9 6" />
      </svg>
      <span class="spec-filename">{filename}</span>
    </div>
    <span class="spec-meta">{lineCount} lines</span>
  </button>

  {#if expanded}
    <div class="spec-code">
      <pre><code>{@html highlighted}</code></pre>
    </div>
  {/if}
</div>

<style>
  .spec-root {
    border: 1px solid #e5e7eb;
    border-radius: 0.5rem;
    overflow: hidden;
    font-family: system-ui, -apple-system, sans-serif;
  }

  .spec-root.expanded {
    max-height: 36rem;
    overflow-y: auto;
  }

  .spec-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 0.5rem 0.75rem;
    background: #f9fafb;
    border: none;
    border-bottom: 1px solid transparent;
    cursor: pointer;
    transition: background 0.15s;
  }

  .spec-root.expanded .spec-header {
    position: sticky;
    top: 0;
    z-index: 1;
    border-bottom-color: #e5e7eb;
  }

  .spec-header:hover {
    background: #f3f4f6;
  }

  .spec-header-left {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .spec-chevron {
    color: #6b7280;
    transition: transform 0.15s ease;
    flex-shrink: 0;
  }

  .spec-chevron.rotated {
    transform: rotate(90deg);
  }

  .spec-filename {
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.8125rem;
    font-weight: 500;
    color: #374151;
  }

  .spec-meta {
    font-size: 0.75rem;
    color: #9ca3af;
  }

  .spec-code {
    background: #111827;
  }

  .spec-root.expanded::-webkit-scrollbar {
    width: 0.5rem;
    height: 0.5rem;
  }

  .spec-root.expanded::-webkit-scrollbar-track {
    background: #1f2937;
  }

  .spec-root.expanded::-webkit-scrollbar-thumb {
    background: #4b5563;
    border-radius: 0.25rem;
  }

  .spec-code pre {
    margin: 0;
    padding: 1rem;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.8125rem;
    line-height: 1.625;
    color: #e5e7eb;
    white-space: pre;
    tab-size: 4;
    overflow-x: auto;
  }

  .spec-code code {
    color: inherit;
  }

  /* Syntax highlighting tokens */
  .spec-code :global(.hl-comment) {
    color: #6b7280;
    font-style: italic;
  }

  .spec-code :global(.hl-string) {
    color: #a5d6ff;
  }

  .spec-code :global(.hl-keyword) {
    color: #ff7b72;
  }

  .spec-code :global(.hl-number) {
    color: #79c0ff;
  }
</style>
