import { icon } from "./actions.js";

export async function setupShares(data) {
  const table = document.querySelector(".shares-table");
  const body = table.querySelector("tbody");
  const status = document.getElementById("shares-status");
  const retry = document.getElementById("shares-retry");
  const fallback = document.getElementById("shares-copy-link");
  const dateFormat = new Intl.DateTimeFormat(undefined, matchMedia("(max-width: 30rem)").matches
    ? { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }
    : { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

  async function load() {
    retry.hidden = true;
    status.textContent = "Loading shares…";
    try {
      const response = await fetch(data.uri_prefix + "__sharefs__/api/shares", { cache: "no-store" });
      if (!response.ok) throw new Error(`Could not load shares (${response.status}).`);
      const shares = await response.json();
      body.replaceChildren();
      for (const share of shares) {
        const row = body.insertRow();
        row.dataset.expires = share.expires_at;
        const file = document.createElement("a");
        file.textContent = share.path;
        file.href = data.uri_prefix + share.path.slice(1).split("/").map(encodeURIComponent).join("/");
        row.insertCell().append(file);
        for (const [field, className] of [["created_at", "cell-created"], ["expires_at", "cell-expiry"]]) {
          const cell = row.insertCell();
          cell.className = className;
          const time = document.createElement("time");
          const date = new Date(share[field] * 1000);
          if (Number.isFinite(date.getTime())) {
            time.dateTime = date.toISOString();
            time.textContent = dateFormat.format(date);
          } else {
            time.textContent = "Distant future";
          }
          cell.append(time);
        }
        const actions = row.insertCell();
        actions.className = "cell-actions";
        const copy = document.createElement("button");
        copy.type = "button";
        copy.className = "action-btn";
        copy.setAttribute("aria-label", `Copy link to ${share.path}`);
        copy.innerHTML = icon("copy");
        copy.addEventListener("click", async () => {
          fallback.hidden = true;
          try {
            await navigator.clipboard.writeText(share.url);
            copy.innerHTML = icon("check");
            status.textContent = "Link copied.";
          } catch {
            fallback.value = share.url;
            fallback.hidden = false;
            fallback.focus();
            fallback.select();
            status.textContent = "Copy the selected link.";
          }
        });
        const open = document.createElement("a");
        open.className = "action-btn";
        open.href = share.url;
        open.target = "_blank";
        open.rel = "noopener noreferrer";
        open.setAttribute("aria-label", `Open shared ${share.path}`);
        open.innerHTML = icon("open");
        const revoke = document.createElement("button");
        revoke.type = "button";
        revoke.className = "action-btn action-revoke";
        revoke.setAttribute("aria-label", `Deactivate share for ${share.path}`);
        revoke.innerHTML = icon("trash");
        revoke.addEventListener("click", async () => {
          revoke.disabled = true;
          try {
            const response = await fetch(data.uri_prefix + "__sharefs__/api/share", {
              method: "DELETE",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ path: share.path, token: new URL(share.url).searchParams.get("token") }),
            });
            if (!response.ok) throw new Error(`Could not deactivate share (${response.status}). Try again.`);
            fallback.hidden = true;
            row.remove();
            expireRows();
            status.textContent = "Share deactivated.";
          } catch (error) {
            status.textContent = error.message;
          } finally {
            revoke.disabled = false;
          }
        });
        actions.append(copy, open, revoke);
      }
      expireRows();
    } catch (error) {
      table.hidden = true;
      status.textContent = error.message;
      retry.hidden = false;
    }
  }

  function expireRows() {
    for (const row of body.rows) row.hidden = Number(row.dataset.expires) <= Date.now() / 1000;
    const count = [...body.rows].filter(row => !row.hidden).length;
    table.hidden = count === 0;
    status.textContent = count ? `${count} active ${count === 1 ? "link" : "links"}` : "No active shared links.";
  }

  retry.addEventListener("click", load);
  await load();
  setInterval(() => { if (!table.hidden) expireRows(); }, 30_000);
}
