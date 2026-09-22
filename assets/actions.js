export function setupActions(data) {
  const menu = document.getElementById("file-menu");
  const share = document.getElementById("share-popover");
  const rename = document.getElementById("rename-popover");
  const panels = [menu, share, rename];
  let context;
  let anchor;
  let selection = 0;
  let expanded;

  const fileURL = path => location.origin + data.uri_prefix + path.slice(1).split("/").map(encodeURIComponent).join("/");
  const hidePanels = () => panels.forEach(panel => {
    if (panel.matches(":popover-open")) panel.hidePopover();
  });
  panels.forEach(panel => panel.addEventListener("toggle", () => {
    if (!panels.some(item => item.matches(":popover-open"))) expanded?.setAttribute("aria-expanded", "false");
  }));

  function position(panel) {
    if (!panel.matches(":popover-open")) return;
    const origin = anchor.getBoundingClientRect();
    const bounds = panel.getBoundingClientRect();
    panel.style.left = `${Math.max(12, Math.min(origin.right - bounds.width, innerWidth - bounds.width - 12))}px`;
    panel.style.top = `${Math.max(12, Math.min(origin.bottom + 6, innerHeight - bounds.height - 12))}px`;
  }

  function show(panel) {
    hidePanels();
    expanded?.setAttribute("aria-expanded", "false");
    expanded = anchor;
    expanded.setAttribute("aria-expanded", "true");
    panel.showPopover();
    position(panel);
    panel.querySelector("input:not([hidden]), select, button, a:not([hidden])")?.focus();
  }
  window.addEventListener("resize", () => panels.forEach(position));
  window.addEventListener("scroll", hidePanels, { passive: true });

  function showRename() {
    document.getElementById("rename-status").textContent = "";
    const input = document.getElementById("rename-input");
    input.value = context.name;
    show(rename);
    input.select();
  }

  document.addEventListener("click", event => {
    const button = event.target.closest("[data-action]");
    if (!button) return;
    selection++;
    panels.forEach(panel => { const submit = panel.querySelector("button[type=submit]"); if (submit) submit.disabled = false; });
    const index = button.dataset.index;
    const file = index === undefined ? null : data.paths[Number(index)];
    context = file ? {
      name: file.name,
      path: data.href.replace(/\/$/, "") + "/" + file.name,
      isDir: file.path_type.endsWith("Dir"),
    } : { name: data.href.split("/").pop(), path: data.href, isDir: false };
    anchor = button;

    if (button.dataset.action === "share") {
      document.getElementById("share-name").textContent = context.name;
      document.getElementById("share-status").textContent = "";
      document.getElementById("share-link").hidden = true;
      show(share);
    } else if (button.dataset.action === "rename") {
      showRename();
    } else {
      const download = document.getElementById("menu-download");
      download.href = fileURL(context.path);
      download.hidden = context.isDir;
      document.getElementById("menu-rename").hidden = !data.allow_rename;
      show(menu);
    }
  });

  document.getElementById("menu-rename").addEventListener("click", showRename);
  document.getElementById("menu-download").addEventListener("click", hidePanels);
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && panels.some(panel => panel.matches(":popover-open"))) {
      hidePanels();
      anchor?.focus();
    }
  });

  document.getElementById("share-form").addEventListener("submit", async event => {
    event.preventDefault();
    const button = event.submitter;
    const status = document.getElementById("share-status");
    const path = context.path;
    const selected = selection;
    button.disabled = true;
    status.textContent = "Creating link…";
    try {
      const response = await fetch(data.uri_prefix + "__sharefs__/api/share", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path, expires_in: Number(document.getElementById("share-expiry").value) }),
      });
      if (!response.ok) throw new Error(`Cannot share this file (${response.status})`);
      const link = await response.json();
      if (selected !== selection) return;
      const input = document.getElementById("share-link");
      input.value = link.url;
      input.hidden = false;
      try {
        await navigator.clipboard.writeText(link.url);
        status.textContent = `Copied. Expires ${new Date(link.expires_at * 1000).toLocaleString()}.`;
      } catch {
        input.focus();
        input.select();
        status.textContent = "Link ready. Copy the selected URL.";
      }
    } catch (error) {
      if (selected === selection) status.textContent = error.message;
    } finally {
      if (selected === selection) {
        button.disabled = false;
        position(share);
      }
    }
  });

  document.getElementById("rename-form").addEventListener("submit", async event => {
    event.preventDefault();
    const button = event.submitter;
    const status = document.getElementById("rename-status");
    const name = document.getElementById("rename-input").value;
    const path = context.path;
    if (!name || name === "." || name === ".." || /[/\\\0]/.test(name)) {
      status.textContent = "Enter a file name without slashes.";
      return;
    }
    if (name === context.name) { hidePanels(); return; }
    const destination = fileURL(path.slice(0, path.lastIndexOf("/") + 1) + name);
    button.disabled = true;
    status.textContent = "Renaming…";
    try {
      const response = await fetch(fileURL(path), {
        method: "MOVE", headers: { Destination: destination, Overwrite: "F" },
      });
      if (response.status === 409) throw new Error("A file with that name already exists.");
      if (!response.ok) throw new Error(`Rename failed (${response.status}).`);
      if (data.kind === "Index") location.reload();
      else location.assign(destination + location.search);
    } catch (error) {
      status.textContent = error.message;
    } finally {
      button.disabled = false;
      position(rename);
    }
  });
}
