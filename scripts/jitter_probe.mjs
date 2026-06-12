#!/usr/bin/env node
// Multi-window enemy/player jitter measurement.
//
// Opens N tabs in a CDP-controlled Chrome, forms a room, starts the game,
// then samples test_enemy_positions()/test_player_snapshot() from every tab
// for a while and reports:
//   - enemy position jumps (large per-sample displacement)
//   - enemy reversals ("moved forward then snapped back")
//   - cross-tab divergence for the same enemy
//   - remote-player view error vs the owning tab's true position
//   - post-stop convergence (the "slow crawl to final position" symptom)
//
// Usage:
//   node scripts/jitter_probe.mjs
// Env:
//   SLIME_CDP_BASE   (default http://127.0.0.1:9222)
//   SLIME_PAGE_URL   (default http://127.0.0.1:8080)
//   SLIME_SIGNALING  (default ws://127.0.0.1:3536)
//   SLIME_ICE        (default none)
//   SLIME_TABS       (default 3)
//   SLIME_SCENARIO   cluster | border   (default cluster)
//   SLIME_THROTTLE   index of tab to rAF-throttle to ~1.25Hz, -1 = none (default -1)
//   SLIME_DURATION   sampling ms (default 22000)
//   SLIME_OUT        raw sample dump path (default /tmp/jitter_samples.json)

const CDP_BASE = process.env.SLIME_CDP_BASE || "http://127.0.0.1:9222";
const PAGE_URL = process.env.SLIME_PAGE_URL || "http://127.0.0.1:8080";
const SIGNALING_URL = process.env.SLIME_SIGNALING || "ws://127.0.0.1:3536";
const ICE_URLS = process.env.SLIME_ICE || "none";
const TABS = Math.max(2, Number(process.env.SLIME_TABS || 3));
const SCENARIO = process.env.SLIME_SCENARIO || "cluster";
const THROTTLE = Number(process.env.SLIME_THROTTLE ?? -1);
const DURATION_MS = Number(process.env.SLIME_DURATION || 22000);
const SAMPLE_MS = 100;
const OUT_PATH = process.env.SLIME_OUT || "/tmp/jitter_samples.json";
const WINDOW_WIDTH = 1280;
const WINDOW_HEIGHT = 900;
const VISIBLE_MARGIN = 64;

import { writeFileSync } from "node:fs";

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchJson(path, init = {}) {
  const res = await fetch(`${CDP_BASE}${path}`, init);
  if (!res.ok) throw new Error(`CDP ${path} failed: ${res.status}`);
  return res.json();
}

class CdpPage {
  constructor(info) {
    this.wsUrl = info.webSocketDebuggerUrl;
    this.id = info.id;
    this.ws = null;
    this.nextId = 1;
    this.pending = new Map();
  }
  async connect() {
    this.ws = new WebSocket(this.wsUrl);
    await new Promise((resolve, reject) => {
      this.ws.addEventListener("open", resolve, { once: true });
      this.ws.addEventListener("error", (e) => reject(new Error(String(e))), { once: true });
    });
    this.ws.addEventListener("message", (event) => {
      const msg = JSON.parse(event.data);
      if (!("id" in msg)) return;
      const pending = this.pending.get(msg.id);
      if (!pending) return;
      this.pending.delete(msg.id);
      if (msg.error) pending.reject(new Error(msg.error.message));
      else pending.resolve(msg.result);
    });
    await this.command("Runtime.enable");
  }
  command(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.command("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.text || "evaluate exception");
    }
    return result.result?.value;
  }
  async slime(expr) {
    return this.evaluate(`(async () => (${expr}))()`);
  }
  async waitFor(expr, timeoutMs = 30000, label = expr) {
    const deadline = Date.now() + timeoutMs;
    let last;
    while (Date.now() < deadline) {
      last = await this.evaluate(expr);
      if (last) return last;
      await delay(150);
    }
    throw new Error(`Timed out waiting for ${label}; last=${last}`);
  }
}

let browserConnectionPromise = null;

async function getBrowserConnection() {
  if (!browserConnectionPromise) {
    browserConnectionPromise = (async () => {
      const version = await fetchJson("/json/version");
      const conn = {
        ws: new WebSocket(version.webSocketDebuggerUrl),
        nextId: 1,
        pending: new Map(),
      };
      await new Promise((resolve, reject) => {
        conn.ws.addEventListener("open", resolve, { once: true });
        conn.ws.addEventListener("error", (e) => reject(new Error(String(e))), { once: true });
      });
      conn.ws.addEventListener("message", (event) => {
        const msg = JSON.parse(event.data);
        if (!("id" in msg)) return;
        const pending = conn.pending.get(msg.id);
        if (!pending) return;
        conn.pending.delete(msg.id);
        if (msg.error) pending.reject(new Error(msg.error.message || JSON.stringify(msg.error)));
        else pending.resolve(msg.result);
      });
      conn.command = (method, params = {}) => {
        const id = conn.nextId++;
        return new Promise((resolve, reject) => {
          conn.pending.set(id, { resolve, reject });
          conn.ws.send(JSON.stringify({ id, method, params }));
        });
      };
      return conn;
    })();
  }
  return browserConnectionPromise;
}

async function newPage(url) {
  const browser = await getBrowserConnection();
  const created = await browser.command("Target.createTarget", {
    url,
    newWindow: true,
    background: false,
    width: WINDOW_WIDTH,
    height: WINDOW_HEIGHT,
  });
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const pages = await fetchJson("/json/list");
    const page = pages.find((p) => p.id === created.targetId && p.webSocketDebuggerUrl);
    if (page) return page;
    await delay(100);
  }
  throw new Error("could not resolve created page");
}

function parseFields(input) {
  return Object.fromEntries(
    String(input)
      .split(";")
      .map((part) => {
        const idx = part.indexOf("=");
        return idx === -1 ? [part, ""] : [part.slice(0, idx), part.slice(idx + 1)];
      })
  );
}

function parsePlayers(snapshot) {
  const fields = parseFields(snapshot);
  const out = { scene: fields.scene, local: null, remotes: {} };
  const [ln, lc = "0,0"] = String(fields.local || "none@0,0").split("@");
  const [lx, ly] = lc.split(",").map(Number);
  out.local = { name: ln, x: lx, y: ly };
  if (fields.remote && fields.remote !== "none") {
    for (const entry of fields.remote.split("|")) {
      const [name, coords = "0,0"] = entry.split("@");
      const [x, y] = coords.split(",").map(Number);
      out.remotes[name] = { x, y };
    }
  }
  return out;
}

function visibleFrom(row, x, y) {
  const px = row?.data?.px;
  const py = row?.data?.py;
  if (!Number.isFinite(px) || !Number.isFinite(py)) return false;
  return (
    x >= px - WINDOW_WIDTH / 2 - VISIBLE_MARGIN &&
    x <= px + WINDOW_WIDTH / 2 + VISIBLE_MARGIN &&
    y >= py - WINDOW_HEIGHT / 2 - VISIBLE_MARGIN &&
    y <= py + WINDOW_HEIGHT / 2 + VISIBLE_MARGIN
  );
}

async function waitForNet(page, predicate, label) {
  const deadline = Date.now() + 25000;
  let last = "";
  while (Date.now() < deadline) {
    last = await page.slime("window.slimeTest.net()");
    if (predicate(parseFields(last))) return last;
    await delay(300);
  }
  throw new Error(`Timed out: ${label}; last=${last}`);
}

async function main() {
  const names = Array.from({ length: TABS }, (_, i) => `JIT${String.fromCharCode(65 + i)}`);
  const pages = [];

  // Creator
  {
    const info = await newPage(PAGE_URL);
    const page = new CdpPage(info);
    await page.connect();
    await page.waitFor("typeof window.slimeTest === 'object'", 40000, "page1 ready");
    pages.push(page);
  }
  await pages[0].slime(`window.slimeTest.reloadWithProfile(${JSON.stringify(names[0])}, "")`);
  await pages[0].waitFor("typeof window.slimeTest === 'object' && typeof window.slimeTest.createRoom === 'function'", 40000, "page1 reload");
  await pages[0].slime(`window.wasmBindings.set_signaling_server(${JSON.stringify(SIGNALING_URL)})`);
  await pages[0].slime(`window.wasmBindings.set_ice_servers(${JSON.stringify(ICE_URLS)}, "", "")`);
  const room = await pages[0].slime("window.slimeTest.createRoom()");
  if (typeof room !== "string" || room.length < 4) throw new Error(`bad room: ${room}`);
  await waitForNet(pages[0], (f) => f.room === room && f.local_peer !== "none", "creator peer");

  for (let i = 1; i < TABS; i += 1) {
    const info = await newPage(PAGE_URL);
    const page = new CdpPage(info);
    await page.connect();
    await page.waitFor("typeof window.slimeTest === 'object'", 40000, `page${i + 1} ready`);
    pages.push(page);
    await page.slime(`window.slimeTest.reloadWithProfile(${JSON.stringify(names[i])}, ${JSON.stringify(room)})`);
    await page.waitFor("typeof window.slimeTest === 'object' && typeof window.slimeTest.joinSavedRoom === 'function'", 40000, `page${i + 1} reload`);
    await page.slime(`window.wasmBindings.set_signaling_server(${JSON.stringify(SIGNALING_URL)})`);
    await page.slime(`window.wasmBindings.set_ice_servers(${JSON.stringify(ICE_URLS)}, "", "")`);
    await page.slime("window.slimeTest.joinSavedRoom()");
    await waitForNet(page, (f) => f.room === room && f.local_peer !== "none", `page${i + 1} peer`);
  }

  // Everyone sees everyone.
  for (let i = 0; i < pages.length; i += 1) {
    await waitForNet(pages[i], (f) => Number(f.remote_players) === TABS - 1, `page${i + 1} roster`);
  }

  for (const page of pages) {
    await page.slime("window.slimeTest.startGame()");
  }
  await delay(800);

  // Teleport layout.
  const layouts = {
    cluster: [[0, 0], [240, 0], [0, 240], [-240, 0], [0, -240]],
    border: [[1900, 0], [2200, 0], [2050, 240], [1700, 0], [2400, 0]],
  };
  const spots = layouts[SCENARIO] || layouts.cluster;
  for (let i = 0; i < pages.length; i += 1) {
    const [x, y] = spots[i] || [i * 150, 0];
    await pages[i].slime(`window.slimeTest.teleport(${x}, ${y})`);
  }

  // Continuous movement on every tab so AI targets move realistically.
  for (const page of pages) {
    await page.slime("window.slimeTest.keepAliveStart(500, 0.7)");
  }

  if (THROTTLE >= 0 && THROTTLE < pages.length) {
    await pages[THROTTLE].slime(
      "window.__origRaf ||= window.requestAnimationFrame.bind(window); window.requestAnimationFrame = (cb) => setTimeout(() => cb(performance.now()), 800); 'throttled'"
    );
  }

  await delay(1500);

  // ---- Sampling ----
  const samples = []; // {t, page, enemies: {key: [x,y]}, players, areas}
  const t0 = Date.now();
  while (Date.now() - t0 < DURATION_MS) {
    const t = Date.now() - t0;
    const rows = await Promise.all(
      pages.map(async (page, idx) => {
        try {
          const raw = await page.slime("window.wasmBindings.test_enemy_positions()");
          const playersRaw = await page.slime("window.slimeTest.players()");
          return { t, page: idx, data: JSON.parse(raw), players: parsePlayers(playersRaw) };
        } catch (err) {
          return { t, page: idx, error: String(err.message || err) };
        }
      })
    );
    samples.push(...rows);
    const elapsed = Date.now() - t0 - t;
    if (elapsed < SAMPLE_MS) await delay(SAMPLE_MS - elapsed);
  }

  // ---- Stop-convergence phase: stop all movement, keep sampling ----
  for (const page of pages) {
    await page.slime("window.slimeTest.releaseAll()");
  }
  const stopT = Date.now() - t0;
  const stopDeadline = Date.now() + 4000;
  while (Date.now() < stopDeadline) {
    const t = Date.now() - t0;
    const rows = await Promise.all(
      pages.map(async (page, idx) => {
        try {
          const raw = await page.slime("window.wasmBindings.test_enemy_positions()");
          const playersRaw = await page.slime("window.slimeTest.players()");
          return { t, page: idx, data: JSON.parse(raw), players: parsePlayers(playersRaw) };
        } catch (err) {
          return { t, page: idx, error: String(err.message || err) };
        }
      })
    );
    samples.push(...rows);
    await delay(SAMPLE_MS);
  }

  if (THROTTLE >= 0 && THROTTLE < pages.length) {
    await pages[THROTTLE].slime(
      "window.__origRaf && (window.requestAnimationFrame = window.__origRaf); 'restored'"
    );
  }

  const logs = await Promise.all(
    pages.map((p) => p.slime("window.slimeTest.logs()").catch(() => ""))
  );

  writeFileSync(
    OUT_PATH,
    JSON.stringify({ scenario: SCENARIO, throttle: THROTTLE, names, stopT, samples, logs }, null, 0)
  );

  // ---- Analysis ----
  const perPage = new Map(); // page -> [samples sorted by t]
  for (const s of samples) {
    if (s.error || !s.data || !s.data.enemies) continue;
    if (!perPage.has(s.page)) perPage.set(s.page, []);
    perPage.get(s.page).push(s);
  }

  const jumpEvents = [];
  const visibleJumpEvents = [];
  const reversalEvents = [];
  for (const [pageIdx, rows] of perPage) {
    const tracks = new Map(); // enemyKey -> [{t,x,y}]
    for (const row of rows) {
      for (const [ty, id, x, y] of row.data.enemies) {
        const key = `${ty}:${id}`;
        if (!tracks.has(key)) tracks.set(key, []);
        tracks.get(key).push({ t: row.t, x, y, row });
      }
    }
    for (const [key, points] of tracks) {
      for (let i = 1; i < points.length; i += 1) {
        const dt = points[i].t - points[i - 1].t;
        if (dt > 350) continue; // gap (enemy temporarily dead/missing)
        const dx = points[i].x - points[i - 1].x;
        const dy = points[i].y - points[i - 1].y;
        const dist = Math.hypot(dx, dy);
        const allowance = 45 * Math.max(1, dt / 100);
        if (dist > allowance) {
          const event = { page: pageIdx, key, t: points[i].t, dist: Math.round(dist), dt };
          jumpEvents.push(event);
          if (
            visibleFrom(points[i - 1].row, points[i - 1].x, points[i - 1].y) ||
            visibleFrom(points[i].row, points[i].x, points[i].y)
          ) {
            visibleJumpEvents.push(event);
          }
        }
        if (i >= 2) {
          const pdx = points[i - 1].x - points[i - 2].x;
          const pdy = points[i - 1].y - points[i - 2].y;
          const pd = Math.hypot(pdx, pdy);
          if (pd > 20 && dist > 20) {
            const cos = (dx * pdx + dy * pdy) / (dist * pd);
            if (cos < -0.5) {
              reversalEvents.push({
                page: pageIdx,
                key,
                t: points[i].t,
                fwd: Math.round(pd),
                back: Math.round(dist),
              });
            }
          }
        }
      }
    }
  }

  // Cross-page divergence at aligned timestamps.
  const divergences = [];
  const visibleDivergences = [];
  const pageList = [...perPage.keys()].sort();
  for (let a = 0; a < pageList.length; a += 1) {
    for (let b = a + 1; b < pageList.length; b += 1) {
      const rowsA = perPage.get(pageList[a]);
      const rowsB = perPage.get(pageList[b]);
      let j = 0;
      for (const rowA of rowsA) {
        while (j < rowsB.length - 1 && rowsB[j].t < rowA.t - 60) j += 1;
        const rowB = rowsB[j];
        if (!rowB || Math.abs(rowB.t - rowA.t) > 80) continue;
        const mapB = new Map(rowB.data.enemies.map(([ty, id, x, y]) => [`${ty}:${id}`, [x, y]]));
        for (const [ty, id, x, y] of rowA.data.enemies) {
          const other = mapB.get(`${ty}:${id}`);
          if (!other) continue;
          const dist = Math.hypot(x - other[0], y - other[1]);
          divergences.push(dist);
          if (visibleFrom(rowA, x, y) || visibleFrom(rowB, other[0], other[1])) {
            visibleDivergences.push(dist);
          }
        }
      }
    }
  }
  divergences.sort((m, n) => m - n);
  visibleDivergences.sort((m, n) => m - n);
  const pct = (p) => (divergences.length ? Math.round(divergences[Math.floor(p * (divergences.length - 1))]) : 0);
  const vpct = (p) =>
    visibleDivergences.length
      ? Math.round(visibleDivergences[Math.floor(p * (visibleDivergences.length - 1))])
      : 0;

  // Remote player view error: page i's truth (local px/py) vs others' view.
  const playerErr = [];
  for (const [pageIdx, rows] of perPage) {
    for (const row of rows) {
      const truthName = names[pageIdx];
      for (const [otherIdx, otherRows] of perPage) {
        if (otherIdx === pageIdx) continue;
        // nearest sample on other page
        let best = null;
        for (const r of otherRows) {
          if (Math.abs(r.t - row.t) <= 60 && (!best || Math.abs(r.t - row.t) < Math.abs(best.t - row.t))) best = r;
        }
        const seen = best?.players?.remotes?.[truthName];
        if (seen) {
          playerErr.push(Math.hypot(seen.x - row.data.px, seen.y - row.data.py));
        }
      }
    }
  }
  playerErr.sort((m, n) => m - n);
  const ppct = (p) => (playerErr.length ? Math.round(playerErr[Math.floor(p * (playerErr.length - 1))]) : 0);

  const enemyCount = perPage.get(0)?.at(-1)?.data.enemies.length ?? 0;
  console.log(`=== jitter probe: scenario=${SCENARIO} tabs=${TABS} throttle=${THROTTLE} ===`);
  console.log(`samples per page: ${perPage.get(0)?.length ?? 0}; enemies at end (page1): ${enemyCount}`);
  console.log(`enemy JUMPS (> ~45px/100ms): ${jumpEvents.length}`);
  for (const e of jumpEvents.slice(0, 12)) console.log(`  page${e.page + 1} ${e.key} t=${e.t}ms dist=${e.dist}px dt=${e.dt}ms`);
  console.log(`visible enemy JUMPS (> ~45px/100ms): ${visibleJumpEvents.length}`);
  for (const e of visibleJumpEvents.slice(0, 12)) console.log(`  page${e.page + 1} ${e.key} t=${e.t}ms dist=${e.dist}px dt=${e.dt}ms`);
  console.log(`enemy REVERSALS (fwd>20px then back>20px, cos<-0.5): ${reversalEvents.length}`);
  for (const e of reversalEvents.slice(0, 12)) console.log(`  page${e.page + 1} ${e.key} t=${e.t}ms fwd=${e.fwd} back=${e.back}`);
  console.log(`cross-page enemy divergence px: p50=${pct(0.5)} p90=${pct(0.9)} p99=${pct(0.99)} max=${pct(1)} (n=${divergences.length})`);
  console.log(`visible cross-page enemy divergence px: p50=${vpct(0.5)} p90=${vpct(0.9)} p99=${vpct(0.99)} max=${vpct(1)} (n=${visibleDivergences.length})`);
  console.log(`remote player view error px: p50=${ppct(0.5)} p90=${ppct(0.9)} p99=${ppct(0.99)} max=${ppct(1)} (n=${playerErr.length})`);
  console.log(`raw samples: ${OUT_PATH}`);
}

main().catch((err) => {
  console.error("PROBE FAILED:", err.message || err);
  process.exit(1);
});
