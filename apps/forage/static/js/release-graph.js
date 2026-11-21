(function () {
  if (window.__forageReleaseGraph) {
    window.__forageReleaseGraph();
    return;
  }

  var current = null;
  var controlSections = ["pipeline", "policies", "destinations"];

  function pageURL() {
    return location.origin + location.pathname + location.search;
  }

  function initialize() {
    if (current) current.destroy();
    current = null;
    var root = document.getElementById("release-graph");
    if (!root) return;
    current = attach(root, pageURL());
    current.schedule();
  }

  function attach(root, url) {
    var viewport = root.querySelector("[data-graph-viewport]");
    var svg = root.querySelector(".release-graph-svg");
    var queue = document.getElementById("release-queue");
    var inspector = root.querySelector("[data-graph-inspector]");
    var status = root.querySelector("[data-graph-refresh-status]");
    var lifetime = new AbortController();
    var request = null;
    var timer = null;
    var disposed = false;
    var pageActive = true;
    var onScreen = true;
    var mode = "active";
    var camera = { x: 0, y: 0, zoom: 1 };
    var nodes = [];
    var selected = null;
    var pinned = false;
    var tab = "details";
    var dirty = new Set();
    var drag = null;
    var space = false;
    var suppressClick = false;
    var actions = [];
    var planText = null;
    var resize = null;
    var intersection = null;
    var visibleSurfaces = new Set();

    function valid() {
      return !disposed && root.isConnected && document.getElementById("release-graph") === root && pageURL() === url;
    }

    function listen(target, event, handler, options) {
      target.addEventListener(event, handler, Object.assign({}, options, { signal: lifetime.signal }));
    }

    function findNode(key) {
      return nodes.find(function (node) { return node.dataset.nodeKey === key; });
    }

    function activeNode() {
      return nodes.find(function (node) { return node.dataset.state === "running"; })
        || nodes.find(function (node) { return node.dataset.state === "waiting"; })
        || nodes.find(function (node) { return node.dataset.state === "next"; })
        || nodes.find(function (node) { return node.dataset.state === "failed"; })
        || nodes[0];
    }

    function targetFor(node) {
      var href = node && node.getAttribute("href");
      return href && href.charAt(0) === "#" ? document.getElementById(href.slice(1)) : null;
    }

    function text(field, value) {
      var element = root.querySelector("[data-graph-inspector-" + field + "]");
      if (element) element.textContent = value || "";
    }

    function renderActions(node) {
      var target = targetFor(node);
      actions = target ? Array.from(target.querySelectorAll('button[type="submit"], input[type="submit"]')).filter(function (button) {
        var form = button.form;
        if (!form || form.method.toLowerCase() !== "post") return false;
        var environment = form.querySelector('[name="target_environment"]');
        return !environment || environment.value === node.dataset.nodeEnvironment;
      }) : [];
      var container = root.querySelector("[data-graph-approval-actions]");
      var signature = JSON.stringify(actions.map(function (button) {
        return [button.form.action, button.textContent || button.value, button.disabled];
      }));
      // Preserve focused mirror controls while reconnecting them to the current real forms.
      if (container.dataset.signature === signature) return;
      container.dataset.signature = signature;
      container.replaceChildren();
      actions.forEach(function (source, index) {
        var button = document.createElement("button");
        button.type = "button";
        button.textContent = source.textContent.trim() || source.value;
        button.disabled = source.disabled;
        button.dataset.graphSubmit = String(index);
        button.dataset.approvalAction = /reject/i.test(button.textContent) ? "reject" : "approve";
        container.append(button);
      });
    }

    function renderPlan(node) {
      var index = node ? node.dataset.planIndex : "";
      var template = index !== "" && root.querySelector('template[data-graph-plan="' + index + '"]');
      var output = template ? template.content.textContent : null;
      if (output === planText) return;
      planText = output;
      var panel = root.querySelector('[data-graph-panel="plan"]');
      if (template) panel.replaceChildren(template.content.cloneNode(true));
      else {
        var empty = document.createElement("p");
        empty.textContent = "No plan output for this node.";
        panel.replaceChildren(empty);
      }
    }

    function inspect(node, pin) {
      if (!node) {
        selected = null;
        pinned = false;
        ["kind", "state", "summary", "timestamp", "context", "reason", "details", "preview"].forEach(function (field) { text(field, ""); });
        text("title", "No pipeline nodes");
        root.querySelector("[data-graph-selection-mode]").textContent = "Preview";
        root.querySelector('[data-graph-action="unpin"]').hidden = true;
        root.querySelector('[data-graph-action="view-stage"]').hidden = true;
        renderPlan(null);
        renderActions(null);
        return;
      }
      selected = node.dataset.nodeKey;
      if (pin) {
        pinned = true;
        inspector.open = true;
      }
      var data = node.dataset;
      text("kind", data.nodeKind);
      text("title", data.nodeLabel);
      text("state", [data.nodeEnvironment, data.stateLabel].filter(Boolean).join(" · "));
      text("summary", data.nodeSummary);
      text("timestamp", data.nodeTimestamp);
      text("context", data.nodeContext);
      text("reason", data.nodeReason);
      text("details", data.nodeDetails);
      text("preview", data.nodePreview !== data.nodeReason ? data.nodePreview : "");
      root.querySelector("[data-graph-inspector-state]").dataset.state = data.state;
      root.querySelector("[data-graph-selection-mode]").textContent = pinned ? "Pinned" : "Preview";
      root.querySelector('[data-graph-action="unpin"]').hidden = !pinned;
      root.querySelector('[data-graph-action="view-stage"]').hidden = !targetFor(node);
      nodes.forEach(function (candidate) {
        candidate.setAttribute("aria-pressed", String(pinned && candidate === node));
      });
      renderPlan(node);
      renderActions(node);
    }

    function mountLogs() {
      var panel = root.querySelector('[data-graph-panel="logs"]');
      if (!panel || root.dataset.hasLogs !== "true" || panel.querySelector("release-logs")) return;
      var source = document.querySelector("#logs release-logs[url]");
      if (!source) {
        panel.textContent = "Logs are not available for this release.";
        return;
      }
      var logs = document.createElement("release-logs");
      logs.id = source.id + "-graph";
      logs.setAttribute("url", source.getAttribute("url"));
      panel.replaceChildren(logs);
    }

    function selectTab(value) {
      if (!root.querySelector('[data-graph-tab="' + value + '"]')) value = "details";
      tab = value;
      root.querySelectorAll("[data-graph-tab]").forEach(function (button) {
        var active = button.dataset.graphTab === tab;
        button.setAttribute("aria-selected", String(active));
        button.tabIndex = active ? 0 : -1;
      });
      root.querySelectorAll("[data-graph-panel]").forEach(function (panel) {
        panel.hidden = panel.dataset.graphPanel !== tab;
      });
      if (tab === "logs") mountLogs();
    }

    function applyCamera() {
      if (!svg || !viewport.clientWidth || !viewport.clientHeight) return;
      svg.setAttribute("viewBox", [camera.x, camera.y, viewport.clientWidth / camera.zoom, viewport.clientHeight / camera.zoom].join(" "));
      root.querySelector("[data-graph-zoom-label]").textContent = Math.round(camera.zoom * 100) + "%";
      root.querySelectorAll("[data-graph-zoom]").forEach(function (button) {
        button.disabled = button.dataset.graphZoom === "in" ? camera.zoom >= 2.5 : camera.zoom <= 0.1;
      });
      ["fit", "active"].forEach(function (name) {
        root.querySelector('[data-graph-action="' + name + '"]').dataset.current = String(mode === name);
      });
    }

    function bounds(list) {
      var boxes = list.map(function (node) {
        var card = node.querySelector(".release-graph-card");
        return { x: Number(card.getAttribute("x")), y: Number(card.getAttribute("y")) };
      });
      if (!boxes.length) return { x: 0, y: 0, width: Number(svg.dataset.graphWidth) || 272, height: Number(svg.dataset.graphHeight) || 168 };
      var left = Math.min.apply(null, boxes.map(function (box) { return box.x; }));
      var top = Math.min.apply(null, boxes.map(function (box) { return box.y; }));
      return {
        x: left, y: top,
        width: Math.max.apply(null, boxes.map(function (box) { return box.x + 272; })) - left,
        height: Math.max.apply(null, boxes.map(function (box) { return box.y + 172; })) - top
      };
    }

    function fit() {
      if (!svg || !viewport.clientWidth || !viewport.clientHeight) return;
      var list = nodes;
      var active = mode === "selected" ? findNode(selected) || activeNode() : activeNode();
      var readable = mode === "active" || mode === "selected";
      if (readable && active) {
        var neighborhood = new Set([active.id]);
        svg.querySelectorAll("[data-from][data-to]").forEach(function (edge) {
          if (edge.dataset.from === active.id || edge.dataset.to === active.id) {
            neighborhood.add(edge.dataset.from);
            neighborhood.add(edge.dataset.to);
          }
        });
        list = nodes.filter(function (node) { return neighborhood.has(node.id); });
      }
      var box = mode === "fit"
        ? { x: 0, y: 0, width: Number(svg.dataset.graphWidth) || 272, height: Number(svg.dataset.graphHeight) || 168 }
        : bounds(list);
      var zoom = Math.min((viewport.clientWidth - 64) / box.width, (viewport.clientHeight - 80) / box.height, readable ? 1.2 : 1);
      // Wide fan-outs must not shrink the selected work below a readable size.
      if (readable && active && zoom < 0.65) {
        box = bounds([active]);
        zoom = Math.min((viewport.clientWidth - 48) / box.width, (viewport.clientHeight - 64) / box.height, 1);
      }
      camera.zoom = Math.max(mode === "fit" ? 0.001 : 0.1, Math.min(2.5, zoom));
      camera.x = box.x + box.width / 2 - viewport.clientWidth / camera.zoom / 2;
      camera.y = box.y + box.height / 2 - viewport.clientHeight / camera.zoom / 2;
      applyCamera();
    }

    function zoomAt(value, x, y) {
      var next = Math.max(0.1, Math.min(2.5, value));
      camera.x += x / camera.zoom - x / next;
      camera.y += y / camera.zoom - y / next;
      camera.zoom = next;
      mode = "manual";
      applyCamera();
    }

    function measure() {
      if (mode === "manual") applyCamera();
      else fit();
    }

    function openNode(key) {
      if (!valid()) return false;
      var node = key ? findNode(key) : null;
      if (key && !node) return false;
      if (node) {
        inspect(node, true);
        selectTab("details");
        mode = "selected";
        fit();
      }
      root.scrollIntoView({ behavior: "auto", block: "start" });
      (node || root).focus({ preventScroll: true });
      return true;
    }

    function fullscreenState() {
      var full = document.fullscreenElement === root || root.dataset.fullscreen === "true";
      var button = root.querySelector('[data-graph-action="fullscreen"]');
      button.setAttribute("aria-pressed", String(full));
      button.textContent = full ? "Exit fullscreen" : "Fullscreen";
      measure();
    }

    async function fullscreen() {
      try {
        if (document.fullscreenElement === root) await document.exitFullscreen();
        else if (root.dataset.fullscreen === "true") delete root.dataset.fullscreen;
        else if (root.requestFullscreen && document.fullscreenEnabled) await root.requestFullscreen();
        else root.dataset.fullscreen = "true";
      } catch (_) {
        if (valid()) root.dataset.fullscreen = "true";
      }
      if (valid()) fullscreenState();
    }

    async function viewStage() {
      var target = targetFor(findNode(selected));
      if (!target) return;
      if (document.fullscreenElement === root) {
        try { await document.exitFullscreen(); } catch (_) { return; }
      }
      delete root.dataset.fullscreen;
      fullscreenState();
      if (!valid() || !target.isConnected) return;
      var disclosure = target.closest("details");
      if (disclosure) disclosure.open = true;
      target.setAttribute("tabindex", "-1");
      target.scrollIntoView({ behavior: "auto", block: "center" });
      target.focus({ preventScroll: true });
    }

    function rememberDirty(event) {
      var section = event.target.closest("#pipeline, #policies, #destinations");
      if (section) dirty.add(section.id);
    }
    listen(document, "input", rememberDirty);
    listen(document, "change", rememberDirty);
    listen(root, "focusin", function (event) {
      var node = event.target.closest("[data-node-id]");
      if (node && !pinned && selected !== node.dataset.nodeKey) inspect(node, false);
    });
    listen(root, "pointerover", function (event) {
      var node = event.target.closest("[data-node-id]");
      if (node && !pinned && !drag && selected !== node.dataset.nodeKey) inspect(node, false);
    });
    listen(root, "click", function (event) {
      var node = event.target.closest("[data-node-id]");
      if (suppressClick) {
        event.preventDefault();
        event.stopPropagation();
        suppressClick = false;
        return;
      }
      if (node) {
        event.preventDefault();
        event.stopPropagation();
        inspect(node, true);
        return;
      }
      var zoom = event.target.closest("[data-graph-zoom]");
      if (zoom) zoomAt(camera.zoom * (zoom.dataset.graphZoom === "in" ? 1.25 : 0.8), viewport.clientWidth / 2, viewport.clientHeight / 2);
      var tabButton = event.target.closest("[data-graph-tab]");
      if (tabButton) selectTab(tabButton.dataset.graphTab);
      var submit = event.target.closest("[data-graph-submit]");
      if (submit) {
        var source = actions[Number(submit.dataset.graphSubmit)];
        if (source && source.isConnected && !source.disabled && source.form) source.form.requestSubmit(source);
      }
      var button = event.target.closest("[data-graph-action]");
      if (!button) return;
      switch (button.dataset.graphAction) {
        case "fit": mode = "fit"; fit(); break;
        case "active": mode = "active"; fit(); break;
        case "fullscreen": fullscreen(); break;
        case "view-stage": viewStage(); break;
        case "unpin": pinned = false; inspect(findNode(selected) || activeNode(), false); break;
      }
    }, { capture: true });

    listen(root, "keydown", function (event) {
      if (event.target.closest("input, textarea, select, [contenteditable], release-logs")) return;
      var node = event.target.closest("[data-node-id]");
      if (node && (event.key === "Enter" || event.key === " ")) {
        event.preventDefault();
        event.stopPropagation();
        inspect(node, true);
        return;
      }
      var tabButton = event.target.closest("[data-graph-tab]");
      if (tabButton && ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
        var tabs = Array.from(root.querySelectorAll("[data-graph-tab]"));
        var index = tabs.indexOf(tabButton);
        index = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
        event.preventDefault();
        selectTab(tabs[index].dataset.graphTab);
        tabs[index].focus();
        return;
      }
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      if (event.key.toLowerCase() === "f" || event.key.toLowerCase() === "a") {
        event.preventDefault();
        mode = event.key.toLowerCase() === "f" ? "fit" : "active";
        fit();
      }
      if (event.key === " " && (event.target === viewport || event.target === svg)) {
        event.preventDefault();
        space = true;
      }
    }, { capture: true });
    listen(document, "keydown", function (event) {
      if (event.key === "Escape" && root.dataset.fullscreen === "true") {
        delete root.dataset.fullscreen;
        fullscreenState();
        root.querySelector('[data-graph-action="fullscreen"]').focus({ preventScroll: true });
      }
    });
    listen(document, "keyup", function (event) { if (event.key === " ") space = false; });
    listen(window, "blur", function () { space = false; });
    listen(document, "fullscreenchange", fullscreenState);

    listen(viewport, "wheel", function (event) {
      event.preventDefault();
      var box = viewport.getBoundingClientRect();
      var delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
      zoomAt(camera.zoom * Math.exp(-Math.max(-120, Math.min(120, delta)) * 0.002), event.clientX - box.left, event.clientY - box.top);
    }, { passive: false });
    listen(viewport, "pointerdown", function (event) {
      if (event.button !== 0 && event.button !== 1) return;
      if (event.target.closest("[data-node-id]") && !space && event.button !== 1) return;
      event.preventDefault();
      viewport.focus({ preventScroll: true });
      suppressClick = false;
      drag = { id: event.pointerId, x: event.clientX, y: event.clientY, left: camera.x, top: camera.y, moved: false };
      viewport.setPointerCapture(event.pointerId);
    });
    listen(viewport, "pointermove", function (event) {
      if (!drag || drag.id !== event.pointerId) return;
      var dx = event.clientX - drag.x;
      var dy = event.clientY - drag.y;
      if (!drag.moved && Math.abs(dx) + Math.abs(dy) < 4) return;
      drag.moved = true;
      mode = "manual";
      viewport.dataset.dragging = "true";
      camera.x = drag.left - dx / camera.zoom;
      camera.y = drag.top - dy / camera.zoom;
      applyCamera();
    });
    function endDrag(event) {
      if (!drag || drag.id !== event.pointerId) return;
      suppressClick = drag.moved && event.type === "pointerup";
      if (suppressClick) setTimeout(function () { suppressClick = false; }, 0);
      drag = null;
      delete viewport.dataset.dragging;
      if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
    }
    listen(viewport, "pointerup", endDrag);
    listen(viewport, "pointercancel", endDrag);
    listen(viewport, "lostpointercapture", function () { drag = null; delete viewport.dataset.dragging; });

    function stop() {
      clearTimeout(timer);
      timer = null;
      if (request) request.abort();
      request = null;
    }

    function schedule() {
      clearTimeout(timer);
      if (valid() && !request && pageActive && onScreen && !document.hidden && root.dataset.live === "true") timer = setTimeout(refresh, 5000);
    }

    async function refresh() {
      if (!valid() || !pageActive || !onScreen || document.hidden || root.dataset.live !== "true") return;
      request = new AbortController();
      var pending = request;
      try {
        var response = await fetch(url, { method: "GET", credentials: "same-origin", cache: "no-store", redirect: "error", headers: { Accept: "text/html" }, signal: pending.signal });
        if (!response.ok) throw new Error("Refresh failed");
        var html = await response.text();
        if (!valid() || !pageActive || !onScreen || document.hidden || pending.signal.aborted) return;
        var parsed = new window.DOMParser().parseFromString(html, "text/html");
        var replacement = parsed.getElementById("release-graph");
        var nextSvg = replacement && replacement.querySelector(".release-graph-svg");
        if (!nextSvg) throw new Error("Release graph missing");
        if (drag) return;
        var active = document.activeElement;
        var focusWithin = root.contains(active);
        var focusedSubmit = active.getAttribute("data-graph-submit");
        var focusedNode = active.closest && active.closest("[data-node-id]");
        var focusedKey = focusedNode && focusedNode.dataset.nodeKey;
        var focusInQueue = queue && queue.contains(active);
        var focusedQueueControl = focusInQueue && active.closest("[data-release-graph-open]");
        var focusedQueueKey = focusedQueueControl && focusedQueueControl.dataset.queueNodeKey;
        var nextQueue = parsed.getElementById("release-queue");
        if (queue && nextQueue) {
          var scrollLeft = queue.querySelector(".release-queue-nodes").scrollLeft;
          if (intersection) {
            intersection.unobserve(queue);
            visibleSurfaces.delete(queue);
          }
          queue.replaceWith(nextQueue);
          queue = nextQueue;
          queue.querySelector(".release-queue-nodes").scrollLeft = scrollLeft;
          if (intersection) intersection.observe(queue);
        }
        controlSections.forEach(function (id) {
          var oldSection = document.getElementById(id);
          var newSection = parsed.getElementById(id);
          // Never discard edits, confirmation inputs or a focused approval control.
          if (oldSection && newSection && !oldSection.contains(active) && !dirty.has(id)) {
            var disclosures = oldSection.querySelectorAll("details");
            newSection.querySelectorAll("details").forEach(function (details, index) {
              if (disclosures[index]) details.open = disclosures[index].open;
            });
            oldSection.replaceWith(newSection);
          }
        });
        if (!document.getElementById("logs") && parsed.getElementById("logs")) root.before(parsed.getElementById("logs"));
        // Keep the workspace, inspector and mounted log element connected across updates.
        svg.replaceWith(nextSvg);
        svg = nextSvg;
        nodes = Array.from(svg.querySelectorAll("[data-node-id]"));
        root.querySelector("[data-graph-plans]").replaceWith(replacement.querySelector("[data-graph-plans]"));
        ["data-graph-count", "data-graph-release-context"].forEach(function (attribute) {
          root.querySelector("[" + attribute + "]").textContent = replacement.querySelector("[" + attribute + "]").textContent;
        });
        root.dataset.live = replacement.dataset.live;
        root.dataset.hasLogs = replacement.dataset.hasLogs;
        root.querySelector("[data-graph-live]").hidden = root.dataset.live !== "true";
        var nextEmpty = replacement.querySelector(".release-graph-empty");
        var oldEmpty = viewport.querySelector(".release-graph-empty");
        if (oldEmpty) oldEmpty.remove();
        if (nextEmpty) viewport.append(nextEmpty);
        var logsTab = root.querySelector('[data-graph-tab="logs"]');
        var logsPanel = root.querySelector('[data-graph-panel="logs"]');
        if (root.dataset.hasLogs === "true" && !logsTab) {
          root.querySelector(".release-graph-tabs").append(replacement.querySelector('[data-graph-tab="logs"]'));
          root.querySelector('[data-graph-panel="plan"]').after(replacement.querySelector('[data-graph-panel="logs"]'));
        } else if (root.dataset.hasLogs !== "true" && logsTab) {
          logsTab.remove();
          logsPanel.remove();
        }
        var remembered = findNode(selected);
        if (!remembered) pinned = false;
        inspect(remembered || activeNode(), false);
        selectTab(tab);
        measure();
        if (focusInQueue) {
          var queueControl = queue && !queue.hidden && (
            Array.from(queue.querySelectorAll("[data-queue-node-key]")).find(function (node) { return node.dataset.queueNodeKey === focusedQueueKey; })
            || queue.querySelector("[data-release-graph-open]")
          );
          (queueControl || root).focus({ preventScroll: true });
        } else if (focusedKey) (findNode(focusedKey) || viewport).focus({ preventScroll: true });
        else if (focusWithin && !active.isConnected) {
          var restored = focusedSubmit !== null && root.querySelector('[data-graph-submit="' + focusedSubmit + '"]');
          (restored || viewport).focus({ preventScroll: true });
        }
        status.textContent = "";
        delete status.dataset.error;
      } catch (error) {
        if (valid() && !document.hidden && !pending.signal.aborted) {
          status.dataset.error = "true";
          status.textContent = "Live update failed. Showing the last received graph; retrying in 5 seconds.";
        }
      } finally {
        if (request === pending) request = null;
        if (!disposed) schedule();
      }
    }

    function destroy() {
      disposed = true;
      stop();
      lifetime.abort();
      if (resize) resize.disconnect();
      if (intersection) intersection.disconnect();
      delete root.dataset.fullscreen;
    }

    listen(document, "visibilitychange", function () {
      if (document.hidden) stop();
      else schedule();
    });
    listen(window, "pagehide", function () { pageActive = false; stop(); });
    listen(window, "pageshow", function () { pageActive = true; schedule(); });
    nodes = Array.from(root.querySelectorAll("[data-node-id]"));
    inspect(activeNode(), false);
    selectTab(tab);
    resize = new ResizeObserver(measure);
    resize.observe(viewport);
    if (window.IntersectionObserver) {
      intersection = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting) visibleSurfaces.add(entry.target);
          else visibleSurfaces.delete(entry.target);
        });
        onScreen = visibleSurfaces.size > 0;
        if (onScreen) schedule();
        else stop();
      });
      intersection.observe(root);
      if (queue) intersection.observe(queue);
    }
    measure();
    return { root: root, schedule: schedule, destroy: destroy, openNode: openNode };
  }

  var observer = new MutationObserver(function () {
    if (current && !current.root.isConnected) {
      current.destroy();
      current = null;
    }
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
  window.__forageReleaseGraph = initialize;
  document.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    var control = event.target.closest("[data-release-graph-open]");
    if (!control || !current || !current.openNode(control.dataset.queueNodeKey)) return;
    event.preventDefault();
    event.stopPropagation();
  }, { capture: true });
  document.addEventListener("sedx:load", initialize);
  initialize();
})();
